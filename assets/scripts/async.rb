# Compiled to async.mrb by tools/compile_scripts.sh (reference mrbc 4.1.0-rc).
# The game answers this question from a Future (`ScriptWorld::answer_with`, examples/async.rs):
# the work goes to Bevy's task pool and the answer comes back several frames later. Nothing
# here says so — a question answered from a thread and one answered by a system are the same
# `ask` and the same `pop` — and the task is parked either way, so the ticker keeps running.

Rubevy.log "async: asking for a path to (8, 3) at frame #{$rubevy[:frame]}"
steps = Rubevy.ask("path", 8.0, 3.0).pop
Rubevy.log "async: the path arrived at frame #{$rubevy[:frame]}: #{steps.length} steps, ending at #{steps.last.inspect}"

:async_done
