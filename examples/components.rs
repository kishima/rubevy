//! Components by name: a script reads its own `Transform`, moves it, writes it back, and looks
//! for every entity that has the game's own `Waypoint`. There is no glue per component type —
//! Bevy's reflection says what a `Transform` is, and the type registry says which types a
//! script may see at all.
//!
//!     cargo run --example components

use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::asset::AssetPlugin;
use bevy::log::LogPlugin;
use bevy::prelude::*;
use rubevy::{MrbAsset, RubevyPlugin, Script, ScriptEnded};

/// A component of the game's own. What makes it reachable from Ruby is the derive, the
/// `#[reflect(Component)]` and the `register_type` below — nothing in rubevy names it.
#[derive(Component, Reflect, Debug, Default)]
#[reflect(Component)]
struct Waypoint {
    #[allow(dead_code)]
    index: u32,
}

fn main() {
    App::new()
        .add_plugins((
            MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::from_millis(16))),
            LogPlugin::default(),
            AssetPlugin::default(),
            TransformPlugin,
            RubevyPlugin::default(),
        ))
        // `TransformPlugin` propagates transforms but does not register the type, and this app
        // is not built with bevy's `reflect_auto_register`, so the types a script may touch are
        // named here. `DefaultPlugins` in a real game registers bevy's own.
        .register_type::<Transform>()
        .register_type::<Waypoint>()
        .add_systems(Startup, spawn)
        .add_systems(Update, report_ended)
        .run();
}

fn spawn(mut commands: Commands, server: Res<AssetServer>) {
    commands.spawn((Waypoint { index: 0 }, Transform::from_xyz(10.0, 0.0, 0.0)));
    commands.spawn((Waypoint { index: 1 }, Transform::from_xyz(0.0, 10.0, 0.0)));
    let script: Handle<MrbAsset> = server.load("scripts/components.mrb");
    commands.spawn((Script::new(script).with_name("components"), Transform::from_xyz(1.0, 2.0, 3.0)));
}

fn report_ended(
    mut ended: MessageReader<ScriptEnded>,
    transforms: Query<&Transform>,
    mut exit: MessageWriter<AppExit>,
) {
    for e in ended.read() {
        if let Ok(t) = transforms.get(e.entity) {
            info!("host: the script left its Transform at {:?}", t.translation);
        }
        info!("host: script on {:?} ended: {:?} {}", e.entity, e.status, e.value);
        exit.write(AppExit::Success);
    }
}
