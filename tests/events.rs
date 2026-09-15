//! Events: `Rubevy.subscribe(:hit)` answers a queue the game pushes onto, so waiting for
//! something to happen is the same thing as waiting for an answer — in the script's own task or
//! in one it made. Only what a script subscribed to reaches it, a message addressed to an
//! entity reaches that entity's scripts alone, and the subscription goes when the script does.

use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;
use rubevy::{entity_object, Answer, MrbAsset, Request, RubevyPlugin, Script, ScriptWorld};
use sabiruby::Value;

#[derive(Resource, Default)]
struct Seen(Vec<Request>);

/// An event of the game's own, to show the observer line a game writes.
#[derive(EntityEvent)]
struct Hit {
    entity: Entity,
    damage: f32,
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

/// The frames it takes for a script to reach its first `Rubevy.ask("ready")` and be answered.
fn until_ready(app: &mut App) {
    for _ in 0..20 {
        frames(app, 1);
        if !app.world().resource::<Seen>().0.is_empty() {
            return;
        }
    }
}

fn asked(app: &App, kind: &str) -> Option<Request> {
    app.world().resource::<Seen>().0.iter().find(|r| r.kind == kind).cloned()
}

#[test]
fn only_what_a_script_subscribed_to_reaches_it() {
    let mut app = app();
    run(
        &mut app,
        r#"
          hits = Rubevy.subscribe(:hit)
          Rubevy.ask("ready").pop
          damage = hits.pop
          Rubevy.ask("got", damage, hits.size).pop
        "#,
    );
    until_ready(&mut app);

    {
        let mut world = app.world_mut().resource_mut::<ScriptWorld>();
        world.publish(None, "hit", Answer::Num(7.0));
        world.publish(None, "miss", Answer::Num(1.0));
    }
    frames(&mut app, 25);

    let got = asked(&app, "got").expect("the script woke on its queue");
    assert_eq!(got.num(0), Some(7.0));
    assert_eq!(got.num(1), Some(0.0), "the miss went nowhere: nobody subscribed to it");
}

#[test]
fn a_message_to_an_entity_reaches_that_entity_alone() {
    let mut app = app();
    let src = r#"
      hits = Rubevy.subscribe(:hit)
      Rubevy.ask("ready").pop
      sleep 0.01
      Rubevy.ask("count", Rubevy.entity.to_i, hits.size).pop
    "#;
    let a = run(&mut app, src);
    let _b = run(&mut app, src);
    // both scripts have to be parked on "ready" before the message goes out
    for _ in 0..20 {
        frames(&mut app, 1);
        if app.world().resource::<Seen>().0.len() >= 2 {
            break;
        }
    }
    assert_eq!(app.world().resource::<Seen>().0.len(), 2, "both scripts asked");

    app.world_mut().resource_mut::<ScriptWorld>().publish(Some(a), "hit", Answer::Num(1.0));
    frames(&mut app, 25);

    let counts: Vec<(f64, f64)> = app
        .world()
        .resource::<Seen>()
        .0
        .iter()
        .filter(|r| r.kind == "count")
        .map(|r| (r.num_or(0, -1.0), r.num_or(1, -1.0)))
        .collect();
    assert_eq!(counts.len(), 2, "both scripts reported");
    for (who, n) in counts {
        let want = if who == a.to_bits() as f64 { 1.0 } else { 0.0 };
        assert_eq!(n, want, "entity {who} had {n} messages");
    }
}

#[test]
fn another_task_can_wait_on_the_queue() {
    let mut app = app();
    run(
        &mut app,
        r#"
          hits = Rubevy.subscribe(:hit)
          $seen = []
          Task.new(name: "reflex") { loop { $seen << hits.pop } }
          Rubevy.ask("ready").pop
          sleep 0.01
          Rubevy.ask("reflex", $seen.length, $seen[0].to_f, $seen[1].to_f).pop
        "#,
    );
    until_ready(&mut app);

    {
        let mut world = app.world_mut().resource_mut::<ScriptWorld>();
        world.publish(None, "hit", Answer::Num(3.0));
        world.publish(None, "hit", Answer::Num(4.0));
    }
    frames(&mut app, 25);

    let r = asked(&app, "reflex").expect("the reflex task read the queue");
    assert_eq!(r.num(0), Some(2.0));
    assert_eq!((r.num(1), r.num(2)), (Some(3.0), Some(4.0)), "in the order they were published");
}

#[test]
fn a_payload_may_carry_an_entity() {
    let mut app = app();
    let by = app.world_mut().spawn_empty().id();
    run(
        &mut app,
        r#"
          hits = Rubevy.subscribe(:hit)
          Rubevy.ask("ready").pop
          who, damage = hits.pop
          Rubevy.ask("by", who, damage, who.class.to_s).pop
        "#,
    );
    until_ready(&mut app);

    {
        let mut world = app.world_mut().resource_mut::<ScriptWorld>();
        let class = world.entity_class();
        world.publish_value(None, "hit", move |vm| {
            let who = entity_object(vm, class, by);
            vm.ary_new(vec![who, Value::Float(2.5)])
        });
    }
    frames(&mut app, 25);

    let r = asked(&app, "by").expect("the script woke");
    assert_eq!(r.entity_arg(0), Some(by), "an entity crosses as an entity, not as a number");
    assert_eq!(r.num(1), Some(2.5));
    assert_eq!(r.text(2), Some("Rubevy::Entity"));
}

#[test]
fn an_observer_is_the_one_line_a_game_writes() {
    let mut app = app();
    app.add_observer(|on: On<Hit>, mut scripts: ResMut<ScriptWorld>| {
        scripts.publish(Some(on.entity), "hit", Answer::Num(on.damage as f64));
    });
    let script = run(
        &mut app,
        r#"
          hits = Rubevy.subscribe(:hit)
          Rubevy.ask("ready").pop
          Rubevy.ask("hurt", hits.pop).pop
        "#,
    );
    until_ready(&mut app);

    app.world_mut().trigger(Hit { entity: script, damage: 12.0 });
    frames(&mut app, 25);

    assert_eq!(asked(&app, "hurt").and_then(|r| r.num(0)), Some(12.0));
}

#[test]
fn the_oldest_messages_are_dropped_when_nobody_reads() {
    let mut app = app();
    run(
        &mut app,
        r#"
          q = Rubevy.subscribe(:tick)
          Rubevy.ask("ready").pop
          sleep 0.01                       # the host fills the queue while this waits
          Rubevy.ask("kept", q.size, q.pop).pop
        "#,
    );
    until_ready(&mut app);

    {
        let mut world = app.world_mut().resource_mut::<ScriptWorld>();
        for i in 0..100 {
            world.publish(None, "tick", Answer::Num(i as f64));
        }
    }
    frames(&mut app, 25);

    let r = asked(&app, "kept").expect("the script woke");
    assert_eq!(r.num(0), Some(ScriptWorld::QUEUE_LIMIT as f64), "the queue is capped");
    assert_eq!(r.num(1), Some(36.0), "and it is the newest 64 that are kept, not the oldest");
}

#[test]
fn the_subscription_goes_when_the_script_does() {
    let mut app = app();
    run(
        &mut app,
        r#"
          hits = Rubevy.subscribe(:hit)
          Rubevy.ask("ready").pop
          hits.pop                         # and the script ends on the first message
        "#,
    );
    until_ready(&mut app);
    frames(&mut app, 2);
    assert_eq!(
        app.world().resource::<ScriptWorld>().subscriptions(),
        1,
        "the script is parked on its queue"
    );

    app.world_mut().resource_mut::<ScriptWorld>().publish(None, "hit", Answer::Num(1.0));
    frames(&mut app, 5);
    assert_eq!(
        app.world().resource::<ScriptWorld>().subscriptions(),
        0,
        "the script ran off its end, so a queue nobody will read is not kept"
    );

    // and publishing to a name nobody listens for is not an error
    app.world_mut().resource_mut::<ScriptWorld>().publish(None, "hit", Answer::Num(1.0));
}

#[test]
fn a_despawned_script_stops_listening() {
    let mut app = app();
    let entity = run(
        &mut app,
        r#"
          hits = Rubevy.subscribe(:hit)
          Rubevy.ask("ready").pop
          loop { hits.pop }
        "#,
    );
    until_ready(&mut app);
    frames(&mut app, 3);
    assert_eq!(app.world().resource::<ScriptWorld>().subscriptions(), 1);

    app.world_mut().entity_mut(entity).despawn();
    assert_eq!(app.world().resource::<ScriptWorld>().subscriptions(), 0);
}
