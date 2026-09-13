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
//! * `Rubevy.entity` — the entity this script is attached to, as an Integer
//!   (`Entity::to_bits`), read off the task the scheduler is running.
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

use sabiruby::value::{ObjId, Slot};
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
}

impl Arg {
    pub fn as_num(&self) -> Option<f64> {
        match self {
            Arg::Num(n) => Some(*n),
            Arg::Text(_) => None,
        }
    }
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Arg::Text(t) => Some(t),
            Arg::Num(_) => None,
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
    /// Ticks not yet handed to the scheduler (frame times shorter than a tick).
    tick_remainder: f32,
    /// What scripts asked the game for and are waiting on (`Rubevy.ask`).
    requests: Vec<Request>,
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
        install_host_api(&mut vm);
        Ok(ScriptWorld { vm, budget: 200_000, tick_remainder: 0.0, requests: Vec::new() })
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

fn enable_scheduler_gc(vm: &mut Vm) -> Result<(), String> {
    let gc = vm.intern("GC");
    let Some(gc) = vm.const_get(vm.core.object, gc) else { return Ok(()) };
    let m = vm.intern("scheduler_driven=");
    vm.funcall(gc, m, &[Value::True], Value::Nil)
        .map(|_| ())
        .map_err(|e| vm.describe_error(&e))
}

/// The queue a native writes and a system reads. It lives outside the VM
/// because a native gets `&mut Vm` and nothing else.
static COMMANDS: std::sync::Mutex<Vec<HostCommand>> = std::sync::Mutex::new(Vec::new());

fn push_command(c: HostCommand) {
    if let Ok(mut q) = COMMANDS.lock() {
        q.push(c);
    }
}

fn take_commands() -> Vec<HostCommand> {
    match COMMANDS.lock() {
        Ok(mut q) => std::mem::take(&mut *q),
        Err(_) => Vec::new(),
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
                let k = vm.intern("@rubevy_entity");
                vm.heap.ivar_set(task, k, Value::Int(entity.to_bits() as i64));
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
    let budget = world.budget;
    if let Err(e) = world.vm.task_run_budget(budget) {
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
        let status = if is_exception(&world.vm, value) { ScriptStatus::Failed } else { ScriptStatus::Finished };
        let text = world.vm.inspect_str(value).unwrap_or_else(|_| String::from("?"));
        ended.write(ScriptEnded { entity, status, value: text });
        world.vm.gc_unregister(st.task);
        commands.entity(entity).insert(ScriptDone);
    }
}

fn is_exception(vm: &Vm, v: Value) -> bool {
    v.obj().map(|o| matches!(vm.heap.get(o).kind, sabiruby::object::ObjKind::Exception)).unwrap_or(false)
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
    let n = vm.intern("$rubevy");
    vm.globals.insert(n, Slot::from(h));
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
    for c in take_commands() {
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

fn install_host_api(vm: &mut Vm) {
    let m = vm.define_module("Rubevy");
    let sc = vm.singleton_class(Value::Obj(m)).expect("Rubevy singleton");
    vm.define_method(sc, "log", |vm, _s, a, _b| {
        let text = match a.first() {
            Some(v) => String::from_utf8_lossy(&vm.as_string(*v)?).into_owned(),
            None => String::new(),
        };
        push_command(HostCommand::Log(text));
        Ok(Value::Nil)
    });
    vm.define_method(sc, "spawn", |vm, _s, a, _b| {
        let name = match a.first() {
            Some(v) => String::from_utf8_lossy(&vm.as_string(*v)?).into_owned(),
            None => String::new(),
        };
        let (x, y, z) = (num(vm, a.get(1)), num(vm, a.get(2)), num(vm, a.get(3)));
        push_command(HostCommand::Spawn { name, x, y, z });
        Ok(Value::Nil)
    });
    vm.define_method(sc, "despawn", |vm, _s, a, _b| {
        let bits = a.first().map(|v| vm.expect_int(*v, "entity")).transpose()?.unwrap_or(0);
        push_command(HostCommand::Despawn(bits as u64));
        Ok(Value::Nil)
    });
    vm.define_method(sc, "entity", |vm, _s, _a, _b| Ok(current_entity(vm)));
    vm.define_method(sc, "move_to", |vm, _s, a, _b| {
        // the entity the script is attached to, which is the common case
        let Value::Int(bits) = current_entity(vm) else { return Ok(Value::Nil) };
        let (x, y, z) = (num(vm, a.first()), num(vm, a.get(1)), num(vm, a.get(2)));
        push_command(HostCommand::SetPosition { entity: bits as u64, x, y, z });
        Ok(Value::Nil)
    });
    // `Rubevy.ask("scan", 40)` — the game answers it, this frame or a later one, and the script
    // waits on the queue meanwhile (its task is parked, so it costs nothing)
    vm.define_method(sc, "ask", |vm, _s, a, _b| {
        let kind = match a.first() {
            Some(v) => String::from_utf8_lossy(&vm.as_string(*v)?).into_owned(),
            None => return Err(vm.raise_arg("ask needs what to ask for")),
        };
        let mut args: Vec<Arg> = Vec::with_capacity(a.len().saturating_sub(1));
        for v in &a[1..] {
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
        push_command(HostCommand::Ask { entity, kind, args, queue });
        Ok(Value::Obj(queue))
    });
    vm.define_method(sc, "set_position", |vm, _s, a, _b| {
        let bits = a.first().map(|v| vm.expect_int(*v, "entity")).transpose()?.unwrap_or(0) as u64;
        let (x, y, z) = (num(vm, a.get(1)), num(vm, a.get(2)), num(vm, a.get(3)));
        push_command(HostCommand::SetPosition { entity: bits, x, y, z });
        Ok(Value::Nil)
    });
}

/// The entity of the task the scheduler is running, as `Entity::to_bits`.
fn current_entity(vm: &mut Vm) -> Value {
    let Some(task) = vm.task.running else { return Value::Nil };
    let k = vm.intern("@rubevy_entity");
    vm.heap.ivar_get(task, k)
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

/// What a script spawned this frame, for tests: the queue is drained by
/// [`drain_commands`], so this is only useful before that system runs.
#[doc(hidden)]
pub fn pending_command_count() -> usize {
    COMMANDS.lock().map(|q| q.len()).unwrap_or(0)
}
