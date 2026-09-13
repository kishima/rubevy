# Rubevy.ask: the script asks the game for something and waits for the answer.
# Its task is parked meanwhile — no polling, no busy loop — and the other scripts keep running.
# The game answers with ScriptWorld::take_requests / answer (see examples/sensor.rs).

Rubevy.log "sensor: asking for a scan"

3.times do |i|
  found = Rubevy.ask("scan", 40.0).pop      # parks here until the game answers
  if found
    Rubevy.log "sensor: round #{i}: enemy at #{found[0].round(1)}, #{found[1].round(1)}"
    Rubevy.move_to found[0], found[1], 0.0
  else
    Rubevy.log "sensor: round #{i}: nothing in range"
  end
  sleep 0.05
end

Rubevy.log "sensor: done at frame #{$rubevy[:frame]}"
:sensor_done
