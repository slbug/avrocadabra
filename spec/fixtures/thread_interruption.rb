# frozen_string_literal: true

require "avrocadabra"

schema = Avrocadabra::Schema.new({ type: "array", items: "long" })
data = Array.new(20_000) { it }
bytes = schema.encode(data)
12.times do
  ready = Queue.new
  worker = Thread.new do
    ready << true
    loop { schema.decode(bytes, release_gvl: true) }
  end
  ready.pop
  sleep 0.002
  worker.kill.join
  raise "corrupt state after interruption" unless schema.decode(bytes) == data
end
GC.start
GC.compact
puts "ok"
