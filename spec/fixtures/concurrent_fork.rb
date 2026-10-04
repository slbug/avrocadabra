# frozen_string_literal: true

require "avrocadabra"

schema = Avrocadabra::Schema.new({ type: "array", items: "long" })
data = Array.new(1_000) { it }
bytes = schema.encode(data)
ready = Queue.new
worker = Thread.new do
  ready << true
  loop { schema.decode(bytes, release_gvl: true) }
end
ready.pop
8.times do
  pid = fork do
    result = schema.decode(bytes, release_gvl: true)
    exit!(result == data ? 0 : 1)
  end
  _, status = Process.wait2(pid)
  raise "child process failed" unless status.success?
end
worker.kill.join
puts "ok"
