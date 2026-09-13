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
| `Rubevy.entity` | the entity this script is attached to (`Entity::to_bits`), or nil |
| `Rubevy.move_to x, y, z` | the script's own entity is moved |
| `Rubevy.set_position entity, x, y, z` | any entity is moved |

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
        let answer = Answer::List(vec![3.0, 3.5]);  // Nil / Bool / Num / Text / List
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
* `kind` is a string; the arguments after it are numbers or strings ([`Arg`]), and
  `Request::num(i)` / `Request::text(i)` read them. An answer is nil, a bool, a number, a string,
  a list of numbers, or a table of them ([`Answer::Rows`] — every robot with its team and hp, say).
  The boundary is deliberately that small: it is enough for a game to ask anything and get a
  table back, without a serialisation format between the two.

## What the script sees of the frame

`$rubevy` is refreshed at the head of every frame: `:frame` (the count), `:delta` (seconds since
the last frame) and `:time` (seconds since the start). Reading it is how a script knows where it
is in the run without asking.

## Time

`sleep` in a script waits in real time: the plugin gives mruby-task's clock Bevy's `Time`
(`task_external_clock` + `task_advance_ticks`), so `sleep 0.5` wakes on the frame about half a
second later. What still comes from the instruction count is the timeslice, which is what keeps
one script from eating a frame — a script that never sleeps is preempted and resumed next frame.

## Building against the VM

`Cargo.toml` names the VM from git while the entry points this plugin needs are still being added
(`Vm::task_instructions` and `task_location` are newer than the published 0.4.0). A clone still
builds on its own — cargo fetches it — and it goes back to a crates.io version once the API
settles. To work against a checkout of the VM next to this one, redirect it in a git-ignored
`.cargo/config.toml` instead of editing `Cargo.toml`:

```toml
[patch."https://github.com/kishima/sabiruby"]
sabiruby = { path = "../sabiruby" }
sabiruby-compiler = { path = "../sabiruby/compiler" }
```

## What a HUD can show of a script

`ScriptWorld::stats(&ScriptTask)` answers a [`ScriptStats`]: the instructions the script has run
(the difference between two frames is what it spent on that frame), where it stands in its own
source (file and line — while it waits as well as while it runs), and whether it is finished.
It is what makes a panel like "Scout — robots/scout.rb:12 — 4,200 insn" possible, which is the
thing this VM can show and an engine's usual scripting cannot.
