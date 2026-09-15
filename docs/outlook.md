# Outlook: what rubevy will build on SabiRuby, and how it fits Bevy

Written 2026-09-12 from the SabiRuby plans (`sabiruby/docs/`: `eval-require-plan.md` for
the `Host` trait, `gems.md` for mruby-task, `utf8-plan.md`, and
`sabiruby-playground/docs/visualizer-plan.md` for snapshot/trace), and brought up to date on
2026-09-14. The Japanese companion is `outlook.ja.md`; how a script and the game actually meet,
and what that gains over embedding the C mruby, is `rust-bridge.ja.md` (Japanese).

**rubevy today (v1, 2026-09-14):** one VM for the app (`ScriptWorld`, a plain Bevy resource —
`Vm` is `Send + Sync`), one mruby-task task per `Script` entity with a priority, `sleep` on
Bevy's `Time`, a budget of instructions and time limits per frame, `Rubevy.ask` for a script to
ask the game something and wait, `ScriptStats` for what a script spends and where it stands,
the task terminated when its `ScriptTask` is removed or the entity despawned, `puts` to the log
and a `ScriptEnded` message. The game that uses all of it is SabiRuby Battle
(`sabiruby/rubevy_games`), which also runs in the browser: https://sabiruby.github.io/rubevy_games/

## Status at a glance (2026-09-14)

| | item | state |
|---|---|---|
| bridge 1 | `Host` | **partly**: `compile` (feature `ruby-source`) and `read_file` from the asset directory; the scheduler's clock from Bevy. Randomness is the VM's own (fixed seed or `srand`), not Bevy's RNG |
| bridge 2 | hot reload | **in the restart form**: a script is replaced by removing its `ScriptTask` and inserting a new `Script` (the old task is terminated). SabiRuby Battle reloads on file save and applies editor text in memory. Redefining methods in place is not done |
| bridge 3 | ECS bridge | **the deferred-write half**, plus `Rubevy.ask` (request/answer, not in the original list). `Data` objects are not done |
| bridge 4 | reflection | not started |
| bridge 5 | coroutine-style scripts | **done in task form** (`sleep`, waiting on `ask(...).pop`) |
| bridge 6 | mruby-task | **done**, with time limits and `Task::Overrun` since 2026-09-14 |
| bridge 7 | events | not started (`ScriptEnded` is the only message) |
| bridge 8 | GC in slices | **the timing half**: collections at the scheduler's idle points (`GC.scheduler_driven`). Still stop-the-world; no `gc_step` |
| bridge 9 | in-game debugger | **the playground has the inspector**; in a game, what a script spends and the lines it keeps returning to |
| bridge 10 | text | **done** (UTF-8 strings, feature `utf8`, default on) |
| bridge 11 | web | **done**: SabiRuby Battle's browser build, the compiler as a second wasm module |
| possibility 1 | live image | **partly** (in-game editor, reload) |
| possibility 2 | snapshots | not started (determinism is in place) |
| possibility 3 | one cartridge, three machines | **two of three** (PC and browser) |
| possibility 4 | safe user and AI content | **mostly** (budgets and time limits; no heap cap) |
| possibility 5 | thousands of small minds | **done** |
| possibility 6 | learning by seeing the machine | **done in the playground** |
| possibility 7 | prototype in CRuby | unchanged |

## What rubevy builds (the bridge), in order

1. **`Host`.** rubevy implements the VM's `Host` trait: `compile` with `sabiruby-compiler`
   (a feature; web builds ship `.mrb`), `read_file` from Bevy assets, the clock from
   `Time`, randomness from Bevy's RNG. `.rb` files become assets and `require` works
   across them.
   *Now:* `FileHost` compiles behind `ruby-source` and reads `require`d files from the asset
   directory on the file system (a packed or remote asset source is not served). The clock is
   two things: mruby-task's ticks follow Bevy's `Time` (`task_external_clock`,
   `task_advance_ticks`), and a monotonic clock on Bevy's `Instant` measures the time limits
   (`task_set_clock`). Randomness is not taken from the host. SabiRuby Battle compiles its
   `.rb` in the game (in the browser, through the playground's compiler module).
2. **Hot reload.** Bevy's asset watcher → recompile. First version restarts the VM
   (state lost); with `eval`/`load`, redefine methods in place and keep state — reopening
   a class is ordinary Ruby, the method just points at a new irep.
   *Now:* the restart form, per script rather than per VM: removing `ScriptTask` and inserting
   a new `Script` starts the script over, and an `on_remove` hook terminates the old task
   (before that hook the old one kept running unseen; `tests/replace.rs`). SabiRuby Battle
   watches its Ruby directory with `notify`, and its editor applies text to one robot in
   memory. Redefining in place, keeping state, is still to do.
3. **ECS bridge.** Ruby objects wrapping Rust values (mruby's `RData`; an
   `ObjKind::Data` with `Drop`) carry `Entity`, components and resources. `&mut World` is
   lent to the `Host` only while an exclusive system steps VMs; writes from Ruby are
   deferred like `Commands` and applied after the step. The VM's "never re-enter" rule is
   what makes this safe: a native returns at once and never holds the world across Ruby.
   *Now:* the deferred writes exist (`Rubevy.spawn`, `.despawn`, `.set_position`, `.move_to`,
   `.entity`, through a command queue a system drains). Next to them, a shape the list did not
   foresee and a whole game now stands on: `Rubevy.ask(kind, *args)` returns a queue the script
   `pop`s, a system of the game takes the requests (`ScriptWorld::take_requests`), answers
   from ordinary queries, and hands back numbers, strings, lists or tables
   (`ScriptWorld::answer`). The rules of the game stay in Rust; Ruby only asks. Wrapping
   `Entity` in a Ruby object is done since 2026-09-15 (`Rubevy::Entity`, a `Data` object that
   carries the entity's bits; `Answer::Entity`, `Arg::Entity`), and natives can be closures with
   the command queue in the VM's typed host state, so there is no `static` any more.
4. **Reflect, no per-type glue.** With `Reflect`/`ReflectComponent`, field names and
   types are known at run time, so Ruby can do `entity[:Transform].translation.x = 1.0`
   without hand-written bindings. Ruby's dynamic access and Bevy's reflection are the same
   idea from two sides — the strongest fit.
   *Now:* not started.
5. **Coroutine-style scripts.** Fibers give `sleep 0.5`, `wait_until { }`, `move_to(x, y)`
   that span frames (the feel of Unity coroutines / Godot `await`). The VM already has
   fibers and `step`; rubevy adds only "resume the yielded fiber next frame".
   *Now:* done through tasks rather than raw fibers (the two cannot be mixed across a task
   switch; see the VM's `gems.md`). `sleep` waits in real time, and waiting on `ask(...).pop`
   costs nothing until the game answers — a robot's brain is a plain loop with no callbacks.
6. **mruby-task.** Many scripts in one VM with priorities; the tick is the frame and the
   loop is `run_once` per frame, not the blocking `Task.run`. Keeps the one-VM-per-entity
   option (stronger isolation, more memory) next to one-VM-many-tasks.
   *Now:* done, one VM with many tasks (a second VM for isolation is not built). Each frame
   runs under `task_run_limits`: the instruction budget, a time budget (`frame_time`, 8 ms)
   that cuts the running timeslice short, and an overrun limit (`overrun`, 50 ms) past which a
   task that cannot be switched out — inside a native waiting for a block — gets
   `Task::Overrun`. Timeslices stay counted in instructions, so what a script does is the same
   on every machine. `ScriptWorld::stats` reads what a task has spent and every frame it
   stands in.
7. **Events.** Bevy events/observers delivered to Ruby blocks (`on(:collision) { |a, b| }`)
   through `call_block`; Ruby exceptions become a log line and a `ScriptError` event.
   *Now:* not started. An exception a script does not handle ends its task and becomes a
   `ScriptEnded { status: Failed }` message; there is no `ScriptError`.
8. **GC in slices.** A `gc_step(work)` entry on the VM (the book's `mrb_gc_step` shape;
   today's collector is stop-the-world) so a frame never pays a whole collection; stress
   mode in development to find missing roots early.
   *Now:* the collector is still stop-the-world, but it runs at the scheduler's idle points
   (`GC.scheduler_driven`) rather than inside an allocation, with a debt limit for a scheduler
   that never idles. Stress mode exists. `gc_step` does not.
9. **In-game debugger.** The playground's snapshot/trace is JSON; a `bevy_egui` window can
   show frames, registers, environments, fibers and GC without an editor.
   *Now:* the playground's inspector exists (stepping, named registers, environments, catch
   tables, fibers, heap and GC). In SabiRuby Battle an egui editor shades the lines a robot's
   brain keeps returning to, and the scoreboard shows the instructions each spends a frame.
   The inspector in a game window is still to do.
10. **Text.** UTF-8 strings (feature `utf8`, default on) for `bevy_text`.
    *Now:* done in the VM.
11. **Web.** Bevy's wasm build takes SabiRuby as is (no_std, wasm32 in CI); the compiler
    needs wasi-sdk, so on the web either ship `.mrb` or compile in a Worker.
    *Now:* done. SabiRuby Battle builds for the browser from the same code as the PC version
    (`web/build.sh`, published on GitHub Pages). Ruby is compiled in the page by the
    playground's `wasm32-wasip1` compiler module loaded beside the game, called synchronously
    from the game (`window.sabibotsCompile`), not in a Worker (`rubevy_games/docs/web.md`).

## Why it fits

* **Budgeted runs ↔ the frame loop.** The VM has no loop of its own, so it is one
  system in Bevy's schedule: `task_run_limits` once a frame.
* **No C stack, no longjmp ↔ borrows and `Result`.** A host call never re-enters the VM,
  so `&mut World` can be lent. The boundary rule that remains is the reference's: a task
  cannot be switched out while a native waits for a block, and a fiber cannot be switched
  across one. Under time limits the first no longer holds the frame (`Task::Overrun`).
* **VM state is plain data ↔ Reflect and inspectors.** Bevy's type info and Ruby's dynamic
  access pair up; the debugger reads the same JSON, and a game reads `task_frames`.
* **Tasks ↔ gameplay written along time.**
* **Compiler as a crate ↔ assets and hot reload.**
* **no_std ↔ web and embedded** with the same VM.

## What the VM must add for this

* `Host` — **done** (compile, read_file, file_exists).
* `Vm: Send + Sync` — **done** (`tests/send_sync.rs`); `ScriptWorld` is an ordinary resource.
* A deadline-based budget next to the instruction count — **done** (2026-09-14): a host clock
  (`task_set_clock`), `Timeslice::{Instructions, Time, Both}`, `RunLimits`, `Task::Overrun`. The
  clock is read at the existing instruction tick and every 32 natives, so benchmarks without a
  clock stayed within noise.
* `Data` objects with a free hook — **done** (2026-09-15, `ObjKind::Data`, `set_on_free`); rubevy's `Rubevy::Entity` uses it.
* A typed host state reachable from natives (instead of a `static`) — **done** (2026-09-15, `set_host_state`, `define_closure`).
* `gc_step(work)` — to do.
* A heap cap — to do (possibility 4).

## How good is this, honestly

Measured against what Bevy users already have, most of the list is **parity, not
novelty**. Lua through `mlua`, and Rhai/Rune, are established Bevy scripting choices
(`bevy_mod_scripting` — from memory, not re-checked here — already does
reflection-based bindings, hot reload, and coroutines are native to Lua); Lua has an
incremental GC and debug hooks, and a Lua VM is faster than mruby, which is itself faster
than SabiRuby today (fib 3.5× slower than the reference). Items 1–9 bring rubevy to that
level; they do not pass it.

What is genuinely distinctive is smaller and specific:

* **Ruby as the scripting language for Bevy.** Nothing offers it today; the audience is
  Rubyists and the mruby community (large in Japan), not Bevy users in general.
* **mruby bytecode compatibility.** `mrbc`, PicoRuby's gems and the reference test suite
  are reusable, and correctness is measurable (2344 of 2507 of mruby's own tests and the
  ported gems' tests, the rest with reasons) rather than asserted — most scripting bindings
  cannot say that about their language.
* **A VM designed for hosting.** Result-based unwinding, data-only state, budgeted
  runs and a VM that is an ordinary `Send + Sync` value make embedding simpler than mruby's C
  API (`mrb_state`, `setjmp`, arena) — this is an engineering quality argument, not a
  feature. `rust-bridge.ja.md` walks through it point by point; the VM crate has one `unsafe`.
* **The book and the kit.** The VM is explained and verifiable end to end, which matters
  for people who want to understand or modify their scripting layer.

Weak points to state plainly: performance (interpreter speed, and whole collections until
`gc_step`; the time limits bound a frame, they do not make scripts faster), maturity (v1,
one game), no Ruby ecosystem beyond mruby's gems, and one more language to learn for a Bevy
team. A fair summary: **rubevy is the right choice for someone who wants Ruby in Bevy, and a
reasonable one for someone who wants a small, inspectable, verifiable scripting VM; it is not
a reason to leave Lua.**

## Direction (author, 2026-09-12)

Do not compete with Lua on speed; the languages are different. rubevy's strength is
**Ruby as a DSL**: scripts describe things (entities and bundles, scenes, state machines,
behaviour trees, timelines, dialogue, tuning tables) and Rust systems run them. Ruby's
tools for that — blocks, `instance_eval`/`class_eval` with a block, `method_missing`,
`define_method`, keyword arguments, `Module#included`-style hooks — are already in the
VM (mruby-metaprog is ported). The per-frame path stays in Rust; Ruby runs at
declaration time, on events, and in coroutines that yield most frames. Design the ECS
bridge for that shape first (build once, apply as `Commands`), not for per-frame
component reads.

SabiRuby Battle is that direction in practice (2026-09-14): `robot "Scout" do … end` and
`match "Training", noise: 0.3 do … end` are DSLs built with `Class.new` and `class_eval`, the
robot's brain decides and asks, and the physics, the damage and the rules are Rust systems.

## Possibilities (the ambitious version, 2026-09-12)

Each of these follows from one property the VM already has or has planned; none is
free, but none needs a new kind of VM.

1. **The game as a live image.** Compiler in the engine + `eval`/`load` + state as data:
   edit Ruby while the game runs, redefine a method and the NPC changes behaviour
   mid-animation; an in-game REPL that talks to entities. Sonic Pi proved that a Ruby DSL
   is a live-coding instrument; rubevy can be that for visuals and play. Needs: `Host`,
   eval, the debugger pane (planned).
   *Now (partly):* SabiRuby Battle edits a robot's brain in the game and applies it with F5,
   to that robot only and in memory; saving a file restarts the robots on it; the browser
   build has the same editor. The pieces under it (`Host`, `eval`, `require`/`load`, compiling
   in the game) are in. What restarts is the task, so its state is not kept; redefining a
   method while it keeps running, and a REPL, are still to do.
2. **Snapshot the whole script state.** Registers, frames, heap, fibers are plain data,
   so a VM can be serialized: save games that include coroutines mid-`sleep`, rewind as
   a mechanic, replays, deterministic lockstep netcode with state hashes (the VM is
   no_std and takes time only from the host, so runs are reproducible by
   construction). Needs: `Vm` serialization (a walk over `heap`/`contexts`; ~1 week), a
   rule for `Data` objects.
   *Now:* serialization is not started. Determinism is in place: timeslices are counted in
   instructions by default; the time limits are a safety net that normal play does not reach;
   a time-only timeslice exists and gives determinism up, by choice. SabiRuby Battle's matches
   take a `seed:` and roll the same dice, but frame times still vary, so a replay is alike
   rather than identical — a fixed timestep in the game is part of what this needs.
3. **One cartridge, three machines.** The same `.mrb` runs in Bevy on a PC, in the
   browser (Bevy wasm, or the playground), and on a microcontroller-class device — the
   author's own hardware line (family-mruby) is the obvious third target. A Ruby fantasy
   console: write once, play on the desk, in a link, and in the hand. Needs: the
   embedded target of SabiRuby (thumbv7em builds in CI already), a small display API in
   `Host`.
   *Now (two of three):* SabiRuby Battle is one codebase built for the PC and for the browser,
   the difference kept in one module chosen by target (`sabibots/src/platform.rs`); the
   browser build is public. The microcontroller side is still only the VM building for
   `thumbv7em-none-eabi` in CI.
4. **Safe user and AI content.** Instruction budgets, a heap the host can cap, no I/O
   except through `Host`: a mod or an LLM-written script cannot hang the frame, exhaust
   memory or touch files. LLMs write Ruby well; an NPC brain can be generated at play
   time, compiled by the in-engine compiler, and run as a task with a budget. The test
   suite and the book are what make "safe" a claim with evidence. Needs: heap cap,
   per-task budgets (task), a `Host` policy for what scripts may reach.
   *Now (mostly):* a script cannot hang the frame. Instruction-counted timeslices stop an
   endless loop; time limits stop what they cannot (`Array.new(1) { loop { } }` used to freeze
   SabiRuby Battle; now that robot gets `Task::Overrun`, which a plain `rescue` does not
   swallow, and the match goes on); a replaced or removed script's task is terminated; an
   exception ends only its own script. Still to do: a heap cap, interrupting a single native
   that takes long (it is noticed when it returns), and the host policy.
5. **Thousands of small minds.** mruby-task + fibers with tiny budgets: every NPC a Ruby
   task, scheduled by priority, sleeping most frames. VMs are small (no_std), so one per
   faction or one per entity are both affordable. Needs: task (planned), `gc_step`.
   *Now (done, 2026-09-13):* rubevy v1 is this shape, and SabiRuby Battle runs a match script
   (priority 10) and four robot brains (priority 100) as tasks of one VM, each asking the game
   and waiting at no cost. Several tasks inside one robot (a `reflex` block on a hit) is the
   game's next step.
6. **Learning by seeing the machine.** The playground's visualizer in the game window:
   beginners write Ruby, see entities move, and can open the VM to see the registers and
   the frames. Ruby is a teaching language in Japan; a game engine with a transparent VM
   is a course, not just a tool.
   *Now (done in the playground):* https://sabiruby.github.io/sabiruby-playground/ shows code,
   AST, bytecode and result side by side, and steps the VM showing named registers,
   environments, catch tables, fibers, heap and GC. In a game, the editor shading and the
   instruction count per frame come from `task_frames` and `task_instructions`. The
   visualizer in a game window is still to do.
7. **Prototype in CRuby, ship in Bevy.** The scripting subset is mruby's, so gameplay
   rules can be prototyped and unit-tested with CRuby's tooling, then run unchanged in
   the engine (`tests/custom` already compares against CRuby where mruby's semantics
   agree).
   *Now:* unchanged. SabiRuby Battle's DSL would run in CRuby with `Rubevy.ask` stubbed, but
   the tooling for it is not built.

The thread through all of them: the VM is a *value* (data, deterministic, budgeted),
not a process. That is what Lua embeddings do not give you cheaply, and it is where
rubevy can be more than "Ruby in Bevy".
