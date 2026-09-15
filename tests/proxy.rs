//! `assets/scripts/proxy.rb`: a call on a `Rubevy::Proxy` reaches the game as `kind.name`, the
//! task is parked on the answer like any other `ask`, the value comes back as the value of the
//! call, and `respond_to?` says yes. The script `require`s the shipped asset, so this is the
//! file a game would load, not a copy of it.

use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;
use rubevy::{Answer, MrbAsset, RubevyPlugin, Script, ScriptWorld};

/// What the game was asked, in order, and what the scripts sent back.
#[derive(Resource, Default)]
struct Seen {
    asked: Vec<(String, Vec<f64>)>,
    reported: Vec<String>,
}

fn compile(src: &str) -> MrbAsset {
    let opts = sabiruby_compiler::Options { filename: "t.rb".into(), ..Default::default() };
    MrbAsset { bytes: sabiruby_compiler::compile(src.as_bytes(), &opts).expect("compiles") }
}

/// `robot.*` is the proxied object; `report` is how a script tells the test a string.
fn answer_all(mut world: ResMut<ScriptWorld>, mut seen: ResMut<Seen>) {
    for r in world.take_requests() {
        match r.kind.as_str() {
            "report" => {
                seen.reported.push(r.text(0).unwrap_or("").to_string());
                world.answer(&r, Answer::Nil);
            }
            kind if kind.starts_with("robot.") => {
                let args: Vec<f64> = (0..2).filter_map(|i| r.num(i)).collect();
                seen.asked.push((kind.to_string(), args));
                world.answer(&r, Answer::Num(seen.asked.len() as f64 * 10.0));
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
    .add_systems(Update, answer_all);
    app
}

fn frames(app: &mut App, n: usize) {
    for _ in 0..n {
        std::thread::sleep(Duration::from_millis(2));
        app.update();
    }
}

fn run(src: &str, n: usize) -> App {
    let mut app = app();
    let script = app.world_mut().resource_mut::<Assets<MrbAsset>>().add(compile(src));
    app.world_mut().spawn(Script::new(script));
    frames(&mut app, n);
    app
}

#[test]
fn a_call_on_a_proxy_reaches_the_game_as_kind_dot_name() {
    let app = run(
        r#"require "proxy"
           robot = Rubevy::Proxy.new("robot")
           robot.move_to(1, 2)
           robot.hp
           :done"#,
        12,
    );
    let seen = app.world().resource::<Seen>();
    assert_eq!(
        seen.asked,
        vec![("robot.move_to".to_string(), vec![1.0, 2.0]), ("robot.hp".to_string(), vec![])],
        "each call goes out once, under `kind.name`, with its arguments"
    );
}

#[test]
fn the_answer_is_the_value_of_the_call() {
    // the game answers 10 and then 20; a call that could not park would never see either
    let app = run(
        r##"require "proxy"
           robot = Rubevy::Proxy.new("robot")
           a = robot.move_to(1, 2)
           b = robot.hp
           Rubevy.ask("report", "#{a}/#{b}")
           :done"##,
        12,
    );
    assert_eq!(app.world().resource::<Seen>().reported, vec!["10.0/20.0".to_string()]);
}

#[test]
fn a_proxy_answers_respond_to_and_says_what_it_stands_for() {
    let app = run(
        r##"require "proxy"
           robot = Rubevy::Proxy.new("robot")
           Rubevy.ask("report", "#{robot.respond_to?(:anything)} #{robot.respond_to?(:move_to)} #{robot.inspect} #{robot.kind}")
           :done"##,
        8,
    );
    assert_eq!(
        app.world().resource::<Seen>().reported,
        vec!["true true #<Rubevy::Proxy robot> robot".to_string()],
        "`kind` and `inspect` are real methods, so they do not go out as questions"
    );
}

#[test]
fn one_proxy_parks_its_task_while_another_script_runs() {
    // the point of the whole thing: the body of `method_missing` blocks on the queue, and that
    // costs the frame nothing — the script beside it keeps going round
    let mut app = app();
    let (proxied, busy) = {
        let mut assets = app.world_mut().resource_mut::<Assets<MrbAsset>>();
        (
            assets.add(compile(
                r#"require "proxy"
                   robot = Rubevy::Proxy.new("robot")
                   4.times { robot.step }
                   :done"#,
            )),
            assets.add(compile(r#"loop { Rubevy.ask("report", "tick") }"#)),
        )
    };
    app.world_mut().spawn(Script::new(proxied));
    app.world_mut().spawn(Script::new(busy));
    frames(&mut app, 12);

    let seen = app.world().resource::<Seen>();
    assert_eq!(seen.asked.len(), 4, "one question per call, none of them lost");
    assert!(seen.reported.len() > 4, "the other script ran meanwhile: {} ticks", seen.reported.len());
}
