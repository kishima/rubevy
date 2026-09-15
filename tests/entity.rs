//! Entities cross the boundary as objects, not as numbers: `Rubevy.entity` and `Answer::Entity`
//! hand out a `Rubevy::Entity`, a Data object carrying `Entity::to_bits` as its handle
//! (`Vm::data_new`), and `Rubevy.ask` reads one back as `Arg::Entity`. The Integer form still
//! works where a script has one, and the free hook runs when the collector takes an object.

use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;
use rubevy::{Answer, MrbAsset, Request, RubevyPlugin, Script, ScriptWorld};

/// The requests the game has taken, in order.
#[derive(Resource, Default)]
struct Seen(Vec<Request>);

/// The entities the game hands to the script in [`answer_targets`].
#[derive(Resource)]
struct Targets {
    as_object: Entity,
    as_bits: Entity,
}

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
    .init_resource::<Seen>();
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

/// One entity as an object, one as the number the game used to have to send.
fn answer_targets(mut world: ResMut<ScriptWorld>, mut seen: ResMut<Seen>, targets: Res<Targets>) {
    for r in world.take_requests() {
        let answer = match r.kind.as_str() {
            "target" => Answer::Entity(targets.as_object),
            "bits" => Answer::Num(targets.as_bits.to_bits() as f64),
            _ => Answer::Nil,
        };
        world.answer(&r, answer);
        seen.0.push(r);
    }
}

fn run(app: &mut App, src: &str) -> Entity {
    let script = app.world_mut().resource_mut::<Assets<MrbAsset>>().add(compile(src));
    app.world_mut().spawn(Script::new(script)).id()
}

#[test]
fn a_script_holds_its_entity_as_an_object() {
    let mut app = app();
    app.add_systems(Update, answer_nil);
    let entity = run(
        &mut app,
        r#"
          e = Rubevy.entity
          Rubevy.ask("who", e, e.to_i, e.class.to_s, (e == Rubevy.entity).to_s, e.inspect).pop
        "#,
    );
    frames(&mut app, 10);

    let seen = app.world().resource::<Seen>();
    let r = seen.0.first().expect("the script asked");
    assert_eq!(r.kind, "who");
    assert_eq!(r.entity, Some(entity), "the request knows whose script it is");
    // the object itself: the entity comes back as an entity, not as a number or a string
    assert_eq!(r.entity_arg(0), Some(entity));
    assert_eq!(r.num(0), None);
    assert_eq!(r.text(0), None);
    // and what Ruby can see of it
    assert_eq!(r.num(1), Some(entity.to_bits() as f64), "to_i is Entity::to_bits");
    assert_eq!(r.text(2), Some("Rubevy::Entity"));
    assert_eq!(r.text(3), Some("true"), "two objects for one entity are ==");
    assert_eq!(r.text(4), Some(format!("#<Rubevy::Entity {entity}>").as_str()));
}

#[test]
fn an_entity_the_game_answered_with_comes_back_the_same() {
    let mut app = app();
    let (as_object, as_bits) = {
        let world = app.world_mut();
        (world.spawn_empty().id(), world.spawn_empty().id())
    };
    app.insert_resource(Targets { as_object, as_bits });
    app.add_systems(Update, answer_targets);
    run(
        &mut app,
        r#"
          a = Rubevy.ask("target").pop
          b = Rubevy.ask("bits").pop
          Rubevy.ask("check", a, a.class.to_s, a.to_i).pop
          Rubevy.despawn a         # the object
          Rubevy.despawn b.to_i    # the number a game may still send
          Rubevy.ask("done").pop
        "#,
    );
    frames(&mut app, 20);

    {
        let seen = app.world().resource::<Seen>();
        let kinds: Vec<&str> = seen.0.iter().map(|r| r.kind.as_str()).collect();
        assert_eq!(kinds, vec!["target", "bits", "check", "done"]);
        let check = &seen.0[2];
        assert_eq!(check.entity_arg(0), Some(as_object), "it is the entity the game answered with");
        assert_eq!(check.text(1), Some("Rubevy::Entity"));
        assert_eq!(check.num(2), Some(as_object.to_bits() as f64));
    }
    assert!(app.world().get_entity(as_object).is_err(), "despawned through the object");
    assert!(app.world().get_entity(as_bits).is_err(), "despawned through the number");
}

#[test]
fn the_host_is_told_when_an_entity_object_is_collected() {
    let mut app = app();
    app.add_systems(Update, answer_nil);
    run(
        &mut app,
        r#"
          300.times { Rubevy.entity }
          GC.start
          Rubevy.ask("collected").pop
        "#,
    );
    frames(&mut app, 10);

    assert_eq!(app.world().resource::<Seen>().0.len(), 1, "the script got that far");
    // an entity object owns nothing, so the hook has nothing to release; what is checked is that
    // it is registered and runs, which is what a Data kind that owns something would rely on
    let freed = app.world().resource::<ScriptWorld>().freed_entities();
    assert!(freed > 250, "the collector took the dropped objects: {freed}");
}
