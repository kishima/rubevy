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
* `kind` is a string; the arguments after it are numbers, strings or entities ([`Arg`]), and
  `Request::num(i)` / `Request::text(i)` / `Request::entity_arg(i)` read them — each answers
  `None` for an argument of another sort. An answer is nil, a bool, a number, a string, an
  entity, a list of numbers, or a table of them ([`Answer::Rows`] — every robot with its team
  and hp, say). The boundary is deliberately that small: it is enough for a game to ask anything
  and get a table back, without a serialisation format between the two.

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
