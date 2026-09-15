//! Components by name: `entity.get(:Transform)` reads one through Bevy's reflection, `entity[:X] =`
//! writes it back with the frame's other commands, `has?` and `components` say what is there,
//! and `Rubevy.find` looks for everything that has a component. No type in rubevy knows what a
//! `Transform` or an `Hp` is; the registry does.

use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;
use rubevy::{Answer, MrbAsset, Request, RubevyPlugin, Script, ScriptWorld};

/// The requests the game itself took, which must never be one of rubevy's own kinds.
#[derive(Resource, Default)]
struct Seen(Vec<Request>);

#[derive(Component, Reflect, Debug, Default, PartialEq)]
#[reflect(Component)]
struct Hp {
    current: f32,
    max: f32,
}

#[derive(Component, Reflect, Debug, Default)]
#[reflect(Component)]
struct Npc;

#[derive(Component, Reflect, Debug, Default, PartialEq)]
#[reflect(Component)]
enum Mode {
    #[default]
    Idle,
    Hunting,
}

/// Registered nowhere, so nothing about it is visible from Ruby.
#[derive(Component, Debug)]
struct Secret(#[allow(dead_code)] f32);

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
    // `MinimalPlugins` brings no `TransformPlugin`, so the test registers what it uses
    .register_type::<Transform>()
    .register_type::<Hp>()
    .register_type::<Npc>()
    .register_type::<Mode>()
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

fn run(app: &mut App, entity: Entity, src: &str) {
    let script = app.world_mut().resource_mut::<Assets<MrbAsset>>().add(compile(src));
    app.world_mut().entity_mut(entity).insert(Script::new(script));
}

#[test]
fn a_script_reads_a_transform_as_a_hash() {
    let mut app = app();
    let entity = app.world_mut().spawn(Transform::from_xyz(1.0, 2.0, 3.0)).id();
    run(
        &mut app,
        entity,
        r#"
          tf = Rubevy.entity.get(:Transform)
          Rubevy.ask("read",
                     tf.class.to_s,
                     tf[:translation].length,
                     tf[:translation][0], tf[:translation][1], tf[:translation][2],
                     tf[:rotation].length,
                     tf[:scale][0]).pop
        "#,
    );
    frames(&mut app, 10);

    let seen = app.world().resource::<Seen>();
    let r = seen.0.first().expect("the script asked");
    assert_eq!(r.kind, "read");
    assert_eq!(r.text(0), Some("Hash"), "a component is a Hash of its fields");
    assert_eq!(r.num(1), Some(3.0), "a Vec3 is three numbers, not a Hash of x/y/z");
    assert_eq!((r.num(2), r.num(3), r.num(4)), (Some(1.0), Some(2.0), Some(3.0)));
    assert_eq!(r.num(5), Some(4.0), "a Quat is four");
    assert_eq!(r.num(6), Some(1.0), "the default scale");
}

#[test]
fn a_script_writes_one_field_and_leaves_the_others() {
    let mut app = app();
    let entity = app.world_mut().spawn((Transform::from_xyz(1.0, 2.0, 3.0), Hp { current: 7.0, max: 10.0 })).id();
    run(
        &mut app,
        entity,
        r#"
          e = Rubevy.entity
          tf = e.get(:Transform)
          tf[:translation][0] += 5.0
          e[:Transform] = tf
          e[:Hp] = { current: 4.0 }        # `max` is not named, so it stays
          Rubevy.ask("written").pop
        "#,
    );
    frames(&mut app, 10);

    assert_eq!(app.world().resource::<Seen>().0.len(), 1, "the script got that far");
    let t = app.world().get::<Transform>(entity).expect("still there");
    assert_eq!(t.translation, Vec3::new(6.0, 2.0, 3.0));
    assert_eq!(t.scale, Vec3::ONE, "what the script sent back unchanged");
    assert_eq!(app.world().get::<Hp>(entity), Some(&Hp { current: 4.0, max: 10.0 }));
}

#[test]
fn a_symbol_switches_an_enum_component() {
    let mut app = app();
    let entity = app.world_mut().spawn(Mode::Idle).id();
    run(
        &mut app,
        entity,
        r#"
          was = Rubevy.entity.get(:Mode)
          Rubevy.entity[:Mode] = :Hunting
          Rubevy.ask("mode", was.to_s, was.class.to_s).pop
        "#,
    );
    frames(&mut app, 10);

    let seen = app.world().resource::<Seen>();
    let r = seen.0.first().expect("the script asked");
    assert_eq!(r.text(0), Some("Idle"), "a unit variant reads as its name");
    assert_eq!(r.text(1), Some("Symbol"));
    assert_eq!(app.world().get::<Mode>(entity), Some(&Mode::Hunting));
}

#[test]
fn has_and_components_and_the_unregistered() {
    let mut app = app();
    let entity = app.world_mut().spawn((Transform::default(), Hp::default(), Secret(1.0))).id();
    run(
        &mut app,
        entity,
        r#"
          e = Rubevy.entity
          Rubevy.ask("what",
                     e.has?(:Hp).to_s,
                     e.has?(:Npc).to_s,
                     e.has?(:Secret).to_s,
                     e.components.join(","),
                     e.get(:Secret).inspect).pop
        "#,
    );
    frames(&mut app, 10);

    let seen = app.world().resource::<Seen>();
    let r = seen.0.first().expect("the script asked");
    assert_eq!(r.text(0), Some("true"));
    assert_eq!(r.text(1), Some("false"));
    assert_eq!(r.text(2), Some("false"), "a type nobody registered is not there, for Ruby");
    let names = r.text(3).expect("a list");
    assert!(names.contains("Transform") && names.contains("Hp"), "{names}");
    assert!(!names.contains("Secret"), "{names}");
    assert_eq!(r.text(4), Some("nil"), "and reading it is nil, not an error");
}

#[test]
fn find_answers_entity_objects() {
    let mut app = app();
    let (a, b) = {
        let world = app.world_mut();
        (world.spawn(Npc).id(), world.spawn(Npc).id())
    };
    let watcher = app.world_mut().spawn_empty().id();
    run(
        &mut app,
        watcher,
        r#"
          found = Rubevy.find(:Npc)
          Rubevy.ask("found", found.length, found[0], found[1], found[0].class.to_s).pop
        "#,
    );
    frames(&mut app, 10);

    let seen = app.world().resource::<Seen>();
    let r = seen.0.first().expect("the script asked");
    assert_eq!(r.num(0), Some(2.0));
    let mut got = [r.entity_arg(1).expect("an entity"), r.entity_arg(2).expect("an entity")];
    got.sort();
    let mut want = [a, b];
    want.sort();
    assert_eq!(got, want, "the entities come back as entities");
    assert_eq!(r.text(3), Some("Rubevy::Entity"));
}

#[test]
fn rubevys_own_questions_never_reach_the_game() {
    let mut app = app();
    let entity = app.world_mut().spawn(Transform::default()).id();
    run(
        &mut app,
        entity,
        r#"
          e = Rubevy.entity
          e.get(:Transform)
          e.has?(:Transform)
          e.components
          Rubevy.find(:Npc)
          Rubevy.ask("mine").pop
        "#,
    );
    frames(&mut app, 15);

    let kinds: Vec<&str> =
        app.world().resource::<Seen>().0.iter().map(|r| r.kind.as_str()).collect();
    assert_eq!(kinds, vec!["mine"], "take_requests hands over only the game's own kinds");
}
