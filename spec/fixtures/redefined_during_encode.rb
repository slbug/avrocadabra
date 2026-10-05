# frozen_string_literal: true

require "avro"
require "avrocadabra/avro_turf"

fields = [{ name: "first", type: "long" }, { name: "second", type: "long" }]
schema = Avro::Schema.parse(JSON.generate({ type: "record", name: "Pair", fields: fields }))
datum = Hash.new do
  Hash.prepend(Module.new { def [](key) = key == "second" ? 22 : super })
  1
end
datum["second"] = 2
output = StringIO.new("".b)
write = -> { Avro::IO::DatumWriter.new(schema).write(datum, Avro::IO::BinaryEncoder.new(output)) }
ARGV.first == "native" ? Avrocadabra::AvroTurf.with_codecs(Avrocadabra::AvroTurf::Cache.new, &write) : write.call
puts output.string.unpack1("H*")
