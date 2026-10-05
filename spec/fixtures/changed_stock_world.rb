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
  "hash impostor" => [pair, impostor.new, nil, -> { on_call.call(impostor, :key?) }]
}.fetch(kind)
before&.call
require "avrocadabra/avro_turf"

cache = Avrocadabra::AvroTurf::Cache.new
write = lambda do
  current[:schema] = Avro::Schema.parse(JSON.generate(definition))
  output = StringIO.new("".b)
  encode = -> { Avro::IO::DatumWriter.new(current[:schema]).write(datum, Avro::IO::BinaryEncoder.new(output)) }
  engine == "native" ? Avrocadabra::AvroTurf.with_codecs(cache, &encode) : encode.call
  output.string.unpack1("H*")
rescue StandardError => e
  "#{e.class}: #{e.message}"
end
write.call
after&.call
calls = 0
puts [write.call, calls].inspect
