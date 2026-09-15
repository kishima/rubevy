//! rubevy: run mruby bytecode inside Bevy, using the [SabiRuby](sabiruby) VM.
//!
//! One VM for the whole app ([`ScriptWorld`]), and one **task** per script
//! (mruby-task): a [`Script`] component becomes a task of the scheduler, and
//! every frame the plugin gives the scheduler a budget of instructions. A task
//! that calls `sleep` costs nothing until its time comes, so hundreds of
//! scripts can sit on entities and wake only when they have something to do.
//!
//! What a script sees of the host:
//! * `$rubevy` — a Hash refreshed at the head of every frame (`:frame`,
//!   `:delta`, `:time`).
//! * `Rubevy.entity` — the entity this script is attached to, read off the task
//!   the scheduler is running. It is a `Rubevy::Entity` object, which carries
//!   `Entity::to_bits` as a handle the VM never reads through (`Vm::data_new`);
//!   `to_i` gives the number, `==` compares by it.
//! * `Rubevy.log`, `Rubevy.spawn`, `Rubevy.despawn`, `Rubevy.set_position`,
//!   `Rubevy.move_to` — these do not touch the Bevy world from inside the VM
//!   (a native cannot); they put a command on a queue that a system drains
//!   after the frame's scripts have run.
//! * `puts`/`p` output is forwarded to Bevy's log.
//!
//! A script may `require` another, which reads from the asset directory
//! ([`RubevyPlugin::with_asset_root`], `assets` by default): `.mrb` always,
//! `.rb` only in a build with the `ruby-source` feature, which brings the
//! reference compiler along.
//!
//! Scripts share one VM, so they share globals and constants. That is the
//! design, not an oversight: a game's scripts are written together. A use that
//! needs isolation wants a second VM, which this plugin does not build yet.

use bevy::asset::{io::Reader, Asset, AssetApp, AssetLoader, LoadContext};
use bevy::diagnostic::FrameCount;
use bevy::prelude::*;
use bevy::reflect::TypePath;

use sabiruby::convert::{DataRef, This};
use sabiruby::value::ObjId;
use sabiruby::{Value, Vm, VmError};

/// The bytes of a RITE binary (`.mrb`).
#[derive(Asset, TypePath, Debug, Clone)]
pub struct MrbAsset {
    pub bytes: Vec<u8>,
}

#[derive(Default, TypePath)]
pub struct MrbLoader;

#[derive(Debug, thiserror::Error)]
pub enum MrbLoadError {
    #[error("could not read .mrb: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a RITE binary: {0}")]
    Rite(String),
}

impl AssetLoader for MrbLoader {
    type Asset = MrbAsset;
    type Settings = ();
    type Error = MrbLoadError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        // Validate early so a broken file fails at load time, not at spawn time.
        sabiruby::rite::parse(&bytes).map_err(|e| MrbLoadError::Rite(e.to_string()))?;
        Ok(MrbAsset { bytes })
    }

    fn extensions(&self) -> &[&str] {
        &["mrb"]
    }
}

/// Attach to an entity to run a script as a task.
#[derive(Component, Debug, Clone)]
pub struct Script {
    pub source: Handle<MrbAsset>,
    /// 0-255, 0 first (mruby-task's priority).
    pub priority: u8,
    /// Shown in logs and answered by `Task#name`.
    pub name: Option<String>,
}

impl Script {
    pub fn new(source: Handle<MrbAsset>) -> Self {
        Script { source, priority: 128, name: None }
    }
    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority;
        self
    }
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
}

/// The task a [`Script`] became. Added by the plugin once the asset arrives.
///
/// Removing it (or despawning the entity) stops the task: it is terminated in the VM, so a script
/// replaced by a new [`Script`] — a reload, a restart — does not keep running beside the new one.
#[derive(Component, Debug, Clone, Copy)]
#[component(on_remove = stop_removed_task)]
pub struct ScriptTask {
    task: ObjId,
}

impl ScriptTask {
    /// The scheduler's id for this script, for the `Vm::task_*` entry points.
    pub fn task(&self) -> ObjId {
        self.task
    }
}

fn stop_removed_task(mut world: bevy::ecs::world::DeferredWorld, context: bevy::ecs::lifecycle::HookContext) {
    let Some(task) = world.get::<ScriptTask>(context.entity).map(|t| t.task) else { return };
    // a script that ended has been let go of already (`ScriptDone`)
    let ended = world.get::<ScriptDone>(context.entity).is_some();
    let Some(mut scripts) = world.get_resource_mut::<ScriptWorld>() else { return };
    scripts.stop_task(task, !ended);
}

/// What a host can show of a running script (`ScriptWorld::stats`).
#[derive(Debug, Clone, Default)]
pub struct ScriptStats {
    /// Instructions this script has run since it started. The difference between two frames is
    /// what it spent on that frame.
    pub instructions: u64,
    /// Where it stands in its own source: file and line, while it waits as well as while it
    /// runs. `None` where the program carries no debug info.
    pub location: Option<(String, u32)>,
    /// Every frame it stands in, innermost first. A script parked inside a library method (a
    /// DSL, `sleep`) stands in the library; this is how a panel finds the line of the script's
    /// own file that is waiting.
    pub frames: Vec<(String, u32)>,
    /// Whether the task has run to its end.
    pub finished: bool,
}

/// Marks an entity whose script has ended, so that [`ScriptEnded`] is sent once and the task
/// is let go of once. The [`ScriptTask`] stays, which is what keeps the script from starting
/// again.
#[derive(Component, Debug, Clone, Copy)]
pub struct ScriptDone;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptStatus {
    Finished,
    Failed,
}

/// Emitted when a script's task runs to its end, with the value it answered
/// (or the exception it did not handle, which mruby-task makes the result).
#[derive(Message, Debug, Clone)]
pub struct ScriptEnded {
    pub entity: Entity,
    pub status: ScriptStatus,
    pub value: String,
}

/// What a script asked the host to do. A native cannot touch the Bevy world,
/// so it leaves one of these behind and [`drain_commands`] carries it out.
#[derive(Debug, Clone)]
enum HostCommand {
    Log(String),
    Spawn { name: String, x: f32, y: f32, z: f32 },
    Despawn(u64),
    SetPosition { entity: u64, x: f32, y: f32, z: f32 },
    /// `Rubevy.ask`: the script is parked on a queue until the game answers it.
    Ask { entity: u64, kind: String, args: Vec<Arg>, queue: ObjId },
}

/// An argument of `Rubevy.ask`, after the name of what is being asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    Num(f64),
    Text(String),
    /// A `Rubevy::Entity` the script passed on: the object `Rubevy.entity` and
    /// [`Answer::Entity`] hand out, read back through its handle rather than through a number
    /// that has been past a `f64`.
    Entity(Entity),
}

impl Arg {
    pub fn as_num(&self) -> Option<f64> {
        match self {
            Arg::Num(n) => Some(*n),
            Arg::Text(_) | Arg::Entity(_) => None,
        }
    }
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Arg::Text(t) => Some(t),
            Arg::Num(_) | Arg::Entity(_) => None,
        }
    }
    /// The entity, where the script passed a `Rubevy::Entity` object.
    pub fn as_entity(&self) -> Option<Entity> {
        match self {
            Arg::Entity(e) => Some(*e),
            Arg::Num(_) | Arg::Text(_) => None,
        }
    }
}

/// Something a script asked the game for and is waiting on (`Rubevy.ask`). Take them with
/// [`ScriptWorld::take_requests`] in a system of your own, work out the answer, and give it back
/// with [`ScriptWorld::answer`] — this frame or any later one. The script's task is parked
/// meanwhile, so it costs nothing and the other scripts keep running.
#[derive(Debug, Clone)]
pub struct Request {
    /// The entity whose script asked, where it has one.
    pub entity: Option<Entity>,
    /// The first argument of `Rubevy.ask`, e.g. `"scan"`.
    pub kind: String,
    /// The rest of the arguments: numbers and strings, in the order they were written.
    pub args: Vec<Arg>,
    /// Hand this back to [`ScriptWorld::answer`]; it is the queue the script waits on.
    pub queue: ObjId,
}

/// What a game answers a [`Request`] with.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    Nil,
    Bool(bool),
    Num(f64),
    Text(String),
    /// A list of numbers, e.g. a position or what a sensor found.
    List(Vec<f64>),
    /// A list of rows of numbers: a table, e.g. every robot with its team and hp.
    Rows(Vec<Vec<f64>>),
    /// An entity, as the `Rubevy::Entity` object a script can pass back to `Rubevy.despawn`,
    /// `Rubevy.set_position` or `Rubevy.ask`. Inside [`Answer::List`] and [`Answer::Rows`] an
    /// entity is still a number (`Entity::to_bits` through a `f64`), which is what a table of
    /// them wants; this is the way to hand one over without that.
    Entity(Entity),
}

impl Request {
    /// The `i`th argument as a number, where it is one.
    pub fn num(&self, i: usize) -> Option<f64> {
        self.args.get(i).and_then(Arg::as_num)
    }
    /// The `i`th argument as a string, where it is one.
    pub fn text(&self, i: usize) -> Option<&str> {
        self.args.get(i).and_then(Arg::as_text)
    }
    /// The `i`th argument as a number, or `or` where there is none.
    pub fn num_or(&self, i: usize, or: f64) -> f64 {
        self.num(i).unwrap_or(or)
    }
    /// The `i`th argument as an entity, where the script passed a `Rubevy::Entity`.
    pub fn entity_arg(&self, i: usize) -> Option<Entity> {
        self.args.get(i).and_then(Arg::as_entity)
    }
}

/// Marks and names an entity a script spawned (`Rubevy.spawn`).
#[derive(Component, Debug, Clone)]
pub struct SpawnedByScript {
    pub name: String,
}

/// The one VM the scripts share, and the queue between them and the world.
#[derive(Resource)]
pub struct ScriptWorld {
    pub vm: Vm,
    /// Instructions the scheduler may spend per frame, over all tasks.
    pub budget: u64,
    /// Time the scripts may take per frame, on the VM's clock (Bevy's `Instant`). The running
    /// timeslice is cut short when it is up. `None`: instructions only.
    pub frame_time: Option<std::time::Duration>,
    /// Past this, a script that cannot be switched out — it is inside a native waiting for a
    /// block, `sort { }` or `Array.new { loop { } }` — gets `Task::Overrun` rather than holding
    /// the frame. `None`: no such limit.
    pub overrun: Option<std::time::Duration>,
    /// Ticks not yet handed to the scheduler (frame times shorter than a tick).
    tick_remainder: f32,
    /// What scripts asked the game for and are waiting on (`Rubevy.ask`).
    requests: Vec<Request>,
    /// The `Rubevy::Entity` class, for [`ScriptWorld::answer`] to build an [`Answer::Entity`]
    /// with. The natives carry it in their closures.
    entity_class: ObjId,
    /// How many entity objects the collector has taken, counted by the free hook.
    freed_entities: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl ScriptWorld {
    fn new() -> Result<ScriptWorld, String> {
        let mut vm = Vm::with_mrblib().map_err(|e| format!("mrblib: {e:?}"))?;
        // the clock is Bevy's, not the instruction count; what the instruction count still does
        // is end a timeslice, which is what keeps one script from eating a frame
        vm.task_external_clock(true);
        // the scheduler collects at its idle points instead of the allocation path, so a script
        // that allocates never pauses a frame for the collector (`docs/gc.md` of the VM)
        if let Err(e) = enable_scheduler_gc(&mut vm) {
            return Err(format!("GC.scheduler_driven: {e}"));
        }
        // the natives' side of the bridge lives in the VM, not in a static, so two `App`s in
        // one process each have their own (`Vm::set_host_state`)
        vm.set_host_state(HostState::default());
        let entity_class = install_host_api(&mut vm);
        // An entity object owns nothing on the host's side — its handle *is* `Entity::to_bits`,
        // and the ECS is what says whether that entity still exists — so there is nothing to
        // release here. The hook is registered all the same: it is the place a kind of Data that
        // does own something (a handle into a slab) would give it back, and counting keeps the
        // path exercised (`ScriptWorld::freed_entities`).
        let freed_entities = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let counter = freed_entities.clone();
        vm.set_on_free(Box::new(move |tag, _handle| {
            if tag == ENTITY_TAG {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }));
        // a clock for the time limits (`frame_time`, `overrun`). Timeslices stay counted in
        // instructions, so what a script does is the same on every machine; the clock only keeps a
        // frame from being lost to one that the count cannot stop
        vm.task_set_clock(Some(clock_ns));
        Ok(ScriptWorld {
            vm,
            budget: 200_000,
            frame_time: Some(std::time::Duration::from_millis(8)),
            overrun: Some(std::time::Duration::from_millis(50)),
            tick_remainder: 0.0,
            requests: Vec::new(),
            entity_class,
            freed_entities,
        })
    }

    /// What a script has spent and where it is, for a HUD or a debugger panel. The task comes
    /// from the entity's [`ScriptTask`].
    pub fn stats(&self, script: &ScriptTask) -> ScriptStats {
        ScriptStats {
            instructions: self.vm.task_instructions(script.task),
            location: self.vm.task_location(script.task),
            frames: self.vm.task_frames(script.task),
            finished: self.vm.task_finished(script.task),
        }
    }

    /// Terminates a task that is still running and, where `release`, lets the collector have it.
    fn stop_task(&mut self, task: ObjId, release: bool) {
        if !self.vm.task_finished(task) {
            let terminate = self.vm.intern("terminate");
            if let Err(e) = self.vm.funcall(Value::Obj(task), terminate, &[], Value::Nil) {
                let message = self.vm.describe_error(&e);
                error!("rubevy: could not stop a script: {message}");
            }
        }
        if release {
            self.vm.gc_unregister(task);
        }
    }

    /// The requests scripts made since the last call (`Rubevy.ask`), for a system of the game to
    /// answer. A request stays valid until it is answered: keep the ones you cannot answer yet
    /// and hand them back to [`ScriptWorld::answer`] on a later frame.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    /// How many `Rubevy::Entity` objects the collector has taken since the VM started, as the
    /// free hook ([`Vm::set_on_free`]) counted them. An entity object owns nothing on the
    /// host's side, so the hook has nothing to release; this is what shows it runs.
    pub fn freed_entities(&self) -> u64 {
        self.freed_entities.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Answers a request: the script's `Rubevy.ask` returns this value and its task becomes
    /// ready again. The queue is let go of here, so answer each request once.
    pub fn answer(&mut self, request: &Request, answer: Answer) {
        let value = match answer {
            Answer::Nil => Value::Nil,
            Answer::Bool(b) => Value::bool(b),
            Answer::Num(n) => Value::Float(n),
            Answer::Text(t) => self.vm.str_new(t.as_bytes()),
            Answer::List(ns) => {
                let items: Vec<Value> = ns.into_iter().map(Value::Float).collect();
                self.vm.ary_new(items)
            }
            Answer::Entity(e) => self.vm.data_new(self.entity_class, ENTITY_TAG, e.to_bits()),
            Answer::Rows(rows) => {
                let items: Vec<Value> = rows
                    .into_iter()
                    .map(|row| {
                        let cells: Vec<Value> = row.into_iter().map(Value::Float).collect();
                        self.vm.ary_new(cells)
                    })
                    .collect();
                self.vm.ary_new(items)
            }
        };
        if let Err(e) = self.vm.task_queue_push(request.queue, value) {
            let message = self.vm.describe_error(&e);
            error!("rubevy: could not answer {}: {message}", request.kind);
        }
        self.vm.gc_unregister(request.queue);
    }
}

/// Nanoseconds since the first call, on Bevy's `Instant` (which is also there in a browser).
fn clock_ns() -> u64 {
    static ORIGIN: std::sync::OnceLock<bevy::platform::time::Instant> = std::sync::OnceLock::new();
    ORIGIN.get_or_init(bevy::platform::time::Instant::now).elapsed().as_nanos() as u64
}

fn enable_scheduler_gc(vm: &mut Vm) -> Result<(), String> {
    let gc = vm.intern("GC");
    let Some(gc) = vm.const_get(vm.core.object, gc) else { return Ok(()) };
    let m = vm.intern("scheduler_driven=");
    vm.funcall(gc, m, &[Value::True], Value::Nil)
        .map(|_| ())
        .map_err(|e| vm.describe_error(&e))
}

/// What this plugin keeps inside the VM (`Vm::set_host_state`), for the natives to reach
/// through the `&mut Vm` they are given. One of these per VM, so two `App`s in one process
/// have a queue each — which is why the tests may run in parallel.
#[derive(Default)]
struct HostState {
    /// The queue a native writes and [`drain_commands`] reads.
    commands: Vec<HostCommand>,
}

fn push_command(vm: &mut Vm, c: HostCommand) {
    if let Some(state) = vm.host_state_mut::<HostState>() {
        state.commands.push(c);
    }
}

fn take_commands(vm: &mut Vm) -> Vec<HostCommand> {
    match vm.host_state_mut::<HostState>() {
        Some(state) => std::mem::take(&mut state.commands),
        None => Vec::new(),
    }
}

/// Where the VM reads a `require` from: the asset directory, `scripts` under
/// it first. A file the OS cannot see is not visible to a script either — a
/// packed or remote asset source is not served here.
struct FileHost;

impl sabiruby::Host for FileHost {
    #[cfg(feature = "ruby-source")]
    fn compile(&mut self, src: &[u8], opts: &sabiruby::EvalOptions) -> Result<Vec<u8>, String> {
        let o = sabiruby_compiler::Options {
            filename: opts.filename.into(), debug_info: opts.debug_info, ..Default::default()
        };
        sabiruby_compiler::compile_eval(src, &o, opts.line, opts.scopes).map_err(|e| {
            match e.diagnostics.iter().find(|d| d.kind.is_error()) {
                Some(d) => d.message.clone(),
                None => String::from("compile error"),
            }
        })
    }
    #[cfg(not(feature = "ruby-source"))]
    fn compile(&mut self, _src: &[u8], _opts: &sabiruby::EvalOptions) -> Result<Vec<u8>, String> {
        Err(String::from("rubevy was built without the `ruby-source` feature: require a .mrb, or `eval` is unavailable"))
    }
    fn read_file(&mut self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(path).ok()
    }
    fn file_exists(&mut self, path: &str) -> bool {
        std::path::Path::new(path).is_file()
    }
}

/// Adds the VM, the `.mrb` asset loader and the three systems.
pub struct RubevyPlugin {
    /// Where a `require` reads from (Bevy's asset directory).
    pub asset_root: String,
}

impl Default for RubevyPlugin {
    fn default() -> Self {
        RubevyPlugin { asset_root: String::from("assets") }
    }
}

impl RubevyPlugin {
    pub fn with_asset_root(root: impl Into<String>) -> Self {
        RubevyPlugin { asset_root: root.into() }
    }
}

impl Plugin for RubevyPlugin {
    fn build(&self, app: &mut App) {
        let mut world = match ScriptWorld::new() {
            Ok(w) => w,
            Err(e) => panic!("rubevy: could not start the VM: {e}"),
        };
        world.vm.set_host(Box::new(FileHost));
        let root = &self.asset_root;
        world.vm.set_load_path(&[&format!("{root}/scripts"), root]);
        app.init_asset::<MrbAsset>()
            .init_asset_loader::<MrbLoader>()
            .add_message::<ScriptEnded>()
            .insert_resource(world)
            .add_systems(Update, (start_scripts, tick_scripts, drain_commands).chain());
    }
}

/// The instance variable each task carries its entity in (`Vm::ivar_set`), read back by the
/// natives through [`Vm::task_running`]. A script can see it — it is an ordinary `@ivar` — but
/// the name is not one a script would write by accident.
const ENTITY_IVAR: &str = "@rubevy_entity";

/// Turns every [`Script`] whose asset has arrived into a task.
fn start_scripts(
    mut commands: Commands,
    assets: Res<Assets<MrbAsset>>,
    mut world: ResMut<ScriptWorld>,
    pending: Query<(Entity, &Script), Without<ScriptTask>>,
) {
    for (entity, script) in &pending {
        let Some(asset) = assets.get(&script.source) else { continue };
        let name = script.name.clone().unwrap_or_else(|| format!("{entity}"));
        let vm = &mut world.vm;
        let irep = match vm.load(&asset.bytes) {
            Ok(i) => i,
            Err(e) => {
                error!("rubevy: {name} failed to load: {}", vm.describe_error(&e));
                continue;
            }
        };
        match vm.task_spawn(irep, script.priority, Some(&name)) {
            Ok(task) => {
                // the entity holds the task, so the collector must not take it
                vm.gc_register(task);
                // the task carries its entity, which is what `Rubevy.entity` answers
                vm.ivar_set(task, ENTITY_IVAR, Value::Int(entity.to_bits() as i64));
                commands.entity(entity).insert(ScriptTask { task });
            }
            Err(e) => error!("rubevy: {name} failed to start: {}", vm.describe_error(&e)),
        }
    }
}

/// Moves the scheduler's clock on by the frame time and runs the ready tasks
/// for up to the frame's budget.
fn tick_scripts(
    time: Res<Time>,
    frame: Res<FrameCount>,
    mut world: ResMut<ScriptWorld>,
    tasks: Query<(Entity, &ScriptTask), Without<ScriptDone>>,
    mut ended: MessageWriter<ScriptEnded>,
    mut commands: Commands,
) {
    let delta = time.delta_secs();
    let elapsed = time.elapsed_secs();
    let frame_no = frame.0;
    let world = &mut *world;

    // frame time in ticks, keeping what did not make a whole one for next frame
    let unit_ms = world.vm.task_tick_unit_ms() as f32;
    world.tick_remainder += delta * 1000.0 / unit_ms;
    let whole = world.tick_remainder.floor().max(0.0);
    world.tick_remainder -= whole;
    if whole >= 1.0 {
        world.vm.task_advance_ticks(whole as u32);
    }

    set_frame_state(&mut world.vm, frame_no, delta, elapsed);
    let limits = sabiruby::RunLimits {
        instructions: Some(world.budget),
        time_ns: world.frame_time.map(|d| d.as_nanos() as u64),
        overrun_ns: world.overrun.map(|d| d.as_nanos() as u64),
        ..Default::default()
    };
    if let Err(e) = world.vm.task_run_limits(limits) {
        // the scheduler itself failed, which a task's own exception never does
        let message = world.vm.describe_error(&e);
        error!("rubevy: the scheduler raised: {message}");
    }
    flush_output(&mut world.vm);

    for (entity, st) in &tasks {
        if !world.vm.task_finished(st.task) {
            continue;
        }
        let value = world.vm.task_value(st.task);
        let status = if world.vm.is_exception(value) { ScriptStatus::Failed } else { ScriptStatus::Finished };
        let text = world.vm.inspect_str(value).unwrap_or_else(|_| String::from("?"));
        ended.write(ScriptEnded { entity, status, value: text });
        world.vm.gc_unregister(st.task);
        commands.entity(entity).insert(ScriptDone);
    }
}

/// `$rubevy`, refreshed at the head of every frame.
fn set_frame_state(vm: &mut Vm, frame: u32, delta: f32, elapsed: f32) {
    let h = vm.hash_new();
    for (key, value) in [
        ("frame", Value::Int(frame as i64)),
        ("delta", Value::Float(delta as f64)),
        ("time", Value::Float(elapsed as f64)),
    ] {
        let k = Value::Sym(vm.intern(key));
        let _ = vm.hash_set(h, k, value);
    }
    vm.global_set("$rubevy", h);
}

fn flush_output(vm: &mut Vm) {
    let out = vm.take_output();
    if out.is_empty() {
        return;
    }
    for line in String::from_utf8_lossy(&out).lines() {
        info!("[script] {line}");
    }
}

/// Carries out what the scripts asked for this frame.
fn drain_commands(
    mut commands: Commands,
    mut transforms: Query<&mut Transform>,
    mut world: ResMut<ScriptWorld>,
) {
    for c in take_commands(&mut world.vm) {
        match c {
            HostCommand::Ask { entity, kind, args, queue } => {
                // parked scripts wait here until a system of the game answers
                // (`ScriptWorld::take_requests` / `answer`)
                world.requests.push(Request { entity: entity_from_bits(entity), kind, args, queue });
            }
            HostCommand::Log(text) => info!("[script] {text}"),
            HostCommand::Spawn { name, x, y, z } => {
                commands.spawn((SpawnedByScript { name }, Transform::from_xyz(x, y, z)));
            }
            HostCommand::Despawn(bits) => {
                if let Some(e) = entity_from_bits(bits) {
                    commands.entity(e).despawn();
                }
            }
            HostCommand::SetPosition { entity, x, y, z } => {
                if let Some(e) = entity_from_bits(entity) {
                    if let Ok(mut t) = transforms.get_mut(e) {
                        t.translation = Vec3::new(x, y, z);
                    }
                }
            }
        }
    }
}

fn entity_from_bits(bits: u64) -> Option<Entity> {
    Entity::try_from_bits(bits)
}

// ------------------------------------------------------------------ the Ruby side

/// What a `Rubevy::Entity` object is, in the `tag` of [`Vm::data_new`]. A host that gives its
/// scripts Data objects of its own picks other numbers.
pub const ENTITY_TAG: u32 = 1;

/// Defines `Rubevy::Entity`, the class of the object a script holds an entity in, and answers it.
///
/// It is a Data object (`Vm::data_new`): the VM carries `Entity::to_bits` and never reads
/// through it. `==`, `eql?` and `hash` go by that handle, so two objects naming the same entity
/// are equal and are one key in a Hash, while `dup` and `clone` refuse — which is the point of
/// the kind. What is defined here is `to_i` (the bits, for a script or a game that wants the
/// number) and `inspect`/`to_s`, so that `p entity` says which entity it is.
fn define_entity_class(vm: &mut Vm, module: ObjId) -> ObjId {
    // There is no `define_class_under` on the VM, so the class is made and named the way Ruby
    // does it: `Class.new(Object)` and `Rubevy.const_set(:Entity, it)`, which also gives the
    // anonymous class its name and its outer module (`Rubevy::Entity`).
    let new = vm.intern("new");
    let object = Value::Obj(vm.core.object);
    let class = Value::Obj(vm.core.class);
    let entity = vm.funcall(class, new, &[object], Value::Nil).expect("Class.new");
    let const_set = vm.intern("const_set");
    let name = Value::Sym(vm.intern("Entity"));
    vm.funcall(Value::Obj(module), const_set, &[name, entity], Value::Nil).expect("Rubevy::Entity");
    let entity = entity.obj().expect("a class is an object");
    vm.define_fn(entity, "to_i", |this: This<DataRef>| this.handle as i64);
    vm.define_fn(entity, "inspect", |this: This<DataRef>| entity_inspect(this.handle));
    vm.define_fn(entity, "to_s", |this: This<DataRef>| entity_inspect(this.handle));
    entity
}

fn entity_inspect(handle: u64) -> String {
    match entity_from_bits(handle) {
        Some(e) => format!("#<Rubevy::Entity {e}>"),
        None => format!("#<Rubevy::Entity bits={handle}>"),
    }
}

/// The entity an argument names: either a `Rubevy::Entity` object, or the Integer of
/// `Entity::to_bits` that the host API took before there was one (`Rubevy.despawn 12884901888`),
/// which still works.
fn entity_arg(vm: &mut Vm, v: Option<&Value>) -> Result<u64, VmError> {
    match v {
        Some(v) => match vm.data_of(*v) {
            Some((ENTITY_TAG, handle)) => Ok(handle),
            _ => Ok(vm.expect_int(*v, "entity")? as u64),
        },
        None => Ok(0),
    }
}

/// Defines the `Rubevy` module and its methods, and answers the `Rubevy::Entity` class.
///
/// The methods are closures ([`Vm::define_closure`]) rather than bare function pointers: they
/// carry the entity class, which is what lets `Rubevy.entity` and `Rubevy.ask` build and read
/// entity objects. What they may not carry is the game — a native gets `&mut Vm` and nothing
/// else — so everything that touches the world is left on the queue in the VM's host state
/// ([`HostState`]) for [`drain_commands`].
fn install_host_api(vm: &mut Vm) -> ObjId {
    let m = vm.define_module("Rubevy");
    let entity_class = define_entity_class(vm, m);
    let sc = vm.singleton_class(Value::Obj(m)).expect("Rubevy singleton");
    vm.define_closure(sc, "log", |vm, _s, a, _b| {
        let text = match a.first() {
            Some(v) => String::from_utf8_lossy(&vm.as_string(*v)?).into_owned(),
            None => String::new(),
        };
        push_command(vm, HostCommand::Log(text));
        Ok(Value::Nil)
    });
    vm.define_closure(sc, "spawn", |vm, _s, a, _b| {
        let name = match a.first() {
            Some(v) => String::from_utf8_lossy(&vm.as_string(*v)?).into_owned(),
            None => String::new(),
        };
        let (x, y, z) = (num(vm, a.get(1)), num(vm, a.get(2)), num(vm, a.get(3)));
        push_command(vm, HostCommand::Spawn { name, x, y, z });
        Ok(Value::Nil)
    });
    vm.define_closure(sc, "despawn", |vm, _s, a, _b| {
        let bits = entity_arg(vm, a.first())?;
        push_command(vm, HostCommand::Despawn(bits));
        Ok(Value::Nil)
    });
    // the entity this script is attached to, as a `Rubevy::Entity`. A fresh object each call:
    // two of them for the same entity are `==` and hash alike, so a script cannot tell, and the
    // ones it drops are what the free hook sees
    vm.define_closure(sc, "entity", move |vm, _s, _a, _b| {
        Ok(match current_entity(vm) {
            Value::Int(bits) => vm.data_new(entity_class, ENTITY_TAG, bits as u64),
            _ => Value::Nil,
        })
    });
    vm.define_closure(sc, "move_to", |vm, _s, a, _b| {
        // the entity the script is attached to, which is the common case
        let Value::Int(bits) = current_entity(vm) else { return Ok(Value::Nil) };
        let (x, y, z) = (num(vm, a.first()), num(vm, a.get(1)), num(vm, a.get(2)));
        push_command(vm, HostCommand::SetPosition { entity: bits as u64, x, y, z });
        Ok(Value::Nil)
    });
    // `Rubevy.ask("scan", 40)` — the game answers it, this frame or a later one, and the script
    // waits on the queue meanwhile (its task is parked, so it costs nothing)
    vm.define_closure(sc, "ask", |vm, _s, a, _b| {
        let kind = match a.first() {
            Some(v) => String::from_utf8_lossy(&vm.as_string(*v)?).into_owned(),
            None => return Err(vm.raise_arg("ask needs what to ask for")),
        };
        let mut args: Vec<Arg> = Vec::with_capacity(a.len().saturating_sub(1));
        for v in &a[1..] {
            // an entity the script was given keeps its identity across the boundary; everything
            // else is a number or a string, as before
            let as_entity = match vm.data_of(*v) {
                Some((ENTITY_TAG, handle)) => entity_from_bits(handle),
                _ => None,
            };
            if let Some(e) = as_entity {
                args.push(Arg::Entity(e));
                continue;
            }
            args.push(match v {
                Value::Int(i) => Arg::Num(*i as f64),
                Value::Float(f) => Arg::Num(*f),
                Value::Sym(s) => Arg::Text(vm.sym_name(*s).to_string()),
                other => match vm.as_string(*other) {
                    Ok(bytes) => Arg::Text(String::from_utf8_lossy(&bytes).into_owned()),
                    Err(_) => Arg::Num(0.0),
                },
            });
        }
        let queue = vm.task_queue_new()?;
        vm.gc_register(queue);
        let entity = match current_entity(vm) { Value::Int(bits) => bits as u64, _ => u64::MAX };
        push_command(vm, HostCommand::Ask { entity, kind, args, queue });
        Ok(Value::Obj(queue))
    });
    vm.define_closure(sc, "set_position", |vm, _s, a, _b| {
        let bits = entity_arg(vm, a.first())?;
        let (x, y, z) = (num(vm, a.get(1)), num(vm, a.get(2)), num(vm, a.get(3)));
        push_command(vm, HostCommand::SetPosition { entity: bits, x, y, z });
        Ok(Value::Nil)
    });
    entity_class
}

/// The entity of the task the scheduler is running, as `Entity::to_bits`.
fn current_entity(vm: &Vm) -> Value {
    match vm.task_running() {
        Some(task) => vm.ivar_get(task, ENTITY_IVAR),
        None => Value::Nil,
    }
}

fn num(vm: &Vm, v: Option<&Value>) -> f32 {
    let _ = vm;
    match v {
        Some(Value::Int(i)) => *i as f32,
        Some(Value::Float(f)) => *f as f32,
        _ => 0.0,
    }
}

/// So the type is named in the public API even where nothing else uses it.
pub type ScriptVmError = VmError;

/// The entity ids the plugin hands to Ruby are `Entity::to_bits`; this is the
/// way back, for a host that wants to read what a script asked about.
pub fn entity_of(bits: u64) -> Option<Entity> {
    entity_from_bits(bits)
}

/// What a script asked for this frame and has not had carried out yet, for tests: the queue is
/// drained by [`drain_commands`], so this is only useful before that system runs.
#[doc(hidden)]
pub fn pending_command_count(world: &ScriptWorld) -> usize {
    world.vm.host_state::<HostState>().map(|s| s.commands.len()).unwrap_or(0)
}
