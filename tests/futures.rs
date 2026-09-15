//! `ScriptWorld::answer_with`: a request answered from a future is answered frames later, the
//! script that asked is parked for all of them, and the other script in the same VM keeps
//! running. The value the future produced is what `Rubevy.ask(...).pop` returns.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;
use rubevy::{Answer, MrbAsset, RubevyPlugin, Script, ScriptWorld};

/// Requests seen per kind, so far, and what the scripts sent back (`tests/replace.rs`'s way of
/// counting: the host is the only thing that can see what a script did).
#[derive(Resource, Default)]
struct Seen {
    slow: usize,
    busy: usize,
    got: Vec<f64>,
}

/// A one-shot the test fires by hand. A future that waits on it is not done until then on any
/// build — single-threaded pool included — so "the script is still parked" is a fact here and
/// not a race against a sleep.
#[derive(Resource, Clone, Default)]
struct Gate(Arc<Mutex<(bool, Option<Waker>)>>);

impl Gate {
    fn fire(&self) {
        let waker = {
            let mut state = self.0.lock().expect("gate");
            state.0 = true;
            state.1.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl Future for Gate {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut state = self.0.lock().expect("gate");
        if state.0 {
            Poll::Ready(())
        } else {
            state.1 = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

fn compile(src: &str) -> MrbAsset {
    let opts = sabiruby_compiler::Options { filename: "t.rb".into(), ..Default::default() };
    MrbAsset { bytes: sabiruby_compiler::compile(src.as_bytes(), &opts).expect("compiles") }
}

/// `slow` is answered from a future that waits for the gate, `now` from one that is ready at
/// once; the rest are answered on the spot, as a system always could.
fn answer_all(mut world: ResMut<ScriptWorld>, mut seen: ResMut<Seen>, gate: Res<Gate>) {
    for r in world.take_requests() {
        match r.kind.as_str() {
            "slow" => {
                seen.slow += 1;
                let gate = gate.clone();
                world.answer_with(r, async move {
                    gate.await;
                    Answer::Num(7.0)
                });
            }
            "now" => world.answer_with(r, async { Answer::Num(7.0) }),
            "busy" => {
                seen.busy += 1;
                world.answer(&r, Answer::Nil);
            }
            "got" => {
                seen.got.push(r.num_or(0, -1.0));
                world.answer(&r, Answer::Nil);
            }
            _ => world.answer(&r, Answer::Nil),
        }
    }
}

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins.set(ScheduleRunnerPlugin::run_once()),
        bevy::asset::AssetPlugin::default(),
        RubevyPlugin::default(),
    ))
    .init_resource::<Seen>()
    .init_resource::<Gate>()
    .add_systems(Update, answer_all);
    app
}

fn frames(app: &mut App, n: usize) {
    for _ in 0..n {
        std::thread::sleep(Duration::from_millis(2));
        app.update();
    }
}

#[test]
fn a_script_waits_for_its_future_and_the_others_do_not() {
    let mut app = app();
    let (slow, busy) = {
        let mut assets = app.world_mut().resource_mut::<Assets<MrbAsset>>();
        (
            assets.add(compile("loop { Rubevy.ask('slow').pop }")),
            assets.add(compile("loop { Rubevy.ask('busy').pop }")),
        )
    };
    app.world_mut().spawn(Script::new(slow));
    app.world_mut().spawn(Script::new(busy));

    frames(&mut app, 10);
    {
        let seen = app.world().resource::<Seen>();
        assert_eq!(seen.slow, 1, "the script asked once and is parked on the unfinished future");
        assert!(seen.busy > 1, "the other script keeps running: {} rounds", seen.busy);
    }
    assert_eq!(app.world().resource::<ScriptWorld>().answering(), 1, "the future is still out");

    app.world().resource::<Gate>().fire();
    frames(&mut app, 5);
    let seen = app.world().resource::<Seen>();
    assert!(seen.slow > 1, "the answer arrived and the script went round again");
}

#[test]
fn the_value_a_future_answers_with_reaches_the_script() {
    let mut app = app();
    // what `pop` returns is what the future produced, and the script sends it back
    let script = app
        .world_mut()
        .resource_mut::<Assets<MrbAsset>>()
        .add(compile("v = Rubevy.ask('now').pop\nRubevy.ask('got', v)\n:done"));
    app.world_mut().spawn(Script::new(script));
    frames(&mut app, 10);
    assert_eq!(app.world().resource::<Seen>().got, vec![7.0]);
}
