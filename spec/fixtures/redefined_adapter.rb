# frozen_string_literal: true

require "avro"
require "avrocadabra/avro_turf"

decimal = { type: "bytes", logicalType: "decimal", precision: 9, scale: 4 }
fields = [{ name: "value", type: ["null", decimal] }]
schema = Avro::Schema.parse(JSON.generate({ type: "record", name: "Amount", fields: fields }))
calls = 0
Avro::LogicalTypes::BytesDecimal.prepend(Module.new { define_method(:encode) { |value| super(value + (calls += 1)) } })
output = StringIO.new("".b)
write = lambda do
  Avro::IO::DatumWriter.new(schema).write({ "value" => BigDecimal("1.5") }, Avro::IO::BinaryEncoder.new(output))
end
ARGV.first == "native" ? Avrocadabra::AvroTurf.with_codecs(Avrocadabra::AvroTurf::Cache.new, &write) : write.call
puts output.string.unpack1("H*")
