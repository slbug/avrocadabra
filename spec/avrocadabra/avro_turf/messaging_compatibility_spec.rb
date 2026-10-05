# frozen_string_literal: true

require "support/schema_registry"

RSpec.describe Avrocadabra::AvroTurf::Messaging do
  include_context "with a schema registry"

  let(:store) { AvroTurfFixture.schema_store }
  let(:options) { { registry: registry, schema_store: store, logger: Logger.new(nil) } }
  let(:native) { described_class.new(**options) }
  let(:reference) { AvroTurf::Messaging.new(**options) }

  def expect_compatible(definition, datum)
    id = registry.register("values", reference_schema(definition))
    ruby_bytes = reference.encode(datum, schema_id: id)
    native_bytes = native.encode(datum, schema_id: id)
    expected = reference.decode(ruby_bytes)
    expect(native_bytes.byteslice(0, 5)).to eq(ruby_bytes.byteslice(0, 5))
    expect(native_bytes.encoding).to eq(ruby_bytes.encoding)
    expect(value_contract(reference.decode(native_bytes))).to eq(value_contract(expected))
    expect(value_contract(native.decode(ruby_bytes))).to eq(value_contract(expected))
    expect(value_contract(native.decode(native_bytes))).to eq(value_contract(expected))
    expected
  end

  def decoding_result(client, bytes)
    result = client.decode_message(bytes, schema_name: "Reader")
    [result.message, result.schema_id, result.writer_schema.to_s, result.reader_schema]
  rescue StandardError => e
    [e.class]
  end

  [
    ["null", nil], ["boolean", false], ["boolean", true], ["int", -(2**31)], ["int", (2**31) - 1],
    ["long", -(2**63)], ["long", (2**63) - 1], ["float", 1.25], ["float", BigDecimal("1.25")],
    ["double", 2**80], ["double", BigDecimal("1.25")], ["bytes", "\x00\xff".b], ["string", "α 🌍"],
    ["string", "é".encode("ISO-8859-1")],
    [{ "type" => "enum", "name" => "State", "symbols" => %w[A B] }, "B"],
    [{ "type" => "fixed", "name" => "Token", "size" => 2 }, "\x00\xff".b],
    [{ "type" => "fixed", "name" => "Empty", "size" => 0 }, "".b],
    [{ "type" => "array", "items" => "long" }, [1, -2, 3]],
    [{ "type" => "map", "values" => "long" }, { "β" => 1, "a" => 2 }],
    [{ "type" => "map", "values" => "long" }, { "é".encode("ISO-8859-1") => 1 }],
    [%w[int long], 1], [%w[int long], 2**40], [%w[int long double], 2**80], [%w[null boolean], false],
    [%w[long int], 1], [%w[string bytes], "value"], [%w[bytes string], "value"],
    [%w[null string], nil], [%w[null string], ""],
    [{ "type" => "int", "logicalType" => "date" }, Date.new(1582, 10, 4)],
    [{ "type" => "int", "logicalType" => "date" }, 10.9],
    [{ "type" => "long", "logicalType" => "timestamp-millis" }, Time.at(-1, 123_456, :microsecond)],
    [{ "type" => "long", "logicalType" => "timestamp-micros" }, DateTime.new(2000, 1, 1)],
    [{ "type" => "long", "logicalType" => "timestamp-nanos" }, Time.at(1, 123_456_789, :nanosecond)],
    [{ "type" => "long", "logicalType" => "timestamp-micros" }, 123.9],
    [{ "type" => "bytes", "logicalType" => "decimal", "precision" => 8, "scale" => 2 }, 12.34],
    [{ "type" => "bytes", "logicalType" => "decimal", "precision" => 8, "scale" => 2 }, BigDecimal("-12.34")],
    [{ "type" => "bytes", "logicalType" => "decimal", "precision" => 8, "scale" => 2 }, 12],
    [{ "type" => "int", "logicalType" => "time-millis" }, -1],
    [{ "type" => "long", "logicalType" => "local-timestamp-micros" }, -1],
    [{ "type" => "string", "logicalType" => "uuid" }, "123E4567-E89B-12D3-A456-426614174000"],
    [{ "type" => "fixed", "name" => "Decimal", "size" => 2,
       "logicalType" => "decimal", "precision" => 4, "scale" => 2 }, "\x04\xd2".b],
    [{ "type" => "fixed", "name" => "Duration", "size" => 12, "logicalType" => "duration" }, [1, 2, 3].pack("V3")],
    [{ "type" => "bytes", "logicalType" => "big-decimal" }, "\x02\x07\x04".b],
    [{ "type" => "map", "values" => { "type" => "int", "logicalType" => "date" } },
     { "second" => Date.new(2000, 1, 1), "first" => Date.new(1970, 1, 1) }],
    [{ "type" => "record", "name" => "Day",
       "fields" => [{ "name" => "day", "type" => { "type" => "int", "logicalType" => "date" } }] },
     { day: Date.new(2000, 1, 1) }]
  ].each do |definition, datum|
    it "matches #{definition.inspect} with #{datum.inspect}" do
      expect_compatible(definition, datum)
    end
  end

  floating_schemas = ["float", "double", %w[float double]]
  { "float overflow" => 1e100, "integer overflow" => 2**2048,
    "decimal overflow" => BigDecimal("1e1000") }.each do |name, value|
    floating_schemas.each do |definition|
      it "matches #{name} for #{definition.inspect}" do
        expect_compatible(definition, value)
        expect_compatible(definition, -value)
      end
    end
  end

  [
    ["null", 0], ["boolean", 0], ["int", 2**31], ["long", -(2**63) - 1], ["int", 1.0],
    ["float", "1"], ["double", Rational(1, 2)], ["string", :text], ["bytes", 1],
    [{ "type" => "fixed", "name" => "Token", "size" => 2 }, "x"],
    [{ "type" => "enum", "name" => "State", "symbols" => %w[A B] }, "C"],
    [{ "type" => "array", "items" => "long" }, {}], [{ "type" => "map", "values" => "long" }, []],
    [%w[null string], 1],
    [{ "type" => "bytes", "logicalType" => "decimal", "precision" => 3, "scale" => 2 }, "1.25"],
    [{ "type" => "bytes", "logicalType" => "decimal", "precision" => 3, "scale" => 2 }, 1.234],
    [{ "type" => "bytes", "logicalType" => "decimal", "precision" => 3, "scale" => 2 }, 10],
    [{ "type" => "array", "items" => { "type" => "int", "logicalType" => "date" } }, {}],
    [{ "type" => "map", "values" => { "type" => "int", "logicalType" => "date" } }, []],
    [{ "type" => "record", "name" => "Day",
       "fields" => [{ "name" => "day", "type" => { "type" => "int", "logicalType" => "date" } }] }, 42],
    [["null", { "type" => "int", "logicalType" => "date" }], "invalid"],
    [["null", { "type" => "record", "name" => "Item", "fields" => [{ "name" => "id", "type" => "int" }] }],
     { "id" => "1" }],
    [["null", { "type" => "array", "items" => ["null", { "type" => "record", "name" => "Item",
                                                         "fields" => [{ "name" => "id", "type" => "int" }] }] }],
     [nil, { "id" => nil }]]
  ].each do |definition, datum|
    it "preserves the encoding exception for #{definition.inspect} and #{datum.inspect}" do
      id = registry.register("errors", reference_schema(definition))
      expect { reference.encode(datum, schema_id: id) }.to raise_error(StandardError) do |error|
        expect { native.encode(datum, schema_id: id) }.to raise_error(error.class, error.message)
      end
    end
  end

  it "chooses the first valid record, map or enum branch like Ruby Avro" do
    record = record_schema("Choice", [field("id", "int")])
    map = { "type" => "map", "values" => "long" }
    expect_compatible([record, map], { "id" => 1 })
    expect_compatible([map, record], { "id" => 1 })
    expect_compatible([record, map], { "other" => 1 })
    enumeration = { "type" => "enum", "name" => "Choice", "symbols" => ["A"] }
    expect_compatible([enumeration, "string"], "A")
    expect_compatible([enumeration, "string"], "B")
  end

  it "encodes decimal adapter values like Ruby Avro" do
    random = Random.new(20_261_005)
    floats = Array.new(400) { (random.rand - 0.5) * (10**random.rand(-12..12)) } +
             Array.new(200) { random.rand(2**53) / (2.0**random.rand(1..12)) } +
             [0.0, -0.0, 0.9524, 1e23, 5e-324, 0.30233257263183977, 920_013_567_207_072.2, 97_404_494_744_092.62,
              Float::NAN, Float::INFINITY]
    numbers = [0, 1, -1, 127, -128, 2**40, -(2**62), 2**70, Rational(1, 2)] +
              %w[0 -0 1.5 0.95240000 -12.345678 123456789.12345678 1e-9 NaN].map { BigDecimal(it) }
    [[14, 8], [4, 0], [38, 18], [6, 2]].each do |precision, scale|
      decimal = { "type" => "bytes", "logicalType" => "decimal", "precision" => precision, "scale" => scale }
      definition = record_schema("Amount", [field("value", decimal), field("optional", ["null", decimal])])
      id = registry.register("decimals", reference_schema(definition))
      (floats + numbers + ["1.5"]).each do |value|
        results = [reference, native].map do |client|
          client.encode({ "value" => value, "optional" => value }, schema_id: id)
        rescue StandardError => e
          [e.class, e.message]
        end
        expect(results.last).to eq(results.first), "#{value.inspect} as decimal(#{precision}, #{scale})"
      end
    end
  end

  it "encodes decimals under a BigDecimal precision limit like Ruby Avro" do
    decimal = { "type" => "bytes", "logicalType" => "decimal", "precision" => 9, "scale" => 4 }
    id = registry.register("limited", reference_schema(record_schema("Amount", [field("value", ["null", decimal])])))
    previous = BigDecimal.limit(2)
    datum = { "value" => BigDecimal("12.345") }
    expect(native.encode(datum, schema_id: id)).to eq(reference.encode(datum, schema_id: id))
  ensure
    BigDecimal.limit(previous)
  end

  it "calls redefined built-in adapters like Ruby Avro" do
    timestamp = { "type" => "long", "logicalType" => "timestamp-millis" }
    decimal = { "type" => "bytes", "logicalType" => "decimal", "precision" => 9, "scale" => 4 }
    definition = record_schema("Stamped", [field("at", ["null", timestamp]), field("amount", ["null", decimal])])
    id = registry.register("stamped", reference_schema(definition))
    calls = 0
    allow(Avro::LogicalTypes::TimestampMillis).to receive(:encode).and_wrap_original do |original, value|
      original.call(value) + (calls += 1)
    end
    results = [reference, native].map do |client|
      calls = 0
      adapter = client.fetch_schema_by_id(id).first.fields.last.type.schemas.last.type_adapter
      allow(adapter).to(receive(:encode).and_wrap_original { |original, value| original.call(value + (calls += 1)) })
      client.encode({ "at" => Time.at(1), "amount" => BigDecimal("1.5") }, schema_id: id)
    end
    expect(results.last).to eq(results.first)
  end

  it "dispatches field-name objects with overridden equality like Ruby Avro" do
    definition = record_schema("Pair", [field("left", "int"), field("right", "string")])
    id = registry.register("pairs", reference_schema(definition))
    name = Class.new(String) do
      def eql?(_other) = raise(IOError, "field name equality failed")
    end
    results = [reference, native].map do |client|
      client.fetch_schema_by_id(id).first.fields.last.instance_variable_set(:@name, name.new("right"))
      client.encode({ "left" => 1, "right" => "x" }, schema_id: id)
    rescue IOError => e
      [e.class, e.message]
    end
    expect(results).to eq([[IOError, "field name equality failed"]] * 2)
  end

  it "looks up identity hash fields by the schema's field name objects" do
    definition = record_schema("Pair", [field("left", "int"), field("right", %w[null string])])
    nested = ["null", definition]
    [definition, nested].each do |schema_definition|
      id = registry.register("pairs", reference_schema(schema_definition))
      results = [reference, native].map do |client|
        schema = client.fetch_schema_by_id(id).first
        record = schema.type_sym == :union ? schema.schemas.last : schema
        datum = {}.compare_by_identity
        datum[record.fields.first.name] = 1
        datum[record.fields.last.name] = "x"
        client.encode(datum, schema_id: id)
      end
      expect(results.last).to eq(results.first)
    end
  end

  it "calls hash default procs inside union branches like Ruby Avro" do
    id = registry.register("defaults", reference_schema(["null", record_schema("Counter", [field("id", "long")])]))
    counter = lambda do
      calls = 0
      Hash.new { calls += 1 }
    end
    expect(native.encode(counter.call, schema_id: id)).to eq(reference.encode(counter.call, schema_id: id))
  end

  it "selects unambiguous nested branches without Ruby Avro validation" do
    factor = { "type" => "bytes", "logicalType" => "decimal", "precision" => 14, "scale" => 8 }
    seen = { "type" => "long", "logicalType" => "timestamp-millis" }
    flag = record_schema("Flag",
                         [field("code", "string"), field("factor", ["null", factor]), field("seen", ["null", seen])])
    station = record_schema("Station", [field("reading", "int"),
                                        field("flags", ["null", { "type" => "array", "items" => ["string", flag] }])])
    definition = ["null", { "type" => "map", "values" => station }]
    datum = { "a" => { reading: 1, flags: ["raw", { code: "x", factor: BigDecimal("0.9524"), seen: Time.at(1) }] },
              "b" => { reading: 2, flags: nil } }
    expect_compatible(definition, datum)
    allow(Avro::Schema).to receive(:validate).and_call_original
    native.encode(datum, schema_id: registry.register("values", reference_schema(definition)))
    expect(Avro::Schema).not_to have_received(:validate)
  end

  it "preserves defaults on hashes, nullable omissions and string-key precedence" do
    schema = record_schema("Defaults", [field("enabled", "boolean"), field("note", %w[null string])])
    value = Hash.new(false).merge("enabled" => false, enabled: true, note: nil).freeze
    expect(expect_compatible(schema, value)).to eq("enabled" => false, "note" => nil)
    expect(expect_compatible(schema, { enabled: true })).to eq("enabled" => true, "note" => nil)
    expect(value).not_to have_key("note")
  end

  it "cross-decodes recursive records" do
    node = record_schema("Node", [field("value", "long"), field("next", %w[null Node])])
    expect_compatible(node, { "value" => 1, "next" => { "value" => 2 } })
  end

  it "does not count recursive union wrappers as an extra datum level" do
    node = record_schema("Node", [field("value", "long"), field("next", %w[null Node])])
    value = 41.times.reduce(nil) { |child, index| { "value" => index, "next" => child } }
    expect_compatible(node, value)
  end

  it "preserves Hash defaults and default-proc lookups for missing fields" do
    schema = record_schema("Defaults", [field("value", "long", default: 7)])
    expect(expect_compatible(schema, Hash.new(12))).to eq("value" => 12)
    seen = []
    input = Hash.new do |_hash, key|
      seen << key
      42
    end
    expect(expect_compatible(schema, input)).to eq("value" => 42)
    expect(seen).to eq(%i[value value])
    expect(input).to be_empty
  end

  it "rejects missing required writer fields despite schema defaults" do
    definition = record_schema("Required", [field("value", "long", default: 7)])
    id = registry.register("required", reference_schema(definition))
    expect { reference.encode({}, schema_id: id) }.to raise_error(Avro::IO::AvroTypeError) do |error|
      expect { native.encode({}, schema_id: id) }.to raise_error(error.class)
    end
  end

  it "looks up ordinary and logical fields in schema order" do
    schema = record_schema("Ordered", [field("id", "long"), field("day", { "type" => "int", "logicalType" => "date" })])
    id = registry.register("ordered", reference_schema(schema))
    results = [reference, native].map do |client|
      seen = []
      input = Hash.new do |_hash, key|
        seen << key
        seen.size
      end
      [client.encode(input, schema_id: id), seen, input]
    end
    expect(results.last).to eq(results.first)
    expect(results.last[1]).to eq(%i[id day])
  end

  it "raises for an earlier invalid field before converting a later logical value" do
    decimal = { "type" => "bytes", "logicalType" => "decimal", "precision" => 8, "scale" => 2 }
    schema = record_schema("Ordered", [field("id", "long"), field("amount", decimal)])
    id = registry.register("ordered", reference_schema(schema))
    value = { "id" => "invalid", "amount" => 1.234 }
    expect { reference.encode(value, schema_id: id) }.to raise_error(Avro::IO::AvroTypeError) do |error|
      expect { native.encode(value, schema_id: id) }.to raise_error(error.class, error.message)
    end
  end

  it "preserves nested logical adapter exceptions" do
    decimal = { "type" => "bytes", "logicalType" => "decimal", "precision" => 8, "scale" => 2 }
    definitions = [{ "type" => "array", "items" => decimal },
                   { "type" => "map", "values" => decimal }, record_schema("Decimals", [field("value", decimal)])]
    definitions.zip([[1.234], { "value" => 1.234 }, { "value" => 1.234 }]).each do |definition, value|
      id = registry.register("decimals", reference_schema(definition))
      expect { reference.encode(value, schema_id: id) }.to raise_error(RangeError, "Rounding necessary") do |error|
        expect { native.encode(value, schema_id: id) }.to raise_error(error.class, error.message)
      end
    end
  end

  it "applies logical adapters to the selected union branch" do
    date = { "type" => "int", "logicalType" => "date" }
    schema = record_schema("Choices", [field("day", ["null", date, "string"]),
                                       field("items", { "type" => "array", "items" => %w[null string] })])
    [nil, Date.new(2000, 2, 29), "today"].each do |day|
      expect_compatible(schema, { "day" => day, "items" => [nil, "x"] })
    end
  end

  it "uses Ruby Avro's missing-field semantics instead of writer schema defaults" do
    schema = record_schema("Defaults", [field("value", %w[long null], default: 7)])
    input = {}.freeze
    expect(expect_compatible(schema, input)).to eq("value" => nil)
    expect(input).to eq({})
  end

  it "preserves Ruby Avro's physical mapping for big-decimal" do
    definition = { "type" => "bytes", "logicalType" => "big-decimal" }
    id = registry.register("decimals", reference_schema(definition))
    value = BigDecimal("12345678901234567890.0123456789")
    direct = native_schema(definition)
    physical = reference_decode("bytes", direct.encode(value))
    expect(reference.decode(native.encode(physical, schema_id: id))).to eq(physical)
    expect(native.decode(reference.encode(physical, schema_id: id))).to eq(physical)
  end

  it "applies the same malformed-data validation as direct schema calls" do
    [["boolean", "\x02".b], ["string", "\x02\xff".b]].each do |definition, bytes|
      id = registry.register("invalid", reference_schema(definition))
      expect { native_schema(definition).decode(bytes) }.to raise_error(Avrocadabra::DecodeError)
      expect { native.decode("\x00".b + [id].pack("N") + bytes) }.to raise_error(Avro::AvroError)
    end
  end

  it "cross-decodes larger values without reparsing prepared schemas" do
    schema = record_schema("Batch", [field("blob", "bytes"), field("items", { "type" => "array", "items" => "int" })])
    value = { "blob" => Random.new(42).bytes(4096), "items" => (0...1000).to_a }
    expect_compatible(schema, value)
  end

  it "switches IDs, subjects, versions and reader definitions on the same client" do
    old = record_schema("Event", [field("id", "int")])
    current = record_schema("Event", [field("id", "long"), field("note", %w[null string], default: nil)])
    renamed = record_schema("Latest", [field("sequence", "long", aliases: ["id"])], aliases: ["Event"])
    ids = [old, current].map { registry.register("events", reference_schema(it)) }
    readers = [old, current, renamed].map { reference_schema(it) }
    readers.cycle.take(6).each do |reader|
      allow(store).to receive(:find).with("Reader", nil).and_return(reader)
      ids.each_with_index do |id, index|
        encoding = index.zero? ? { schema_id: id } : { subject: "events", version: 2 }
        bytes = reference.encode({ "id" => 42 }, **encoding)
        expect(decoding_result(native, bytes)).to eq(decoding_result(reference, bytes))
        expect(reference.decode(native.encode({ "id" => 42 }, **encoding))).to eq(reference.decode(bytes))
      end
    end
  end

  it "keeps logical annotations distinct in the cache" do
    date = { "type" => "int", "logicalType" => "date" }
    ["int", date, "int", date].each { expect_compatible(it, 1) }
  end
end
