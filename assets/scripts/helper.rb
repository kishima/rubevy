# required by hello.rb — shows that a script can require another from the asset directory
module Helper
  def self.greet(who)
    "hello, #{who}"
  end
end
