# frozen_string_literal: true

require "avro"
require "avrocadabra/avro_turf"

engine, kind = ARGV
decimal = { type: "bytes", logicalType: "decimal", precision: 6, scale: 2 }
timestamp = { type: "long", logicalType: "timestamp-millis" }
redefine = lambda do |owner, name, &condition|
  armed = true
  owner.prepend(Module.new do
    define_method(name) do |*args|
      if armed && instance_exec(*args, &condition)
        armed = false
        Hash.prepend(Module.new { def [](key) = key == "second" ? 22 : super })
      end
      super(*args)
    end
  end)
end
digits = -> { redefine.call(Array, :[]) { |*args| args == [1] && length == 4 && self[2] == 10 } }
type, value, patch = {
  "integer" => [%w[null long], 5, -> { redefine.call(Integer, :nil?) { true } }],
  "float" => [%w[null double], 1.5, -> { redefine.call(Float, :nil?) { true } }],
  "boolean" => [%w[null boolean], true, -> { redefine.call(TrueClass, :nil?) { true } }],
  "time" => [["null", timestamp], Time.at(1), -> { redefine.call(Time, :nil?) { true } }],
  "decimal" => [["null", decimal], BigDecimal("1.5"), -> { redefine.call(BigDecimal, :nil?) { true } }],
  "inspect" => [["string", decimal], BigDecimal("1.5"), -> { redefine.call(BigDecimal, :inspect) { true } }],
  "digits" => [decimal, BigDecimal("1.5"), digits],
  "limited" => [decimal, BigDecimal("1.5"), -> { BigDecimal.limit(30) && digits.call }],
  "aliased" => [decimal, BigDecimal("1.5"), -> { BigDecimal.alias_method(:*, :+) }],
  "undefined" => [decimal, BigDecimal("1.5"), -> { BigDecimal.send(:undef_method, :to_i) }]
}.fetch(kind)
schema = Avro::Schema.parse(JSON.generate({ type: "record", name: "Patched",
                                            fields: [{ name: "v", type: type }, { name: "second", type: "long" }] }))
cache = Avrocadabra::AvroTurf::Cache.new
write = lambda do
  output = StringIO.new("".b)
  encode = -> { Avro::IO::DatumWriter.new(schema).write({ "v" => value, "second" => 2 }, Avro::IO::BinaryEncoder.new(output)) }
  engine == "native" ? Avrocadabra::AvroTurf.with_codecs(cache, &encode) : encode.call
  output.string.unpack1("H*")
rescue StandardError => e
  "#{e.class}: #{e.message}"
end
write.call
patch.call
puts write.call
