# Components by name, through Bevy's reflection: the script reads its own Transform as a Hash,
# changes one number, and writes it back. Nothing in rubevy knows what a Transform is — the
# type registry does (`app.register_type::<T>()`), so a game's own component works the same way.
# See examples/components.rs.

Rubevy.log "components: I am #{Rubevy.entity.inspect}"
Rubevy.log "components: I have #{Rubevy.entity.components.join(', ')}"
Rubevy.log "components: a Waypoint? #{Rubevy.entity.has?(:Waypoint)}"

4.times do |i|
  e = Rubevy.entity
  tf = e[:Transform]                        # parked here until the host answers, next frame
  x = tf[:translation][0]
  Rubevy.log "components: step #{i}, x = #{x.round(2)}"
  tf[:translation][0] = x + 1.0             # a Vec3 is three numbers
  e[:Transform] = tf                        # applied after this frame's scripts have run
  sleep 0.05
end

# Everything that has a component, whatever entity it sits on. It walks the world, so it is for
# a lookup now and then — not for every frame.
waypoints = Rubevy.find(:Waypoint)
Rubevy.log "components: #{waypoints.length} waypoints"
waypoints.each do |w|
  at = w[:Transform][:translation]
  Rubevy.log "components:   #{w.inspect} at #{at[0].round(1)}, #{at[1].round(1)}"
end

:components_done
