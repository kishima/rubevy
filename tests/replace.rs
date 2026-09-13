//! A script replaced by another — `ScriptTask` removed and a new `Script` inserted, which is how a
//! game reloads a brain — stops: only the new one keeps asking. The same for a despawned entity.

use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;
use rubevy::{Answer, MrbAsset, RubevyPlugin, Script, ScriptDone, ScriptTask, ScriptWorld};

/// Requests seen per kind, so far.
#[derive(Resource, Default)]
struct Seen {
    old: usize,
    new: usize,
}

fn compile(src: &str) -> MrbAsset {
    let opts = sabiruby_compiler::Options { filename: "t.rb".into(), ..Default::default() };
    MrbAsset { bytes: sabiruby_compiler::compile(src.as_bytes(), &opts).expect("compiles") }
}

fn answer_all(mut world: ResMut<ScriptWorld>, mut seen: ResMut<Seen>) {
    for r in world.take_requests() {
        match r.kind.as_str() {
            "old" => seen.old += 1,
            "new" => seen.new += 1,
            _ => {}
        }
        world.answer(&r, Answer::Nil);
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

// One test, not two: the queue between natives and systems is a static, so two apps running in
// parallel test threads would take each other's requests.
#[test]
fn a_script_whose_task_is_removed_stops() {
    a_replaced_script_stops();
    a_despawned_script_stops();
}

fn a_replaced_script_stops() {
    let mut app = app();
    let (old, new) = {
        let mut assets = app.world_mut().resource_mut::<Assets<MrbAsset>>();
        (
            assets.add(compile("loop { Rubevy.ask('old').pop }")),
            assets.add(compile("loop { Rubevy.ask('new').pop }")),
        )
    };
    let entity = app.world_mut().spawn(Script::new(old)).id();
    frames(&mut app, 10);
    assert!(app.world().resource::<Seen>().old > 0, "the first script runs");

    app.world_mut()
        .entity_mut(entity)
        .remove::<ScriptTask>()
        .remove::<ScriptDone>()
        .insert(Script::new(new));
    // a question asked before the swap may still be answered once
    frames(&mut app, 3);
    let before = app.world().resource::<Seen>().old;
    frames(&mut app, 20);
    let seen = app.world().resource::<Seen>();
    assert_eq!(seen.old, before, "the replaced script keeps asking");
    assert!(seen.new > 0, "the new script runs");
}

fn a_despawned_script_stops() {
    let mut app = app();
    let old = app.world_mut().resource_mut::<Assets<MrbAsset>>().add(compile("loop { Rubevy.ask('old').pop }"));
    let entity = app.world_mut().spawn(Script::new(old)).id();
    frames(&mut app, 10);
    assert!(app.world().resource::<Seen>().old > 0);
    app.world_mut().despawn(entity);
    frames(&mut app, 3);
    let before = app.world().resource::<Seen>().old;
    frames(&mut app, 20);
    assert_eq!(app.world().resource::<Seen>().old, before, "the despawned script keeps asking");
}
