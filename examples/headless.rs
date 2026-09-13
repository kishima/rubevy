//! Two scripts as tasks in one VM, without a window. One sleeps between its
//! steps, the other runs at a lower priority; the app exits when both are done.
//!
//!     cargo run --example headless

use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::asset::AssetPlugin;
use bevy::log::LogPlugin;
use bevy::prelude::*;
use rubevy::{MrbAsset, RubevyPlugin, Script, ScriptEnded, SpawnedByScript};

#[derive(Resource, Default)]
struct Ended(usize);

fn main() {
    App::new()
        .add_plugins((
            MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::from_millis(16))),
            LogPlugin::default(),
            AssetPlugin::default(),
            RubevyPlugin::default(),
        ))
        .init_resource::<Ended>()
        .add_systems(Startup, spawn_scripts)
        .add_systems(Update, (report_spawned, report_moved, exit_when_done))
        .run();
}

fn spawn_scripts(mut commands: Commands, server: Res<AssetServer>) {
    let hello: Handle<MrbAsset> = server.load("scripts/hello.mrb");
    let ticker: Handle<MrbAsset> = server.load("scripts/ticker.mrb");
    // lower number, higher priority: `worker` gets the CPU first each time both are ready.
    // The Transform is what `Rubevy.move_to` writes.
    commands.spawn((Script::new(hello).with_name("worker").with_priority(50), Transform::default()));
    commands.spawn(Script::new(ticker).with_name("ticker").with_priority(200));
}

/// What `Rubevy.spawn` left behind, once `drain_commands` has run.
fn report_spawned(spawned: Query<(Entity, &SpawnedByScript, &Transform), Added<SpawnedByScript>>) {
    for (entity, s, t) in &spawned {
        info!("host: a script spawned {:?} named {} at {:?}", entity, s.name, t.translation);
    }
}

fn report_moved(moved: Query<(Entity, &Transform), (With<rubevy::Script>, Changed<Transform>)>) {
    for (entity, t) in &moved {
        info!("host: a script moved its own {:?} to {:?}", entity, t.translation);
    }
}

fn exit_when_done(
    mut ended: MessageReader<ScriptEnded>,
    mut count: ResMut<Ended>,
    mut exit: MessageWriter<AppExit>,
) {
    for e in ended.read() {
        info!("host: script on {:?} ended: {:?} {}", e.entity, e.status, e.value);
        count.0 += 1;
        if count.0 >= 2 {
            exit.write(AppExit::Success);
        }
    }
}
