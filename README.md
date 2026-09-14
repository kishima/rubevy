# rubevy

Run mruby bytecode inside [Bevy](https://bevy.org/) (0.19), using the
[SabiRuby](https://crates.io/crates/sabiruby) VM.

## What works (2026-09-13, v1: tasks)

* `.mrb` files (compiled with mruby 4.1's `mrbc`) load as `MrbAsset` through Bevy's asset server.
* **One VM for the app, one task per script.** A `Script` component becomes a task of
  mruby-task's scheduler; every frame the plugin moves the scheduler's clock on by the
  frame time and lets the ready tasks run for a budget of instructions. A task that
  calls `sleep` costs nothing until its time comes, and one that never yields is
  preempted at its timeslice, so a frame cannot be lost to a runaway script.
* **Priorities**: `Script::with_priority` (0 first, 128 by default), mruby-task's.
* **A host API**: `Rubevy.log`, `.spawn`, `.despawn`, `.set_position`, `.move_to`, and
  `.entity` (the entity this script is attached to). A native cannot touch the Bevy
  world, so these leave a command behind and a system carries it out after the frame's
  scripts have run — the same promise `Commands` makes.
* The script reads `$rubevy` (`:frame`, `:delta`, `:time`); `puts`/`p` go to Bevy's log;
  a `ScriptEnded` message carries what the task answered, or the exception it did not
  handle (mruby-task makes that the task's result, so one broken script does not stop
  the others).
* **`require`** reads from the asset directory: `.mrb` always, `.rb` in a build with the
  `ruby-source` feature (which brings the reference compiler along).
* The collector runs at the scheduler's idle points (`GC.scheduler_driven`).

Scripts share the VM, so they share globals and constants. That is the design; a use
that needs isolation wants a second VM, which this plugin does not build yet.

Not yet: reading components other than the ones above, events, hot reload, the
reflection bridge (`docs/outlook.md`).

How a script and the game actually meet — `Rubevy.ask`, answering from a system, the clock, reading
where a script stands, stopping it — and what that gains over embedding the C mruby, is written up
in Japanese in [`docs/rust-bridge.ja.md`](docs/rust-bridge.ja.md).

## Try it

The VM comes from crates.io (`sabiruby` 0.3), so a clone of this repository alone builds. To
work against a checkout of `kishima/sabiruby` next to this one, redirect it in
`.cargo/config.toml` (git-ignored) instead of editing `Cargo.toml`:

```toml
[patch.crates-io]
sabiruby = { path = "../sabiruby" }
sabiruby-compiler = { path = "../sabiruby/compiler" }
```

```
tools/compile_scripts.sh          # assets/scripts/*.rb -> .mrb (Docker, reference mrbc)
cargo run --example headless      # MinimalPlugins + AssetPlugin + RubevyPlugin, no window
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
