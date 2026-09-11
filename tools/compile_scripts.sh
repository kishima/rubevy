#!/bin/bash
# Compile assets/scripts/*.rb to .mrb with the reference mruby (Docker image kishima/mruby:4.1.0-rc).
set -eu
cd "$(dirname "$0")/.."
for rb in assets/scripts/*.rb; do
  docker run --rm -v "$PWD/assets/scripts:/w" kishima/mruby:4.1.0-rc mrbc -o "/w/$(basename "${rb%.rb}").mrb" "/w/$(basename "$rb")"
  echo "$rb -> ${rb%.rb}.mrb"
done
