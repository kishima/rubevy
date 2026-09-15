//! `ScriptWorld::answer_with`: the game answers a script's question from a future, worked out
//! on Bevy's task pool. The "path search" takes longer than a frame, so the script is parked
//! for several of them while the ticker beside it keeps running — which is the point.
//!
//!     cargo run --example async

use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::asset::AssetPlugin;
use bevy::diagnostic::FrameCount;
use bevy::log::LogPlugin;
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use rubevy::{Answer, MrbAsset, RubevyPlugin, Script, ScriptEnded, ScriptWorld};

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
        .add_systems(Update, (answer_requests, report_ended).chain())
        .run();
}

fn spawn_scripts(mut commands: Commands, server: Res<AssetServer>) {
    let script: Handle<MrbAsset> = server.load("scripts/async.mrb");
    let ticker: Handle<MrbAsset> = server.load("scripts/ticker.mrb");
    commands.spawn((Script::new(script).with_name("async").with_priority(50), Transform::default()));
    commands.spawn(Script::new(ticker).with_name("ticker").with_priority(200));
}

/// The game's side. `answer_with` takes the request as it comes out of `take_requests` and a
/// future; the plugin answers the script on the frame the future finishes.
fn answer_requests(mut world: ResMut<ScriptWorld>, frame: Res<FrameCount>) {
    let now = frame.0;
    for request in world.take_requests() {
        match request.kind.as_str() {
            "path" => {
                let (x, y) = (request.num_or(0, 0.0), request.num_or(1, 0.0));
                info!("host: frame {now}: a path to ({x}, {y}) — handing it to the task pool");
                world.answer_with(request, walk(x, y));
            }
            other => {
                info!("host: frame {now}: nothing to say about {other}");
                world.answer(&request, Answer::Nil);
            }
        }
    }
}

/// Stands in for work a game cannot do inside a frame: a path search over a big map, a file, a
/// query. It is an ordinary `async fn` — the sleep is what a real search would spend thinking —
/// and it yields between its slices so it is a good citizen of the pool.
async fn walk(x: f64, y: f64) -> Answer {
    let mut steps: Vec<Vec<f64>> = Vec::new();
    for i in 1..=8 {
        std::thread::sleep(Duration::from_millis(10));
        let t = i as f64 / 8.0;
        steps.push(vec![x * t, y * t]);
        future::yield_now().await;
    }
    Answer::Rows(steps)
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
