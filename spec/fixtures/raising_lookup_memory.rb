# frozen_string_literal: true

require "avro"
require "avrocadabra/avro_turf"

fields = [{ name: "blob", type: "string" }, { name: "right", type: "string" }]
schema = Avro::Schema.parse(JSON.generate({ type: "record", name: "Leak", fields: fields }))
schema.fields.last.instance_variable_set(:@name, Class.new(String) { def eql?(_other) = raise(IOError) }.new("right"))
codec = Avrocadabra::AvroTurf::Codec.new(schema)
datum = { "blob" => "x" * 262_144, "right" => "y" }
failures = lambda do |count|
  count.times do
    codec.encode(datum)
  rescue IOError
    nil
  end
  GC.start
  `ps -o rss= -p #{Process.pid}`.to_i * 1024
end
baseline = failures.call(50)
puts failures.call(300) - baseline
