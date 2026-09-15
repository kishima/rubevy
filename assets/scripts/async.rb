# Compiled to async.mrb by tools/compile_scripts.sh (reference mrbc 4.1.0-rc).
# The game answers this question from a Future (`ScriptWorld::answer_with`, examples/async.rs):
# the work goes to Bevy's task pool and the answer comes back several frames later. Nothing
# here says so — a question answered from a thread and one answered by a system are the same
# `ask` and the same `pop` — and the task is parked either way, so the ticker keeps running.

require "proxy"

Rubevy.log "async: asking for a path to (8, 3) at frame #{$rubevy[:frame]}"
steps = Rubevy.ask("path", 8.0, 3.0).pop
Rubevy.log "async: the path arrived at frame #{$rubevy[:frame]}: #{steps.length} steps, ending at #{steps.last.inspect}"

# The same question, written as a call. `robot` stands for something the game owns, and
# `move_to` is not a method anywhere — `Rubevy::Proxy` turns it into `Rubevy.ask("robot.move_to",
# 1, 2).pop`, so this line parks the task exactly as the one above does.
robot = Rubevy::Proxy.new("robot")
Rubevy.log "async: #{robot.inspect} is at #{robot.move_to(1, 2).inspect} after moving, at frame #{$rubevy[:frame]}"

:async_done
