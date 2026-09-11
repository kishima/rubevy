# rubevy

Run mruby bytecode inside [Bevy](https://bevy.org/) (0.19), using the
[SabiRuby](../sabiruby) VM.

## What works (2026-09-11, v0)

* `.mrb` files (compiled with mruby 4.1's `mrbc`) load as `MrbAsset` through Bevy's asset server.
* `Script` component: one VM per entity, created when the asset arrives, stepped every
  frame with an instruction budget so a busy script cannot stall a frame.
* The script reads `$frame` and `$delta`; `puts`/`p` go to Bevy's log; a `ScriptEnded`
  message reports completion or an uncaught exception.

Not yet: a host API (spawning entities, reading components, events), hot reload,
sharing objects between scripts, Fiber-based coroutines.

## Try it

`Cargo.toml` depends on SabiRuby by path, so clone both repositories side by side
(`kishima/sabiruby` and `kishima/rubevy` in the same parent directory).

```
tools/compile_scripts.sh          # assets/scripts/*.rb -> .mrb (Docker, reference mrbc)
cargo run --example headless      # MinimalPlugins + AssetPlugin + RubevyPlugin, no window
```

```rust
use rubevy::{MrbAsset, RubevyPlugin, Script};

app.add_plugins(RubevyPlugin);
// in a system:
let mrb: Handle<MrbAsset> = server.load("scripts/hello.mrb");
commands.spawn(Script::new(mrb).with_budget(5_000));
```

## License

MIT.
