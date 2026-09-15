# Compiled to proxy.mrb by tools/compile_scripts.sh (reference mrbc 4.1.0-rc).
# A thin stand-in for something the game owns. `Rubevy::Proxy.new("robot").move_to(1, 2)` is
# `Rubevy.ask("robot.move_to", 1, 2).pop`: the question goes out with the frame's commands, the
# task is parked on the queue, and the call answers with whatever the game pushed back — which
# may be several frames later. Nothing about the call says any of that happened.
#
# This rests on the VM dispatching a `method_missing` written in Ruby in the frame the call was
# made in, so that the body may park the task (sabiruby `docs/design/fibers.md`, "Native
# boundaries"). A proxy is for objects the game did not register: the plain way is
# `Rubevy.<name>`, which is a real method and says what it takes (`docs/host-api.md`).
module Rubevy
  class Proxy
    def initialize(kind)
      @kind = kind
    end

    attr_reader :kind

    def method_missing(name, *args, &blk)
      Rubevy.ask("#{@kind}.#{name}", *args).pop
    end

    # A proxy answers for everything, so `respond_to?` agrees with what a call does.
    def respond_to_missing?(name, include_private = false)
      true
    end

    def inspect
      "#<Rubevy::Proxy #{@kind}>"
    end
    alias to_s inspect
  end
end
