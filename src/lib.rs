//! rubevy: run mruby bytecode inside Bevy, using the [SabiRuby](sabiruby) VM.
//!
//! v0 scope:
//! * `.mrb` files (compiled with mruby's `mrbc`) are loaded as [`MrbAsset`]s.
//! * A [`Script`] component starts a fresh VM per entity once its asset is
//!   loaded and steps it every frame with an instruction budget
//!   ([`Script::budget`]), so a script that loops forever cannot stall a frame.
//! * The script sees the host through globals refreshed before each step:
//!   `$frame` (frame count) and `$delta` (seconds since the last frame).
//!   `puts`/`p` output is forwarded to Bevy's log.
//!
//! There is no host-call API yet (no `Rubevy.spawn` etc.); see the design
//! notes in the book repository (`docs/notes/rubevy-design.md`).

use bevy::asset::{io::Reader, Asset, AssetApp, AssetLoader, LoadContext};
use bevy::diagnostic::FrameCount;
use bevy::prelude::*;
use bevy::reflect::TypePath;

use sabiruby::{Step, Vm, VmError};

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

/// Attach to an entity to run a script. The VM is created when the asset is
/// available and lives in [`ScriptVm`] on the same entity.
#[derive(Component, Debug, Clone)]
pub struct Script {
    pub source: Handle<MrbAsset>,
    /// Instructions executed per frame before the script is suspended.
    pub budget: u64,
}

impl Script {
    pub fn new(source: Handle<MrbAsset>) -> Self {
        Script { source, budget: 20_000 }
    }
    pub fn with_budget(mut self, budget: u64) -> Self {
        self.budget = budget;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptStatus {
    Running,
    Finished,
    Failed,
}

/// The live VM of a [`Script`] (one interpreter per entity, no sharing).
#[derive(Component)]
pub struct ScriptVm {
    pub vm: Vm,
    pub status: ScriptStatus,
    /// Instructions executed so far.
    pub instructions: u64,
}

/// Emitted when a script finishes or fails.
#[derive(Message, Debug, Clone)]
pub struct ScriptEnded {
    pub entity: Entity,
    pub status: ScriptStatus,
    pub message: String,
}

pub struct RubevyPlugin;

impl Plugin for RubevyPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<MrbAsset>()
            .init_asset_loader::<MrbLoader>()
            .add_message::<ScriptEnded>()
            .add_systems(Update, (start_scripts, step_scripts).chain());
    }
}

/// Creates the VM for every [`Script`] whose asset has arrived.
fn start_scripts(
    mut commands: Commands,
    assets: Res<Assets<MrbAsset>>,
    pending: Query<(Entity, &Script), Without<ScriptVm>>,
    mut ended: MessageWriter<ScriptEnded>,
) {
    for (entity, script) in &pending {
        let Some(asset) = assets.get(&script.source) else { continue };
        match boot(&asset.bytes) {
            Ok(vm) => {
                commands.entity(entity).insert(ScriptVm { vm, status: ScriptStatus::Running, instructions: 0 });
            }
            Err(message) => {
                error!("rubevy: script on {entity:?} failed to start: {message}");
                commands.entity(entity).insert(ScriptVm { vm: Vm::new(), status: ScriptStatus::Failed, instructions: 0 });
                ended.write(ScriptEnded { entity, status: ScriptStatus::Failed, message });
            }
        }
    }
}

fn boot(bytes: &[u8]) -> Result<Vm, String> {
    let mut vm = Vm::with_mrblib().map_err(|e| describe(&mut Vm::new(), &e))?;
    let irep = vm.load(bytes).map_err(|e| describe(&mut vm, &e))?;
    vm.start(irep);
    Ok(vm)
}

fn describe(vm: &mut Vm, e: &VmError) -> String {
    vm.describe_error(e)
}

/// Runs every live script for up to its budget, forwarding output to the log.
fn step_scripts(
    time: Res<Time>,
    frame: Res<FrameCount>,
    mut scripts: Query<(Entity, &Script, &mut ScriptVm)>,
    mut ended: MessageWriter<ScriptEnded>,
) {
    for (entity, script, mut state) in &mut scripts {
        if state.status != ScriptStatus::Running {
            continue;
        }
        let before = state.vm.instructions;
        set_host_globals(&mut state.vm, frame.0, time.delta_secs());
        let result = state.vm.step(script.budget);
        state.instructions += state.vm.instructions - before;
        flush_output(&mut state.vm, entity);
        match result {
            Ok(Step::Paused) => {}
            Ok(Step::Finished(_)) => {
                state.status = ScriptStatus::Finished;
                ended.write(ScriptEnded { entity, status: ScriptStatus::Finished, message: String::new() });
            }
            Err(e) => {
                let message = state.vm.describe_error(&e);
                error!("rubevy: script on {entity:?} raised: {message}");
                state.status = ScriptStatus::Failed;
                ended.write(ScriptEnded { entity, status: ScriptStatus::Failed, message });
            }
        }
    }
}

fn set_host_globals(vm: &mut Vm, frame: u32, delta: f32) {
    let f = vm.intern("$frame");
    let d = vm.intern("$delta");
    vm.globals.insert(f, sabiruby::value::Slot::from(sabiruby::Value::Int(frame as i64)));
    vm.globals.insert(d, sabiruby::value::Slot::from(sabiruby::Value::Float(delta as f64)));
}

fn flush_output(vm: &mut Vm, entity: Entity) {
    let out = vm.take_output();
    if out.is_empty() {
        return;
    }
    for line in String::from_utf8_lossy(&out).lines() {
        info!("[{entity:?}] {line}");
    }
}
