# Outlook: what rubevy will build on SabiRuby, and how it fits Bevy

Written 2026-09-12 from the SabiRuby plans (`sabiruby/docs/`: `eval-require-plan.md` for
the `Host` trait, `gems.md` for mruby-task, `utf8-plan.md`, and
`sabiruby-playground/docs/visualizer-plan.md` for snapshot/trace). rubevy v0 today: `.mrb`
assets, one VM per `Script` entity, stepped each frame with an instruction budget,
`$frame`/`$delta`, `puts` to the log, a `ScriptEnded` event.

## What rubevy builds (the bridge), in order

1. **`Host`.** rubevy implements the VM's `Host` trait: `compile` with `sabiruby-compiler`
   (a feature; web builds ship `.mrb`), `read_file` from Bevy assets, the clock from
   `Time`, randomness from Bevy's RNG. `.rb` files become assets and `require` works
   across them.
2. **Hot reload.** Bevy's asset watcher → recompile. First version restarts the VM
   (state lost); with `eval`/`load`, redefine methods in place and keep state — reopening
   a class is ordinary Ruby, the method just points at a new irep.
3. **ECS bridge.** Ruby objects wrapping Rust values (mruby's `RData`; an
   `ObjKind::Data` with `Drop`) carry `Entity`, components and resources. `&mut World` is
   lent to the `Host` only while an exclusive system steps VMs; writes from Ruby are
   deferred like `Commands` and applied after the step. The VM's "never re-enter" rule is
   what makes this safe: a native returns at once and never holds the world across Ruby.
4. **Reflect, no per-type glue.** With `Reflect`/`ReflectComponent`, field names and
   types are known at run time, so Ruby can do `entity[:Transform].translation.x = 1.0`
   without hand-written bindings. Ruby's dynamic access and Bevy's reflection are the same
   idea from two sides — the strongest fit.
5. **Coroutine-style scripts.** Fibers give `sleep 0.5`, `wait_until { }`, `move_to(x, y)`
   that span frames (the feel of Unity coroutines / Godot `await`). The VM already has
   fibers and `step`; rubevy adds only "resume the yielded fiber next frame".
6. **mruby-task.** Many scripts in one VM with priorities; the tick is the frame and the
   loop is `run_once` per frame, not the blocking `Task.run`. Keeps the one-VM-per-entity
   option (stronger isolation, more memory) next to one-VM-many-tasks.
7. **Events.** Bevy events/observers delivered to Ruby blocks (`on(:collision) { |a, b| }`)
   through `call_block`; Ruby exceptions become a log line and a `ScriptError` event.
8. **GC in slices.** A `gc_step(work)` entry on the VM (the book's `mrb_gc_step` shape;
   today's collector is stop-the-world) so a frame never pays a whole collection; stress
   mode in development to find missing roots early.
9. **In-game debugger.** The playground's snapshot/trace is JSON; a `bevy_egui` window can
   show frames, registers, environments, fibers and GC without an editor.
10. **Text.** UTF-8 strings (feature `utf8`, default on) for `bevy_text`.
11. **Web.** Bevy's wasm build takes SabiRuby as is (no_std, wasm32 in CI); the compiler
    needs wasi-sdk, so on the web either ship `.mrb` or compile in a Worker.

## Why it fits

* **Budgeted `step` ↔ the frame loop.** The VM has no loop of its own, so it is one
  system in Bevy's schedule.
* **No C stack, no longjmp ↔ borrows and `Result`.** A host call never re-enters the VM,
  so `&mut World` can be lent; fibers need no "C boundary" rule.
* **VM state is plain data ↔ Reflect and inspectors.** Bevy's type info and Ruby's dynamic
  access pair up; the debugger reads the same JSON.
* **Fibers/tasks ↔ gameplay written along time.**
* **Compiler as a crate ↔ assets and hot reload.**
* **no_std ↔ web and embedded** with the same VM.

## What the VM must add for this

`Host` (planned), `Data` objects with `Drop`, `gc_step(work)`, a deadline-based budget
next to the instruction count (the `Host` clock), and `Vm: Send` (Bevy resources are
`Send + Sync` by default; a VM of `Vec`s and `fn` pointers with `Box<dyn Host + Send>`
should be — to be checked).

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
  are reusable, and correctness is measurable (1185/1227 of mruby's own tests) rather than
  asserted — most scripting bindings cannot say that about their language.
* **A VM designed for hosting.** Result-based unwinding, data-only state, budgeted
  stepping and no global state make embedding simpler than mruby's C API (`mrb_state`,
  `setjmp`, arena) — this is an engineering quality argument, not a feature.
* **The book and the kit.** The VM is explained and verifiable end to end, which matters
  for people who want to understand or modify their scripting layer.

Weak points to state plainly: performance (interpreter speed and GC pauses until
`gc_step`), maturity (v0, one example), no Ruby ecosystem beyond mruby's gems, and one
more language to learn for a Bevy team. A fair summary: **rubevy is the right choice for
someone who wants Ruby in Bevy, and a reasonable one for someone who wants a small,
inspectable, verifiable scripting VM; it is not a reason to leave Lua.**

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

## Possibilities (the ambitious version, 2026-09-12)

Each of these follows from one property the VM already has or has planned; none is
free, but none needs a new kind of VM.

1. **The game as a live image.** Compiler in the engine + `eval`/`load` + state as data:
   edit Ruby while the game runs, redefine a method and the NPC changes behaviour
   mid-animation; an in-game REPL that talks to entities. Sonic Pi proved that a Ruby DSL
   is a live-coding instrument; rubevy can be that for visuals and play. Needs: `Host`,
   eval, the debugger pane (planned).
2. **Snapshot the whole script state.** Registers, frames, heap, fibers are plain data,
   so a VM can be serialized: save games that include coroutines mid-`sleep`, rewind as
   a mechanic, replays, deterministic lockstep netcode with state hashes (the VM is
   no_std and takes time and randomness only through `Host`, so runs are reproducible by
   construction). Needs: `Vm` serialization (a walk over `heap`/`contexts`; ~1 week), a
   rule for `Data` objects.
3. **One cartridge, three machines.** The same `.mrb` runs in Bevy on a PC, in the
   browser (Bevy wasm, or the playground), and on a microcontroller-class device — the
   author's own hardware line (family-mruby) is the obvious third target. A Ruby fantasy
   console: write once, play on the desk, in a link, and in the hand. Needs: the
   embedded target of SabiRuby (thumbv7em builds in CI already), a small display API in
   `Host`.
4. **Safe user and AI content.** Instruction budgets, a heap the host can cap, no I/O
   except through `Host`: a mod or an LLM-written script cannot hang the frame, exhaust
   memory or touch files. LLMs write Ruby well; an NPC brain can be generated at play
   time, compiled by the in-engine compiler, and run as a task with a budget. The test
   suite and the book are what make "safe" a claim with evidence. Needs: heap cap,
   per-task budgets (task), a `Host` policy for what scripts may reach.
5. **Thousands of small minds.** mruby-task + fibers with tiny budgets: every NPC a Ruby
   task, scheduled by priority, sleeping most frames. VMs are small (no_std), so one per
   faction or one per entity are both affordable. Needs: task (planned), `gc_step`.
6. **Learning by seeing the machine.** The playground's visualizer in the game window:
   beginners write Ruby, see entities move, and can open the VM to see the registers and
   the frames. Ruby is a teaching language in Japan; a game engine with a transparent VM
   is a course, not just a tool.
7. **Prototype in CRuby, ship in Bevy.** The scripting subset is mruby's, so gameplay
   rules can be prototyped and unit-tested with CRuby's tooling, then run unchanged in
   the engine (`tests/custom` already compares against CRuby where mruby's semantics
   agree).

The thread through all of them: the VM is a *value* (data, deterministic, budgeted),
not a process. That is what Lua embeddings do not give you cheaply, and it is where
rubevy can be more than "Ruby in Bevy".
