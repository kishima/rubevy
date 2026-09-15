# What a script can say to the game, and the game to a script

The Ruby side of rubevy is small on purpose: a script is a task in one VM (`docs/outlook.ja.md`),
and everything it does to the world goes through `Rubevy`. This file is the current surface and
the rules behind it.

## From the script to the world (one way)

A native cannot touch Bevy's `World` — it gets `&mut Vm` and nothing else — so these leave a
command behind and the `drain_commands` system carries it out later in the same frame.

| Ruby | What happens |
|---|---|
| `Rubevy.log "text"` | `info!` through Bevy's log |
| `Rubevy.spawn "name", x, y, z` | an entity with `SpawnedByScript { name }` and a `Transform` |
| `Rubevy.despawn entity` | that entity is despawned |
| `Rubevy.entity` | the entity this script is attached to, as a `Rubevy::Entity`, or nil |
| `Rubevy.move_to x, y, z` | the script's own entity is moved |
| `Rubevy.set_position entity, x, y, z` | any entity is moved |
| `entity[:Transform] = hash` | the fields the Hash names are written over that component |

## What an entity is, on the Ruby side

`Rubevy.entity` — and any [`Answer::Entity`] the game sends back — is a `Rubevy::Entity`: an
object whose *handle* is `Entity::to_bits`, which the VM carries and never reads through
(SabiRuby's `Vm::data_new`). What a script can do with one:

```ruby
e = Rubevy.entity
e.to_i                     # 4294967294 — Entity::to_bits, exactly
e == Rubevy.entity         # true: two objects for one entity are equal, and are one Hash key
e.inspect                  # "#<Rubevy::Entity 1v0>" (Bevy's own name for it)
Rubevy.despawn e           # and it can be handed back
Rubevy.ask("look_at", e)   # → Arg::Entity in the Request, read with `entity_arg(0)`
```

It is not an Integer, which is the point: `Rubevy.despawn 3` used to name somebody, and an
entity that had been past an `f64` (`Answer::Num`) lost its low bits once a generation went
past 2^21. `dup` and `clone` raise, so a handle is not copied behind the host's back.
`Rubevy.despawn` and `Rubevy.set_position` still take the Integer form as well, which is what a
script gets from an `Answer::Num`, `List` or `Rows` — a table of entities is still a table of
numbers, and `entity_of(bits)` is the way back on the Rust side.

## From the script to the game and back (`Rubevy.ask`)

`Rubevy.ask` is the shape everything asynchronous takes: a sensor reading, a path request, a file
the game has to load, an answer another system computes. It returns a `Task::Queue`; `pop` parks
the script's task until the game pushes an answer.

```ruby
found = Rubevy.ask("scan", 40.0).pop     # parked here; the other scripts keep running
Rubevy.move_to found[0], found[1], 0.0 if found
```

The game answers in a system of its own:

```rust
fn answer_requests(mut world: ResMut<ScriptWorld>, /* whatever the answer needs */) {
    for request in world.take_requests() {          // Request { entity, kind, args, queue }
        let answer = Answer::List(vec![3.0, 3.5]);  // Nil / Bool / Num / Text / List / Rows / Entity
        world.answer(&request, answer);             // this frame, or keep it for a later one
    }
}
```

`examples/sensor.rs` does exactly that, answering two frames after the question to show that a
request may outlive the frame it was made on.

**Why a queue rather than a callback or a Future.** The VM already has a scheduler (mruby-task)
whose tasks can wait: on a deadline (`sleep`), on another task (`join`), or on a queue. An
asynchronous host call is just one more thing to wait on, so it needs no second scheduler, no
`await` keyword, and nothing in the script that looks asynchronous. A parked task costs nothing —
it is not polled, not woken per frame, and the instruction budget goes to the scripts that can
actually run.

**Rules.**

* Answer each request once. `ScriptWorld::answer` lets go of the queue (`gc_unregister`) as it
  answers, so a second answer to the same request is a mistake.
* A request you cannot answer yet is yours to keep; nothing expires. If the script should not wait
  forever, it can say so on the Ruby side: `Rubevy.ask(…).pop(timeout_ms: 500)` answers nil when
  the deadline passes.
* `kind` is a string; the arguments after it are numbers, strings, entities, or any other Ruby
  value ([`Arg`]), and `Request::num(i)` / `Request::text(i)` / `Request::entity_arg(i)` /
  `Request::value(i)` read them — each answers `None` for an argument of another sort. An answer
  is nil, a bool, a number, a string, an entity, a list of numbers, or a table of them
  ([`Answer::Rows`] — every robot with its team and hp, say), or anything the host builds in the
  VM itself (`answer_value`). The question's side is described below.

## What a question may carry (`Arg`)

Three sorts of argument are copied out of the VM at the call and cost the host nothing
afterwards:

| Ruby | `Arg` | read with |
|---|---|---|
| Integer, Float | `Arg::Num(f64)` | `request.num(i)`, `request.num_or(i, d)` |
| String, Symbol | `Arg::Text(String)` | `request.text(i)` |
| `Rubevy::Entity` | `Arg::Entity(Entity)` | `request.entity_arg(i)` |

Everything else — a Hash, an Array, an object of the script's own, `nil`, `true`, `false` — is
`Arg::Value`, which is the Ruby value **itself**, not a copy of it:

```ruby
Rubevy.ask("path", { from: [1.0, 2.0], to: [8.0, 3.0], avoid: ["lava"] }).pop
```

```rust
for request in world.take_requests() {
    // sabiruby's `FromRuby`, for anything that trait knows: Vec<f64>, Vec<String>, f64, …
    let waypoints: Vec<f64> = request.arg_as(&mut world.vm, 0).unwrap_or_default();

    // a Hash has no `FromRuby` in the VM, so it is read entry by entry
    let h = request.value(0).expect("a value");
    for (k, v) in world.vm.hash_entries(h).unwrap_or_default() {
        let name = world.vm.as_string(k).unwrap_or_default();
        // … `f64::from_ruby(&mut world.vm, v)`, `vm.ary_vals(v)`, `vm.hash_entries(v)`
    }
}
```

**Nothing inside is flattened.** An argument is one `Arg`, and which one it is depends on that
argument alone: `Rubevy.ask("path", [1.0, 2.0], 3)` is an `Arg::Value` and an `Arg::Num`, and the
numbers inside the Array stay Ruby numbers inside a Ruby Array (`request.num(0)` is `None`). The
alternative — walking the structure and turning each leaf into an `Arg` — would lose the shape,
which is the only thing the structure was for.

**Why the value rather than a copy.** A component write (`entity[:Transform] = hash`) *is* copied
out, because the write happens a system later, with a `&mut World` and no VM in sight. A question
is the other way round: the host has the `ScriptWorld`, so it has the `Vm`, and copying a nested
value out would mean a second and poorer `Answer` — the same argument `answer_value` makes on the
way back. Before this, a Hash arrived as `Arg::Text` of its `to_s`, which threw away everything a
structure was for.

**How it is kept alive, and let go of.** The value is a value nothing in the VM points at any
more — the script has parked on its queue, the frame that built the literal may have returned —
so it lives only because `Vm::gc_register` says so. That registration is made once, in the
`Rubevy.ask` native, and is owned by an `Arc` inside the `Request`. When the last `Request` naming
it is dropped — answered and let go, dropped unanswered, lost with a system that panicked — the
`Drop` puts the object on a queue and the plugin unregisters it at the head of the next frame
(`ScriptWorld::release_dropped_values`, which a host driving `ScriptWorld` without the plugin's
systems calls itself).

So a game has nothing to remember. There is no `release(request)` to forget, `Request` stays
`Clone` and may be kept for as many frames as the answer takes, and the only way to hold a value
for ever is to hold a `Request` for ever. The cost of the two-step release is that a value
outlives its request by up to one frame, which is the safe side of the mistake.

`tests/ask_value.rs` checks all of it against a real collection: a Hash and an Array asked for
inside a method (so the script itself holds nothing), a `GC.start` in the script and a
`Vm::gc_collect` between `take_requests` and the answer, and the live count falling by the size
of the structure once the request is dropped. Without the registration the same test says
`access to freed object`.

## Making the answer later (`answer_with`)

Where the answer is work rather than a lookup — a path search, a file, a query, anything that
does not fit in a frame — hand the request and a future to `ScriptWorld::answer_with` instead
of answering on the spot:

```rust
fn answer_paths(mut world: ResMut<ScriptWorld>) {
    for request in world.take_requests() {
        let to = (request.num_or(0, 0.0), request.num_or(1, 0.0));
        world.answer_with(request, async move { Answer::Rows(search(to).await) });
    }
}
```

The future goes to Bevy's `AsyncComputeTaskPool`, and a system of the plugin answers the request
on the frame it finishes. The request is the one `take_requests` gave you, handed over as it is;
it is answered once, there, so do not answer it again yourself. `ScriptWorld::answering()` says
how many are still out.

Nothing changes for the script: it is parked on its queue from the `Rubevy.ask` until the answer
arrives, exactly as when a system keeps the request for three frames, and the other scripts keep
running. `examples/async.rs` asks on frame 0 and is answered on frame 6, with the ticker beside
it running on 3 and 5.

The plugin picks the finished futures up at the head of the frame, before the scripts run, rather
than at the end beside `drain_commands`: a future finishes whenever its thread is done, and most
of a frame's wall clock is outside the schedule, so an answer that arrived in that gap wakes its
script on this frame instead of the next one.

**Threads are the app's choice, not this plugin's.** Bevy's task pools are threads only in a
build with bevy's `multi_threaded` feature. Without it they are a single-threaded fallback on the
main thread, where a future that only computes is driven to its end inside `answer_with` — the
frame waits for it — while a future waiting on something else (a channel, an IO completion, a
waker of your own) still parks and is picked up later. An app that wants the work off the main
thread puts `multi_threaded` in the features of its own `bevy` dependency; this crate's
`dev-dependencies` do that for the example and the tests. Either way the app needs Bevy's
`TaskPoolPlugin`, which `MinimalPlugins` and `DefaultPlugins` both add — `answer_with` panics
without it, because the pool it spawns on does not exist.

## Components by name

A script reads a component as a Hash of its fields and writes one back by naming the fields it
means. Nothing in rubevy knows what a `Transform` is: the plugin walks whatever Bevy's type
registry holds, through `ReflectComponent`, so a game's own component works the same way.

```ruby
e  = Rubevy.entity
tf = e[:Transform]              # {translation: [x, y, z], rotation: [x, y, z, w], scale: [...]}
tf[:translation][0] += 1.0
e[:Transform] = tf              # applied after this frame's scripts have run

e.has?(:Velocity)               # true / false
e.components                    # ["Transform", "Sprite", ...] — the short type names, sorted
Rubevy.find(:Npc)               # every entity with that component, as Rubevy::Entity objects
```

**What a type has to do to be there.** `#[derive(Reflect)]`, `#[reflect(Component)]`, and
`app.register_type::<T>()`. Bevy's own types are registered by the app — `DefaultPlugins` does
it, and `MinimalPlugins` does not, so `examples/components.rs` names `Transform` itself (in
bevy 0.19 even `TransformPlugin` only propagates transforms; the automatic registration is
bevy's `reflect_auto_register` feature, which this crate does not turn on). A type nobody
registered is not an error on the Ruby side: the component reads as `nil`, `has?` answers
`false`, and it is not in `components`.

**What a value looks like.**

| reflected | Ruby |
|---|---|
| struct | Hash, keys as Symbols |
| `Vec2` / `Vec3` / `Vec3A` / `Vec4` / `Quat` | Array of Floats — they are structs of `x`, `y`, … in bevy_reflect, but `tf[:translation][0]` is what a script wants |
| tuple struct, tuple, array, list, set | Array |
| map | Hash |
| enum, unit variant | the variant's name as a Symbol (`:Hidden`) |
| enum, tuple or struct variant | a one-entry Hash, `{Srgba: {red: …}}` |
| numbers, `bool`, String, `char` | the Ruby ones |
| `Entity` | a `Rubevy::Entity` object, not a number |
| anything else opaque | nil |

**What a write does.** Only the fields the Hash names, so `e[:Hp] = { current: 4.0 }` leaves
`max` alone and two scripts that touch different fields do not undo each other — and reading a
component, changing one number and writing it back is one round trip, not two. An Array over a
struct goes by position, which is how `[1.0, 2.0, 3.0]` reaches a `Vec3`. A Symbol switches an
enum to that variant, as long as the variant has no fields: bevy builds a new variant from the
value handed to it, and a Ruby Hash says nothing about types, so a tuple or struct variant can
only have its fields written while it is already the current one. A field that cannot take what
it was given is logged with its path (`translation.x: takes a number`) and skipped; the rest of
the write still happens.

**A read costs a frame.** `e[:Transform]` is `Rubevy.ask` under a nicer name: the question
goes out with the frame's commands and rubevy answers it at the head of the next frame, before
the scripts run. The task is parked meanwhile, so it costs nothing and the other scripts keep
running, but this is a boundary for declaration time and for events — not for a dozen reads a
frame. `Rubevy.find` walks every entity in the world, so it is for a lookup now and then.

The four kinds rubevy answers itself — `component.get`, `component.has`, `components`,
`entities.with` — never reach `ScriptWorld::take_requests`: they are sorted out where the
request is made, so a game's answering system sees only its own. A game that wants those names
picks others.

**Why a read can wait inside `[]`.** mrbc folds a one-argument `[]` into `OP_GETIDX`, which the
VM used to dispatch with `funcall` — a nested run loop, which is a native boundary, and a task
cannot be parked across one (`blocking pop cannot be called from within a C function boundary`).
For a while the read was `e.get(:Transform)` for that reason. SabiRuby now does what the
reference does: the opcode answers an Array, Hash or String itself and *sends* everything else
in the frame the call was made in, so a `[]` written in Ruby is an ordinary frame that a task
can be parked in. `e[:Transform]` is the spelling; `e.get(:Transform)` is the same call, kept
because spelling the round trip out reads better where it matters. The Ruby side of all this is
`src/prelude.rb`, compiled to `.mrb` and run when the VM starts, so a script has these without
requiring anything.

## Events

`Rubevy.subscribe(:hit)` answers a `Task::Queue` that a game pushes onto. Waiting for something
to happen is then the same thing as waiting for an answer — the task is parked and costs
nothing — and it can be done in a task of the script's own:

```ruby
hits = Rubevy.subscribe(:hit)          # this entity's own, and everyone's
Task.new(name: "reflex") do
  loop do
    by, damage = hits.pop              # parked here until the game publishes
    Rubevy.log "hit by #{by.inspect} for #{damage}"
  end
end
```

A reflex is then not a callback that interrupts the brain; it is another task that happens to be
ready — and it is the script's task in every way that matters here: **a task made with
`Task.new` carries the entity of the task that made it**, so it may `Rubevy.subscribe`,
`Rubevy.ask` and `Rubevy.entity` for itself. (The entity is an ordinary instance variable on the
task, `@rubevy_entity`, and the prelude copies it in `Task.new`, where the task doing the making
is still the one running. A task that should act for another entity sets it itself.) Subscribing
in the script's own task and sharing the queue, as above, is still the clearer shape when
several tasks read one stream.

The game publishes:

```rust
scripts.publish(Some(entity), "hit", Answer::Num(damage as f64));   // that entity's scripts
scripts.publish(None, "bell", Answer::Num(frame as f64));           // everyone who subscribed
```

and for a payload that is not flat, `publish_value(entity, name, |vm| …)` builds the value
inside the VM, the way `answer_value` does. An entity inside a list is
`entity_object(vm, class, entity)`, with `class` read from `ScriptWorld::entity_class()` before
the closure.

**Bevy's events are joined by the game, one line per event type.** rubevy does not tie them to
Ruby by itself: which events a script may see, and what each one carries, is the game's to say.

```rust
app.add_observer(|on: On<Hit>, mut scripts: ResMut<ScriptWorld>| {
    scripts.publish(Some(on.entity), "hit", Answer::Num(on.damage as f64));
});
```

An ordinary system reading `MessageReader` does just as well. `examples/events.rs` has both, and
a script (`assets/scripts/events.rb`) with a brain and a reflex.

**Rules.**

* Nothing is queued for a name nobody subscribed to, so a game may publish freely.
* A queue holds `ScriptWorld::QUEUE_LIMIT` (64) messages and drops the oldest past that. A
  script that wakes late gets the last 64 things that happened, not the first 64 — and a script
  that never reads its queue is not a leak with a slow fuse.
* A subscription is let go of when the script's task ends and when its `ScriptTask` is removed
  (a reload, a despawn). A script that runs off its end keeps its `ScriptTask` — that is what
  stops it starting again — so both places matter. `ScriptWorld::subscriptions()` says how many
  are standing.
* **The queue is closed when the subscription goes, and a `pop` waiting on it raises
  `Rubevy::Unsubscribed`.** The script's own task is terminated with its `ScriptTask`, but a
  task it made with `Task.new` is not: parked on a message that will never come, it would stand
  there for as long as the VM lives, holding its context and everything its block closed over,
  and its `ensure` would never run. The raise unwinds it instead:

  ```ruby
  Task.new(name: "reflex") do
    begin
      loop { handle(hits.pop) }
    rescue Rubevy::Unsubscribed        # the script is going; end on our own terms
      Rubevy.log "reflex off"
    ensure
      release_the_wheel                # this runs either way
    end
  end
  ```

  Whatever was already in the queue is popped first, so a script that was behind still reads
  what it missed before it hears the end. An unrescued `Rubevy::Unsubscribed` simply becomes
  that task's result, as any unhandled exception in a task does; nothing else stops.

  It is the subscription's queue that says this, not every queue: `Rubevy.subscribe` extends the
  one it answers with `Rubevy::Subscription` (`pop`, `shift`, `deq`). A queue from `Rubevy.ask`
  is an ordinary `Task::Queue` and keeps mruby-task's own meaning, where a closed queue answers
  `pop` with nil.

## A dynamic proxy (`Rubevy::Proxy`)

`assets/scripts/proxy.rb` is a small Ruby library that turns a call into a question:

```ruby
require "proxy"

robot = Rubevy::Proxy.new("robot")
robot.move_to(1, 2)          # => Rubevy.ask("robot.move_to", 1, 2).pop
robot.hp                     # => Rubevy.ask("robot.hp").pop
```

The game answers `"robot.move_to"` as it answers any other kind, and the task is parked on the
answer in the usual way — the call takes as many frames as the answer does. The proxy has a
`kind`, an `inspect` and a `respond_to_missing?` that says yes to everything, so `respond_to?`
agrees with what a call actually does.

**Registration is the plain way; a proxy is for objects the game did not register.** A real
method — `Rubevy.spawn`, `Rubevy.move_to`, or one the host adds with `Vm::define_fn` — says what
it takes, fails at the call when the name is wrong, and costs nothing to look up. A proxy says
nothing about itself: every name is accepted, every mistake becomes a question the game has to
recognise or ignore, and the script finds out at run time. Use it where the thing on the other
side is not rubevy's to register — another robot, an entity of a game that decides its own verbs,
a service reached over the `ask` channel — and register everything else.

It rests on one property of the VM: a `method_missing` written in Ruby is dispatched in the frame
the call was made in, not in a nested run loop, so the body may park the task on `pop` (SabiRuby
`docs/design/fibers.md`, "Native boundaries"). Before that the same file raised
`blocking pop cannot be called from within a C function boundary`.

## What the script sees of the frame

`$rubevy` is refreshed at the head of every frame: `:frame` (the count), `:delta` (seconds since
the last frame) and `:time` (seconds since the start). Reading it is how a script knows where it
is in the run without asking.
The plugin puts it there with `Vm::global_set`, so it is an ordinary global: a script may write
to it, and what it writes stands until the next frame replaces it.

## Time

`sleep` in a script waits in real time: the plugin gives mruby-task's clock Bevy's `Time`
(`task_external_clock` + `task_advance_ticks`), so `sleep 0.5` wakes on the frame about half a
second later. What still comes from the instruction count is the timeslice, which is what keeps
one script from eating a frame — a script that never sleeps is preempted and resumed next frame.

The instruction count cannot stop everything, so each frame also runs under time limits
(`Vm::task_run_limits`, on a clock the plugin gives the VM — Bevy's `Instant`, which a browser
has too):

| `ScriptWorld` field | default | what it does |
|---|---|---|
| `budget` | 200,000 instructions | checked between timeslices, as before |
| `frame_time` | 8 ms | the running timeslice is cut short once the frame's scripts have taken this long |
| `overrun` | 50 ms | a script that cannot be switched out — inside a native waiting for a block, `sort { }` or `Array.new(1) { loop { } }` — gets `Task::Overrun` past this, and the frame comes back |

`Task::Overrun` is an `Exception`, not a `StandardError`, so a script's `rescue => e` does not
keep it going; the script ends with it (`ScriptEnded { status: Failed }`). Timeslices themselves
stay counted in instructions, so what a script does is the same on every machine; the clock only
bounds a frame. A single native that takes long (reversing a 20 MB string) is not interrupted and
is noticed after it returns. `tests/replace.rs` checks that a stuck script no longer holds the
frame (without `overrun`, that test never finishes). The VM side is written up in SabiRuby's
`docs/gems.md`, mruby-task, "Time limits".

## Building against the VM

`Cargo.toml` names the VM from git while the entry points this plugin needs are still being added
(`Vm::task_instructions` and `task_location`, and since 2026-09-15 `define_closure`,
`set_host_state`, `define_fn`, `data_new`, and `task_running` / `ivar_get` / `ivar_set` /
`global_get` / `global_set` / `is_exception`, are newer than the published 0.4.0). Those last six
are what this plugin used to reach into `Vm`'s public fields for, and with them it no longer
touches `vm.heap`, `vm.task` or `vm.globals` anywhere. A clone still builds on its own — cargo fetches it — and it goes back to a crates.io version once the API
settles. To work against a checkout of the VM next to this one, redirect it in a git-ignored
`.cargo/config.toml` instead of editing `Cargo.toml`:

```toml
[patch."https://github.com/sabiruby/sabiruby"]
sabiruby = { path = "../sabiruby" }
sabiruby-compiler = { path = "../sabiruby/compiler" }
```

## What a HUD can show of a script

`ScriptWorld::stats(&ScriptTask)` answers a [`ScriptStats`]: the instructions the script has run
(the difference between two frames is what it spent on that frame), where it stands in its own
source (file and line — while it waits as well as while it runs), and whether it is finished.
It is what makes a panel like "Scout — robots/scout.rb:12 — 4,200 insn" possible, which is the
thing this VM can show and an engine's usual scripting cannot.

## Replacing and removing a script

A game reloads a script by removing the entity's `ScriptTask` and inserting a new `Script`, and
ends one by despawning the entity. Either way `ScriptTask`'s `on_remove` hook terminates the task
in the VM (`Task#terminate`) and lets the collector have it.

Before that hook the task was only forgotten by the ECS: it stayed in the scheduler's queues and
kept running, still carrying its entity, so a reloaded robot had two brains asking for the same
body — the old one invisible, since nothing showed it any more. `tests/replace.rs` checks both
paths (a question already asked when the script is replaced may still be answered once; no new
one is asked).

## Further reading

`rust-bridge.ja.md` walks through one question from a robot in SabiRuby Battle to the system that
answers it and back, and compares each step with what embedding the C mruby would take (in
Japanese).
