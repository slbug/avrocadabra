# frozen_string_literal: true

require "avro"
require "avrocadabra/avro_turf"

fields = [{ name: "blob", type: "string" }, { name: "right", type: "string" }]
schema = Avro::Schema.parse(JSON.generate({ type: "record", name: "Leak", fields: fields }))
cache = Avrocadabra::AvroTurf::Cache.new
lookup = Class.new(Hash) { def [](key) = key == "right" ? raise(IOError) : super }
datum = lookup.new.merge("blob" => "x" * 262_144, "right" => "y")
failures = lambda do |count|
  count.times do
    Avrocadabra::AvroTurf.with_codecs(cache) do
      Avro::IO::DatumWriter.new(schema).write(datum, Avro::IO::BinaryEncoder.new(StringIO.new(+"".b)))
    end
  rescue IOError
    nil
  end
  GC.start
  `ps -o rss= -p #{Process.pid}`.to_i * 1024
end
baseline = failures.call(2000)
puts failures.call(2000) - baseline
