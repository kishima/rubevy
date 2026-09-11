//! Runs `assets/scripts/hello.mrb` without a window and exits when it finishes.
//!
//!     cargo run --example headless

use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::asset::AssetPlugin;
use bevy::log::LogPlugin;
use bevy::prelude::*;
use rubevy::{MrbAsset, RubevyPlugin, Script, ScriptEnded};

fn main() {
    App::new()
        .add_plugins((
            MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::from_millis(16))),
            LogPlugin::default(),
            AssetPlugin::default(),
            RubevyPlugin,
        ))
        .add_systems(Startup, spawn_script)
        .add_systems(Update, exit_when_done)
        .run();
}

fn spawn_script(mut commands: Commands, server: Res<AssetServer>) {
    let mrb: Handle<MrbAsset> = server.load("scripts/hello.mrb");
    commands.spawn(Script::new(mrb).with_budget(300));
}

fn exit_when_done(mut ended: MessageReader<ScriptEnded>, mut exit: MessageWriter<AppExit>) {
    for e in ended.read() {
        info!("script {:?} ended: {:?} {}", e.entity, e.status, e.message);
        exit.write(AppExit::Success);
    }
}
