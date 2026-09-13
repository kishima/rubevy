//! `Rubevy.ask`: a script asks the game for something and is parked until the game answers.
//! Here the "sensor" answers two frames later, and a second script keeps running meanwhile —
//! which is the point: waiting costs nothing and blocks nobody.
//!
//!     cargo run --example sensor

use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::asset::AssetPlugin;
use bevy::diagnostic::FrameCount;
use bevy::log::LogPlugin;
use bevy::prelude::*;
use rubevy::{Answer, MrbAsset, Request, RubevyPlugin, Script, ScriptEnded, ScriptWorld};

/// Requests the game has taken but not answered yet, with the frame they may be answered on.
#[derive(Resource, Default)]
struct Pending(Vec<(Request, u32)>);

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
        .init_resource::<Pending>()
        .init_resource::<Ended>()
        .add_systems(Startup, spawn_scripts)
        .add_systems(Update, (answer_requests, report_ended).chain())
        .run();
}

fn spawn_scripts(mut commands: Commands, server: Res<AssetServer>) {
    let sensor: Handle<MrbAsset> = server.load("scripts/sensor.mrb");
    let ticker: Handle<MrbAsset> = server.load("scripts/ticker.mrb");
    commands.spawn((Script::new(sensor).with_name("sensor").with_priority(50), Transform::default()));
    commands.spawn(Script::new(ticker).with_name("ticker").with_priority(200));
}

/// The game's side of `Rubevy.ask`: take what the scripts asked, and answer it when the answer
/// is ready — here two frames later, to show that a request may outlive the frame it was made on.
fn answer_requests(mut world: ResMut<ScriptWorld>, mut pending: ResMut<Pending>, frame: Res<FrameCount>) {
    let now = frame.0;
    for request in world.take_requests() {
        info!("host: {} asked for {:?} ({:?})", request.kind, request.args, request.entity);
        pending.0.push((request, now + 2));
    }
    let (ready, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut pending.0).into_iter().partition(|(_, at)| *at <= now);
    pending.0 = waiting;
    for (request, _) in ready {
        let answer = match request.kind.as_str() {
            // a sensor that finds something, except on the third round
            "scan" if request.num_or(0, 0.0) > 1.0 => {
                Answer::List(vec![now as f64 % 7.0, 3.5])
            }
            _ => Answer::Nil,
        };
        info!("host: answering {} with {:?}", request.kind, answer);
        world.answer(&request, answer);
    }
}

fn report_ended(mut ended: MessageReader<ScriptEnded>, mut done: ResMut<Ended>, mut exit: MessageWriter<AppExit>) {
    for e in ended.read() {
        info!("host: script on {:?} ended: {:?} {}", e.entity, e.status, e.value);
        done.0 += 1;
        if done.0 >= 2 {
            exit.write(AppExit::Success);
        }
    }
}
