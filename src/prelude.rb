# Compiled to prelude.mrb by tools/compile_scripts.sh (reference mrbc 4.1.0-rc) and run when the
# VM starts (`include_bytes!` in src/lib.rs), so every script has these without requiring
# anything: they belong to the plugin, as `Rubevy::Entity` does.
#
# Why Ruby and not `Vm::define_fn`. Every reading method here waits for an answer, and waiting
# is the one thing a native cannot do: `Rubevy.ask` parks the task on a queue, and the VM
# refuses a blocking `pop` inside a native ("blocking pop cannot be called from within a C
# function boundary", sabiruby src/builtins/ext_task.rs). A native would have to hand the queue
# back and let the script `pop` it, which is not what `e[:Transform]` should be. So the work
# stays on the Rust side (`answer_components` in src/lib.rs) and the waiting is here — the same
# reason `Rubevy::Proxy` is Ruby.
module Rubevy
  class Entity
    # The component as a Hash of its fields, or nil where the entity has no component of that
    # type — or the type is not registered (`app.register_type::<T>()`; nothing unregistered is
    # visible from Ruby). The answer comes from the host on the next frame, and the task is
    # parked until it does.
    def get(name)
      Rubevy.ask("component.get", self, name.to_s).pop
    end

    # `e[:Transform]`, which is what a script writes. mrbc folds a one-argument `[]` into
    # OP_GETIDX; the VM answers an Array, Hash or String itself and *sends* everything else in
    # the frame the call was made in, so this body is an ordinary frame and the task can be
    # parked in it until the host answers. It was `get` only (sabiruby ran OP_GETIDX through a
    # nested run loop, which is a native boundary a task cannot be parked across); `get` stays
    # because a script that spells the round trip out is easier to read than one that does not.
    def [](name)
      get(name)
    end

    # Writes are deferred, as `Rubevy.spawn` and `Rubevy.move_to` are: the value is applied
    # after this frame's scripts have run. Only the fields the Hash names are written, so
    # reading a component, changing one number and writing it back is one round trip and does
    # not undo what another script wrote to another field.
    def set(name, value)
      Rubevy.set_component(self, name.to_s, value)
      value
    end

    def []=(name, value)
      set(name, value)
    end

    # Whether the entity has that component. Unregistered types answer false.
    def has?(name)
      Rubevy.ask("component.has", self, name.to_s).pop
    end

    # The short type names of the registered components on this entity, sorted.
    def components
      Rubevy.ask("components", self).pop
    end
  end

  # Every entity that has that component, as an Array of Rubevy::Entity. It walks the whole
  # world, so it is for a lookup now and then — at the start, on an event — not for every frame.
  def self.find(name)
    ask("entities.with", name.to_s).pop
  end
end
