# Compiled to ticker.mrb by tools/compile_scripts.sh.
# A second task, at a lower priority (a higher number), sharing the same VM:
# globals and constants are shared, which is the design.
$shared = (($shared || 0) + 1)
5.times do
  puts "ticker: frame #{$rubevy[:frame]}, shared=#{$shared}"
  sleep 0.03
end
:ticker_done
