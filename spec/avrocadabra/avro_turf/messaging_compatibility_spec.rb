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
    [["null", { "type" => "int", "logicalType" => "date" }], "invalid"]
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
