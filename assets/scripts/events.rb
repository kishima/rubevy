# Events: `Rubevy.subscribe` answers a queue the game pushes onto, so waiting for something to
# happen is the same thing as waiting for an answer — the task is parked and costs nothing.
#
# Two things are waited on here at once: the brain goes about its business in this task, and a
# second task does nothing but read the queue. That is the shape a reflex wants — it is not a
# callback interrupting the brain, it is another task that happens to be ready.
# See examples/events.rs.

hits = Rubevy.subscribe(:hit)        # this entity's own: only hits aimed here
bells = Rubevy.subscribe(:bell)      # everyone's

Task.new(name: "reflex") do
  loop do
    who, damage = hits.pop           # parked here until the game publishes
    Rubevy.log "reflex: hit by #{who.inspect} for #{damage.round(1)}"
  end
end

Rubevy.log "events: thinking"
6.times do |i|
  Rubevy.log "events: round #{i} (#{bells.size} bells waiting)"
  sleep 0.1
end

Rubevy.log "events: heard #{bells.size} bells"
bells.size.times { Rubevy.log "events:   bell #{bells.pop}" }
:events_done
