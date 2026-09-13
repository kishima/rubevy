# Compiled to hello.mrb by tools/compile_scripts.sh (reference mrbc 4.1.0-rc).
# A script is a task: it may run, sleep, and come back next frame without
# costing anything in between.
require "helper"                    # read from the asset directory
Rubevy.log Helper.greet("SabiRuby inside Bevy")

3.times do |i|
  puts "worker: step #{i} at frame #{$rubevy[:frame]} (t=#{$rubevy[:time].round(2)}s)"
  sleep 0.05
end

Rubevy.spawn "marker", 1.0, 2.0, 3.0
Rubevy.move_to 4.0, 5.0, 6.0        # the entity this script is attached to
puts "worker: entity=#{Rubevy.entity}, done at frame #{$rubevy[:frame]}"
:worker_done
