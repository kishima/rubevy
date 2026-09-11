# Compiled to hello.mrb by tools/compile_scripts.sh (reference mrbc 4.1.0-rc).
puts "hello from SabiRuby inside Bevy"
ticks = 0
loop do
  ticks += 1
  puts "tick #{ticks}: frame=#{$frame} delta=#{$delta.round(3)}" if ticks % 25 == 0
  break if ticks >= 100
end
class Counter
  attr_reader :n
  def initialize; @n = 0; end
  def bump; @n += 1; self; end
end
c = Counter.new
10.times { c.bump }
puts "counter=#{c.n}"
puts "done at frame #{$frame}"
