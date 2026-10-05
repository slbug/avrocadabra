# frozen_string_literal: true

require "avro"
require "avrocadabra/avro_turf"

timestamp = { type: "long", logicalType: "timestamp-millis" }
fields = [{ name: "at", type: ["null", timestamp] }]
schema = Avro::Schema.parse(JSON.generate({ type: "record", name: "Stamp", fields: fields }))
calls = 0
Time.prepend(Module.new { define_method(:to_time) { self + (calls += 1) } })
output = StringIO.new("".b)
write = -> { Avro::IO::DatumWriter.new(schema).write({ "at" => Time.at(1) }, Avro::IO::BinaryEncoder.new(output)) }
ARGV.first == "native" ? Avrocadabra::AvroTurf.with_codecs(Avrocadabra::AvroTurf::Cache.new, &write) : write.call
puts output.string.unpack1("H*")
