# rubevy

Run mruby bytecode inside [Bevy](https://bevy.org/) (0.19), using the
[SabiRuby](https://crates.io/crates/sabiruby) VM.

## What works (2026-09-15, v1: tasks)

* `.mrb` files (compiled with mruby 4.1's `mrbc`) load as `MrbAsset` through Bevy's asset server.
* **One VM for the app, one task per script.** A `Script` component becomes a task of
  mruby-task's scheduler; every frame the plugin moves the scheduler's clock on by the
  frame time and lets the ready tasks run for a budget of instructions. A task that
  calls `sleep` costs nothing until its time comes, and one that never yields is
  preempted at its timeslice, so a frame cannot be lost to a runaway script.
* **Priorities**: `Script::with_priority` (0 first, 128 by default), mruby-task's.
* **A host API**: `Rubevy.log`, `.spawn`, `.despawn`, `.set_position`, `.move_to`, and
  `.entity` (the entity this script is attached to, as a `Rubevy::Entity` object that
  carries `Entity::to_bits` as a handle — `to_i` for the number, `==` by it). A native
  cannot touch the Bevy world, so these leave a command behind — on a queue inside the
  VM (`Vm::set_host_state`), one per `App` — and a system carries it out after the
  frame's scripts have run, the same promise `Commands` makes.
* **A question the game answers when it can**: `Rubevy.ask("scan", 40)` parks the script's
  task on a queue, a system of the game takes the request and answers it on that frame or a
  later one. Where the answer is work rather than a lookup, `ScriptWorld::answer_with` takes
  a `Future` instead: it goes to Bevy's task pool and the plugin answers the script on the
  frame it finishes. Bevy's pools are threads only with bevy's own `multi_threaded` feature —
  this crate does not ask for it, the app does (`docs/host-api.md`).
* **A question written as a call**: `require "proxy"` gives a script `Rubevy::Proxy`, whose
  `robot.move_to(1, 2)` is `Rubevy.ask("robot.move_to", 1, 2).pop`. Registering a real method
  is the plain way; a proxy is for objects the game did not register (`docs/host-api.md`).
* **Components by name**: `e.get(:Transform)` answers a Hash of the component's fields,
  `e[:Transform] = tf` writes back the fields the Hash names, and `e.has?`, `e.components`
  and `Rubevy.find(:Npc)` say what is where. It goes through Bevy's reflection, so no type
  is named in rubevy: a game's own component joins in with `#[derive(Reflect)]`,
  `#[reflect(Component)]` and `register_type` (`docs/host-api.md`).
* **Events**: `Rubevy.subscribe(:hit)` answers a queue the game pushes onto
  (`ScriptWorld::publish`), so a script waits for something to happen exactly as it waits
  for an answer — in its own task, or in one it made with `Task.new`, which is what a reflex
  wants. A game joins one of Bevy's events with one observer.
* The script reads `$rubevy` (`:frame`, `:delta`, `:time`); `puts`/`p` go to Bevy's log;
  a `ScriptEnded` message carries what the task answered, or the exception it did not
  handle (mruby-task makes that the task's result, so one broken script does not stop
  the others).
* **`require`** reads from the asset directory: `.mrb` always, `.rb` in a build with the
  `ruby-source` feature (which brings the reference compiler along).
* The collector runs at the scheduler's idle points (`GC.scheduler_driven`).

Scripts share the VM, so they share globals and constants. That is the design; a use
that needs isolation wants a second VM, which this plugin does not build yet.

Not yet: building a component from Ruby to spawn with, queries as blocks
(`each(:Enemy, :Transform) { }`), hot reload that keeps a script's state, a heap
cap (`docs/outlook.md`).

How a script and the game actually meet — `Rubevy.ask`, answering from a system, the clock, reading
where a script stands, stopping it — and what that gains over embedding the C mruby, is written up
in Japanese in [`docs/rust-bridge.ja.md`](docs/rust-bridge.ja.md).

## Try it

The VM comes from git (`sabiruby`, main), so a clone of this repository alone builds. To
work against a checkout of `sabiruby/sabiruby` next to this one, redirect it in
`.cargo/config.toml` (git-ignored) instead of editing `Cargo.toml`:

```toml
[patch."https://github.com/sabiruby/sabiruby"]
sabiruby = { path = "../sabiruby" }
sabiruby-compiler = { path = "../sabiruby/compiler" }
```

```
tools/compile_scripts.sh          # the .rb that ships here -> .mrb (Docker, reference mrbc)
cargo run --example headless      # MinimalPlugins + AssetPlugin + RubevyPlugin, no window
cargo run --example sensor        # Rubevy.ask, answered by a system two frames later
cargo run --example async         # Rubevy.ask, answered from a future on the task pool,
                                  # then the same question written as a call on a proxy
cargo run --example components    # a script reads its own Transform, moves it, writes it back
cargo run --example events        # an observer publishes to a queue; a reflex task waits on it
```

```rust
use rubevy::{MrbAsset, RubevyPlugin, Script};

app.add_plugins(RubevyPlugin::default());   // or ::with_asset_root("assets")
// in a system:
let mrb: Handle<MrbAsset> = server.load("scripts/npc.mrb");
commands.spawn((Script::new(mrb).with_name("npc").with_priority(100), Transform::default()));
```

```ruby
# assets/scripts/npc.rb
loop do
  Rubevy.move_to($rubevy[:time].sin * 5, 0, 0)
  sleep 0.1          # the task is off the CPU until then
end
```

## License

MIT.

## Outlook

What rubevy will build on SabiRuby's planned features, why it fits Bevy, and an honest assessment: [`docs/outlook.md`](docs/outlook.md) (日本語の平易版: [`docs/outlook.ja.md`](docs/outlook.ja.md)).
