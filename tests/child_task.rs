//! A task a script makes with `Task.new` is still the script's: it speaks for the same entity.
//!
//! The natives read the entity off the task the scheduler is running (`@rubevy_entity`, which
//! the plugin hangs on the task it spawned), and a task made out of a block carries nothing —
//! so `Rubevy.ask` used to reach the game with no entity and `Rubevy.subscribe` refused
//! outright. `src/prelude.rb` copies the entity in `Task.new`, where the task that is doing the
//! making is still the one running.

use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;
use rubevy::{Answer, MrbAsset, Request, RubevyPlugin, Script, ScriptWorld};

#[derive(Resource, Default)]
struct Seen(Vec<Request>);

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
    ))
    .init_resource::<Seen>()
    .add_systems(Update, answer_nil);
    app
}

fn frames(app: &mut App, n: usize) {
    for _ in 0..n {
        std::thread::sleep(Duration::from_millis(2));
        app.update();
    }
}

fn answer_nil(mut world: ResMut<ScriptWorld>, mut seen: ResMut<Seen>) {
    for r in world.take_requests() {
        world.answer(&r, Answer::Nil);
        seen.0.push(r);
    }
}

fn run(app: &mut App, src: &str) -> Entity {
    let script = app.world_mut().resource_mut::<Assets<MrbAsset>>().add(compile(src));
    app.world_mut().spawn(Script::new(script)).id()
}

fn asked(app: &App, kind: &str) -> Option<Request> {
    app.world().resource::<Seen>().0.iter().find(|r| r.kind == kind).cloned()
}

#[test]
fn a_task_a_script_made_asks_as_that_script() {
    let mut app = app();
    let entity = run(
        &mut app,
        r#"
          Task.new(name: "worker") { Rubevy.ask("from_child", Rubevy.entity.to_i).pop }
          sleep 0.05
        "#,
    );
    frames(&mut app, 25);

    let r = asked(&app, "from_child").expect("the child task asked the game");
    assert_eq!(r.entity, Some(entity), "the request carries the script's entity");
    assert_eq!(
        r.num(0),
        Some(entity.to_bits() as f64),
        "and `Rubevy.entity` answers it inside the child task"
    );
}

#[test]
fn a_task_a_script_made_may_subscribe() {
    let mut app = app();
    let entity = run(
        &mut app,
        r#"
          $got = nil
          Task.new(name: "listener") do
            hits = Rubevy.subscribe(:hit)
            Rubevy.ask("listening").pop
            $got = hits.pop
          end
          sleep 0.01
          Rubevy.ask("ready").pop
          sleep 0.05
          Rubevy.ask("heard", $got.to_f).pop
        "#,
    );
    for _ in 0..20 {
        frames(&mut app, 1);
        if asked(&app, "listening").is_some() {
            break;
        }
    }
    assert!(asked(&app, "listening").is_some(), "the child task subscribed without being refused");
    assert_eq!(
        app.world().resource::<ScriptWorld>().subscriptions(),
        1,
        "and the subscription belongs to an entity, so a message addressed to one reaches it"
    );

    // addressed to the entity, which is what the child task had to inherit for this to arrive
    app.world_mut().resource_mut::<ScriptWorld>().publish(Some(entity), "hit", Answer::Num(9.0));
    frames(&mut app, 25);

    assert_eq!(asked(&app, "heard").and_then(|r| r.num(0)), Some(9.0));
}

#[test]
fn a_task_may_speak_for_another_entity_if_it_says_so() {
    let mut app = app();
    let other = app.world_mut().spawn_empty().id();
    let src = format!(
        r#"
          t = Task.new(name: "stand-in") {{ Rubevy.ask("stand_in", Rubevy.entity.to_i).pop }}
          t.instance_variable_set(:@rubevy_entity, {bits})
          sleep 0.05
        "#,
        bits = other.to_bits()
    );
    run(&mut app, &src);
    frames(&mut app, 25);

    let r = asked(&app, "stand_in").expect("the child task asked");
    assert_eq!(r.entity, Some(other), "what a script writes itself wins over what it inherited");
}
