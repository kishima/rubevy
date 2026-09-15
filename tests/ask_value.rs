//! `Rubevy.ask` carries a Hash or an Array to the host as the value it is (`Arg::Value`), not as
//! its `to_s` and not as a copy: the host is handed the Ruby object, registered with the
//! collector for as long as the `Request` lives and let go of when the request does.
//!
//! What the tests here are actually about is the collector. A value the host holds is a value
//! nothing in the VM points at — the script has parked, the frame that built the literals is
//! gone — so it is alive only because `Vm::gc_register` says so, and it dies when the last
//! `Request` naming it is dropped. Both halves are checked with a real collection.

use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;
use rubevy::{Answer, Arg, MrbAsset, Request, RubevyPlugin, Script, ScriptWorld};
use sabiruby::convert::FromRuby;
use sabiruby::Value;

fn compile(src: &str) -> MrbAsset {
    let opts = sabiruby_compiler::Options { filename: "t.rb".into(), ..Default::default() };
    MrbAsset { bytes: sabiruby_compiler::compile(src.as_bytes(), &opts).expect("compiles") }
}

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins.set(ScheduleRunnerPlugin::run_once()),
        bevy::asset::AssetPlugin::default(),
        RubevyPlugin::default(),
    ));
    app
}

fn frames(app: &mut App, n: usize) {
    for _ in 0..n {
        std::thread::sleep(Duration::from_millis(2));
        app.update();
    }
}

fn run(app: &mut App, src: &str) -> Entity {
    let script = app.world_mut().resource_mut::<Assets<MrbAsset>>().add(compile(src));
    app.world_mut().spawn(Script::new(script)).id()
}

/// The question is asked inside a method, so that the frame that held the literals is gone by
/// the time the host sees them: what keeps them alive from there on is the host's registration
/// and nothing else. `GC.start` right after it collects while the request is already made.
const ASKS: &str = r#"
  def question
    Rubevy.ask("keep",
               [1.0, 2.5, 4.0],
               { name: "scout", speed: 2.5, tags: ["fast", "small"] },
               Array.new(400) { |i| "s#{i}" })
  end
  q = question
  GC.start
  $answer = q.pop
"#;

/// A Hash and an Array reach the host as themselves, survive a collection between
/// `take_requests` and the answer, and are let go of when the request is dropped.
#[test]
fn a_hash_and_an_array_reach_the_host_and_are_released_with_the_request() {
    let mut app = app();
    run(&mut app, ASKS);
    // the script runs and parks on `pop`; `drain_commands` turns the command into a request
    frames(&mut app, 2);

    let mut held: Vec<Request> = app.world_mut().resource_mut::<ScriptWorld>().take_requests();
    assert_eq!(held.len(), 1, "one question reached the game");
    let request = held.remove(0);
    assert_eq!(request.kind, "keep");

    // the fast paths are untouched, and everything else is one `Arg::Value` each — nothing
    // inside an Array or a Hash is flattened into an `Arg` of its own
    assert!(matches!(request.args[0], Arg::Value(_)), "the Array");
    assert!(matches!(request.args[1], Arg::Value(_)), "the Hash");
    assert_eq!(request.args.len(), 3);
    assert_eq!(request.num(0), None, "an Array is not a number");
    assert_eq!(request.text(1), None, "a Hash is not its `to_s`");

    let world = &mut *app.world_mut().resource_mut::<ScriptWorld>();

    // a collection between `take_requests` and `answer`, which is the moment the plan is about:
    // the script is parked, the method that built these has returned, and the values are alive
    // only because the host holds them
    world.vm.gc_collect();
    let live_held = world.vm.live_after_gc;

    // the Array, through sabiruby's `FromRuby`
    let waypoints: Vec<f64> = request.arg_as(&mut world.vm, 0).expect("an Array of numbers");
    assert_eq!(waypoints, vec![1.0, 2.5, 4.0], "intact after the collection");

    // the Hash, entry by entry (the VM has no `FromRuby` for a map; `hash_entries` is the way)
    let h = request.value(1).expect("the Hash is a value");
    let entries = world.vm.hash_entries(h).expect("a Hash");
    assert_eq!(entries.len(), 3);
    let mut by_key: Vec<(String, Value)> = Vec::new();
    for (k, v) in entries {
        let k = String::from_utf8_lossy(&world.vm.as_string(k).expect("a name")).into_owned();
        by_key.push((k, v));
    }
    assert_eq!(by_key[0].0, "name");
    assert_eq!(
        String::from_utf8_lossy(&world.vm.as_string(by_key[0].1).expect("a String")),
        "scout"
    );
    assert_eq!(by_key[1].1, Value::Float(2.5), "speed");
    // the nested Array is still a Ruby Array, read the same way (nothing was flattened)
    let tags = Vec::<String>::from_ruby(&mut world.vm, by_key[2].1).expect("an Array of Strings");
    assert_eq!(tags, vec!["fast", "small"]);

    // 400 strings, the size that makes the release visible below
    let names: Vec<String> = request.arg_as(&mut world.vm, 2).expect("an Array of Strings");
    assert_eq!(names.len(), 400);
    assert_eq!(names[399], "s399");

    // answered, and then dropped: the drop is what lets the values go
    world.answer(&request, Answer::Num(1.0));
    drop(request);
    drop(held);
    let released = world.release_dropped_values();
    assert_eq!(released, 3, "the Array, the Hash and the Array of names");

    // the script resumes, returns from `pop` and ends, so nothing in the VM points at them
    // either any more
    frames(&mut app, 3);
    let world = &mut *app.world_mut().resource_mut::<ScriptWorld>();
    world.vm.gc_collect();
    let live_after = world.vm.live_after_gc;
    assert!(
        live_held - live_after >= 400,
        "the collector took what the request had been holding: {live_held} -> {live_after}"
    );
}

/// The three fast paths are unchanged, and `nil`/`true`/`false` — which used to arrive as the
/// empty string, through `to_s` — are values now.
#[test]
fn numbers_strings_and_entities_keep_their_fast_paths() {
    let mut app = app();
    let e = run(
        &mut app,
        r#"
          Rubevy.ask("mixed", 3, 4.5, "text", :sym, Rubevy.entity, nil, true).pop
        "#,
    );
    frames(&mut app, 2);

    let held = app.world_mut().resource_mut::<ScriptWorld>().take_requests();
    assert_eq!(held.len(), 1);
    let r = &held[0];
    assert_eq!(r.num(0), Some(3.0));
    assert_eq!(r.num(1), Some(4.5));
    assert_eq!(r.text(2), Some("text"));
    assert_eq!(r.text(3), Some("sym"), "a Symbol is still its name");
    assert_eq!(r.entity_arg(4), Some(e));
    assert_eq!(r.value(5), Some(Value::Nil));
    assert_eq!(r.value(6), Some(Value::True));
    // an immediate has no object to register, so nothing is waiting for a sweep
    drop(held);
    let world = &mut *app.world_mut().resource_mut::<ScriptWorld>();
    assert_eq!(world.release_dropped_values(), 0, "nil and true own nothing");
}

/// A value carried by a request that nobody ever answers is still let go of when the request is
/// dropped: the registration belongs to the `Request`, not to the answer.
#[test]
fn a_request_that_is_never_answered_still_releases_its_value() {
    let mut app = app();
    run(&mut app, r#"Rubevy.ask("dropped", { a: 1 }).pop"#);
    frames(&mut app, 2);

    let held = app.world_mut().resource_mut::<ScriptWorld>().take_requests();
    assert_eq!(held.len(), 1);
    drop(held);

    let world = &mut *app.world_mut().resource_mut::<ScriptWorld>();
    assert_eq!(world.release_dropped_values(), 1, "the Hash was let go of unanswered");
}

/// A `Request` may be cloned and kept in two places; the value goes when the last of them does.
#[test]
fn a_cloned_request_holds_the_value_until_the_last_clone_is_gone() {
    let mut app = app();
    run(&mut app, r#"Rubevy.ask("twice", [1, 2, 3]).pop"#);
    frames(&mut app, 2);

    let held = app.world_mut().resource_mut::<ScriptWorld>().take_requests();
    let copy = held[0].clone();
    drop(held);
    {
        let world = &mut *app.world_mut().resource_mut::<ScriptWorld>();
        assert_eq!(world.release_dropped_values(), 0, "the clone still holds it");
        world.vm.gc_collect();
        let items: Vec<f64> = copy.arg_as(&mut world.vm, 0).expect("still there");
        assert_eq!(items, vec![1.0, 2.0, 3.0]);
    }
    drop(copy);
    let world = &mut *app.world_mut().resource_mut::<ScriptWorld>();
    assert_eq!(world.release_dropped_values(), 1, "and the last clone lets it go");
}
