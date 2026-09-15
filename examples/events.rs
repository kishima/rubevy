//! Events: a Bevy event reaches a script through one line a game writes — an observer that
//! calls `ScriptWorld::publish`. The script waits on a queue, in a task of its own, so a reflex
//! is not a callback that interrupts the brain but another task that happens to be ready.
//!
//!     cargo run --example events

use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::asset::AssetPlugin;
use bevy::diagnostic::FrameCount;
use bevy::log::LogPlugin;
use bevy::prelude::*;
use rubevy::{entity_object, Answer, MrbAsset, RubevyPlugin, Script, ScriptEnded, ScriptWorld};
use sabiruby::Value;

/// An event of the game's own, aimed at the entity that was hit.
#[derive(EntityEvent)]
struct Hit {
    entity: Entity,
    by: Entity,
    damage: f32,
}

/// Who is hitting whom, so the example has something to trigger.
#[derive(Resource)]
struct Fight {
    robot: Entity,
    attacker: Entity,
}

fn main() {
    App::new()
        .add_plugins((
            MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::from_millis(16))),
            LogPlugin::default(),
            AssetPlugin::default(),
            RubevyPlugin::default(),
        ))
        // the whole of the bridge between Bevy's events and Ruby: one observer per event type
        .add_observer(publish_hit)
        .add_systems(Startup, spawn)
        .add_systems(Update, (strike, ring, report_ended))
        .run();
}

/// A Bevy event becomes a message on the queues of the scripts on that entity. The payload is
/// built inside the VM so that `by` crosses as a `Rubevy::Entity` rather than as a number.
fn publish_hit(on: On<Hit>, mut scripts: ResMut<ScriptWorld>) {
    let (hurt, by, damage) = (on.entity, on.by, on.damage);
    let class = scripts.entity_class();
    scripts.publish_value(Some(hurt), "hit", move |vm| {
        let by = entity_object(vm, class, by);
        vm.ary_new(vec![by, Value::Float(damage as f64)])
    });
}

fn spawn(mut commands: Commands, server: Res<AssetServer>) {
    let script: Handle<MrbAsset> = server.load("scripts/events.mrb");
    let robot = commands.spawn(Script::new(script).with_name("events")).id();
    let attacker = commands.spawn_empty().id();
    commands.insert_resource(Fight { robot, attacker });
}

/// Hits the robot every so often.
fn strike(mut commands: Commands, frame: Res<FrameCount>, fight: Res<Fight>) {
    if frame.0 > 5 && frame.0.is_multiple_of(12) {
        let damage = (frame.0 % 7) as f32 + 1.0;
        commands.trigger(Hit { entity: fight.robot, by: fight.attacker, damage });
    }
}

/// And rings a bell for everyone, which is what `None` means.
fn ring(mut scripts: ResMut<ScriptWorld>, frame: Res<FrameCount>) {
    if frame.0 > 5 && frame.0.is_multiple_of(20) {
        scripts.publish(None, "bell", Answer::Num(frame.0 as f64));
    }
}

fn report_ended(mut ended: MessageReader<ScriptEnded>, mut exit: MessageWriter<AppExit>) {
    for e in ended.read() {
        info!("host: script on {:?} ended: {:?} {}", e.entity, e.status, e.value);
        exit.write(AppExit::Success);
    }
}
