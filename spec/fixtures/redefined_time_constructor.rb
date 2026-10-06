# frozen_string_literal: true

require "avro"
require "avrocadabra/avro_turf"

engine, kind = ARGV
timestamp = { type: "long", logicalType: "timestamp-millis" }
schema = Avro::Schema.parse(JSON.generate({ type: "record", name: "Stamp",
                                            fields: [{ name: "at", type: ["null", timestamp] }] }))
value = kind == "date" ? Date.new(2000, 1, 1) : DateTime.new(2000, 1, 1, 12)
constructor = kind == "date" ? :local : :new
calls = 0
Time.singleton_class.prepend(Module.new do
  define_method(constructor) do |*args, **options|
    super(*args, **options) + (calls += 1)
  end
end)
output = StringIO.new("".b)
write = -> { Avro::IO::DatumWriter.new(schema).write({ "at" => value }, Avro::IO::BinaryEncoder.new(output)) }
engine == "native" ? Avrocadabra::AvroTurf.with_codecs(Avrocadabra::AvroTurf::Cache.new, &write) : write.call
puts output.string.unpack1("H*")
