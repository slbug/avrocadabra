# frozen_string_literal: true

require "avro"

engine, kind = ARGV
calls = 0
current = {}
count = ->(*) { calls += 1 }
on_call = lambda do |owner, name, &change|
  owner.prepend(Module.new do
    define_method(name) do |*args|
      count.call
      change&.call(calls)
      super(*args)
    end
  end)
end
pair = { type: "record", name: "Pair", fields: [{ name: "v", type: %w[null long] }, { name: "second", type: "long" }] }
stamp = { type: "record", name: "Stamp",
          fields: [{ name: "at", type: { type: "long", logicalType: "timestamp-millis" } }] }
single = { type: "record", name: "Single", fields: [{ name: "x", type: { type: "fixed", name: "One", size: 1 } }] }
renamed = { type: "record", name: "Renamed", fields: [{ name: "a", type: "long" }] }
decimal = { type: "bytes", logicalType: "decimal", precision: 6, scale: 2 }
renaming = Class.new(Hash) do
  define_method(:key?) do |key|
    current[:schema].fields.first.instance_variable_set(:@name, "b")
    super(key)
  end
end
impostor = Class.new do
  def is_a?(klass) = klass == Hash || super
  def key?(key) = key == "v"
  def [](key) = key == "v" ? 7 : 3
end
definition, datum, before, after = {
  "write-time validation" => [pair, { "v" => 5, "second" => 2 }, nil, lambda do
    on_call.call(Integer, :is_a?) do |seen|
      Hash.prepend(Module.new { def [](key) = key == "second" ? 22 : super }) if seen == 2
    end
  end],
  "method removed before load" => [pair, { "v" => 5, "second" => 2 },
                                   -> { BigDecimal.send(:remove_method, :to_i) }, nil],
  "singleton visibility" => [stamp, { "at" => 7 }, nil,
                             -> { Avro::LogicalTypes::TimestampMillis.singleton_class.send(:private, :encode) }],
  "ancestor module method" => [pair, { "v" => 5, "second" => 2 }, nil, -> { on_call.call(Comparable, :is_a?) }],
  "late mixin method" => [pair, { "v" => 5, "second" => 2 }, -> { Integer.include(Module.new) },
                          -> { on_call.call(Integer.ancestors[1], :is_a?) }],
  "returning raise" => [pair, { "v" => "x", "second" => 2 }, nil,
                        -> { Avro::IO::DatumWriter.define_method(:raise) { |*| count.call } }],
  "constant swapped mid-encode" => [pair, { "v" => 5, "second" => 2 }, nil, lambda do
    on_call.call(Integer, :is_a?) { |seen| Avro::IO::DatumWriter.const_set(:Hash, Array) if seen == 1 }
  end],
  "fields reordered mid-encode" => [pair, { "v" => 5, "second" => 2 }, nil, lambda do
    on_call.call(Integer, :is_a?) { |seen| current[:schema].fields.reverse! if seen == 1 }
  end],
  "union branches reordered mid-encode" => [pair, { "v" => nil, "second" => 2 }, nil, lambda do
    on_call.call(NilClass, :nil?) { |seen| current[:schema].fields.first.type.schemas.reverse! if seen == 1 }
  end],
  "hash impostor" => [pair, impostor.new, nil, -> { on_call.call(impostor, :key?) }],
  "plan replaced mid-encode" => [single, { "x" => "B" }, nil, lambda do
    on_call.call(Hash, :key?) do |seen|
      next unless seen == 1

      current[:schema].fields.first.instance_variable_set(:@type, Avro::Schema.parse('"string"'))
      current[:nested].call
      GC.start(full_mark: true, immediate_sweep: true)
    end
  end],
  "field renamed inside key?" => [renamed, renaming.new.update("a" => 1, "b" => 22), nil, nil],
  "decimal factor changed" => [decimal, BigDecimal("1.5"), nil, lambda do
    current[:prepare] = ->(schema) { schema.type_adapter.instance_variable_set(:@factor, BigDecimal(1000)) }
  end]
}.fetch(kind)
before&.call
require "avrocadabra/avro_turf"

cache = Avrocadabra::AvroTurf::Cache.new
encode = lambda do |value|
  output = StringIO.new("".b)
  operation = -> { Avro::IO::DatumWriter.new(current[:schema]).write(value, Avro::IO::BinaryEncoder.new(output)) }
  engine == "native" ? Avrocadabra::AvroTurf.with_codecs(cache, &operation) : operation.call
  output.string.unpack1("H*")
end
current[:nested] = -> { encode.call({ "x" => "changed" }) }
write = lambda do
  current[:schema] = Avro::Schema.parse(JSON.generate(definition))
  current[:prepare]&.call(current[:schema])
  encode.call(datum)
rescue StandardError => e
  "#{e.class}: #{e.message}"
end
write.call
after&.call
calls = 0
puts [write.call, calls].inspect
