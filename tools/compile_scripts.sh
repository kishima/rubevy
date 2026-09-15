#!/bin/bash
# Compile the Ruby that ships with this crate to .mrb with the reference mruby (Docker image
# kishima/mruby:4.1.0-rc): the example scripts in assets/scripts, and src/prelude.rb, which the
# plugin runs when the VM starts (`include_bytes!` in src/lib.rs). Both .rb and .mrb are
# committed; run this after changing a .rb.
set -eu
cd "$(dirname "$0")/.."
compile() {
  local dir="$1" rb="$2"
  docker run --rm -v "$PWD/$dir:/w" kishima/mruby:4.1.0-rc mrbc -o "/w/$(basename "${rb%.rb}").mrb" "/w/$(basename "$rb")"
  echo "$dir/$rb -> $dir/${rb%.rb}.mrb"
}
for rb in assets/scripts/*.rb; do
  compile assets/scripts "$(basename "$rb")"
done
compile src prelude.rb
