# frozen_string_literal: true

require "avro"
require "avrocadabra/avro_turf"

fields = [{ name: "blob", type: "string" }, { name: "right", type: "string" }]
schema = Avro::Schema.parse(JSON.generate({ type: "record", name: "Leak", fields: fields }))
codec = Avrocadabra::AvroTurf::Codec.new(schema)
lookup = Class.new(Hash) { def [](key) = key == "right" ? raise(IOError) : super }
datum = lookup.new.merge("blob" => "x" * 262_144, "right" => "y")
failures = lambda do |count|
  count.times do
    codec.encode(datum)
  rescue IOError
    nil
  end
  GC.start
  `ps -o rss= -p #{Process.pid}`.to_i * 1024
end
baseline = failures.call(2000)
puts failures.call(2000) - baseline
