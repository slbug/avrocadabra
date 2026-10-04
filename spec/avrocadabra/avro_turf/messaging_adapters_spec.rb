# frozen_string_literal: true

require "support/schema_registry"

RSpec.describe Avrocadabra::AvroTurf::Messaging do
  include_context "with a schema registry"

  let(:store) { AvroTurfFixture.schema_store }
  let(:options) { { registry: registry, schema_store: store, logger: Logger.new(nil) } }
  let(:native) { described_class.new(**options) }
  let(:reference) { AvroTurf::Messaging.new(**options) }

  def payload(writer, reader, value)
    allow(store).to receive(:find).with("Reader", nil).and_return(reader)
    id = registry.register("adapters", reference_schema(writer))
    reference.encode(value, schema_id: id)
  end

  def observe_adapter(schema, calls, label, increment)
    adapter = Object.new
    allow(adapter).to receive(:decode) do |value|
      calls << [label, value]
      value + increment
    end
    schema.instance_variable_set(:@type_adapter, adapter)
  end

  [[false, false, 42], [true, false, 43], [false, true, 42],
   [true, true, 52]].each do |writer_union, reader_union, expected|
    it "preserves adapter calls with writer union #{writer_union} and reader union #{reader_union}" do
      calls = []
      reader = reference_schema(reader_union ? %w[null long] : "long")
      observe_adapter(reader_union ? reader.schemas.last : reader, calls, :scalar, 1)
      observe_adapter(reader, calls, :union, 10) if reader_union
      bytes = payload(writer_union ? %w[null long] : "long", reader, 41)
      results = [reference, native, native].map do |client|
        calls.clear
        [client.decode(bytes, schema_name: "Reader"), calls.dup]
      end
      expect(results.map(&:first)).to eq([expected] * 3)
      expect(results.last(2)).to eq([results.first] * 2)
    end
  end

  [[{ "type" => "int", "logicalType" => "date" }, Date.new(2000, 1, 1)],
   [{ "type" => "long", "logicalType" => "timestamp-millis" }, Time.at(1)],
   [{ "type" => "long", "logicalType" => "timestamp-micros" }, Time.at(1)],
   [{ "type" => "bytes", "logicalType" => "decimal", "precision" => 8, "scale" => 2 },
    BigDecimal("1.23")]].each do |type, value|
    it "preserves #{type.fetch("logicalType")} adapter failures when removing nullability" do
      bytes = payload(["null", type], reference_schema(type), value)
      expect { reference.decode(bytes, schema_name: "Reader") }.to raise_error(StandardError) do |error|
        expect { native.decode(bytes, schema_name: "Reader") }.to raise_error(error.class, error.message)
      end
    end
  end

  it "keeps adapter order through nested collections and reader defaults" do
    old = record_schema("Item", [field("value", %w[null long])])
    current = record_schema("Item", [field("value", "long"), field("added", "long", default: 7)])
    writer = { "type" => "array", "items" => old }
    reader = reference_schema({ "type" => "array", "items" => current })
    calls = []
    reader.items.fields.each { observe_adapter(it.type, calls, it.name, 1) }
    bytes = payload(writer, reader, [{ "value" => 41 }, { "value" => 51 }])
    results = [reference, native].map do |client|
      calls.clear
      [client.decode(bytes, schema_name: "Reader"), calls.dup]
    end
    expect(results.last).to eq(results.first)
    expect(results.last.first).to eq([{ "value" => 43, "added" => 8 }, { "value" => 53, "added" => 8 }])
  end

  [false, true].each do |missing|
    it "runs earlier adapters before a later #{missing ? "missing" : "incompatible"} reader field fails" do
      fields = [field("first", "long"), field("later", "string")]
      writer = record_schema("Ordered", missing ? fields.take(1) : fields)
      reader = reference_schema(record_schema("Ordered", [field("first", "long"), field("later", "long")]))
      adapter = Object.new
      allow(adapter).to receive(:decode).and_raise(IOError, "earlier adapter failed")
      reader.fields.first.type.instance_variable_set(:@type_adapter, adapter)
      bytes = payload(writer, reader, "first" => 1, "later" => "bad")
      [reference, native].each do |client|
        expect { client.decode(bytes, schema_name: "Reader") }.to raise_error(IOError, "earlier adapter failed")
      end
      expect(adapter).to have_received(:decode).twice
    end
  end

  it "preserves built-in adapter precedence before incompatible later fields" do
    date = { "type" => "int", "logicalType" => "date" }
    writer = record_schema("OrderedDate", [field("first", ["null", date]), field("later", "string")])
    reader = reference_schema(record_schema("OrderedDate", [field("first", date), field("later", "long")]))
    bytes = payload(writer, reader, "first" => Date.new(2000, 1, 1), "later" => "bad")
    expect { reference.decode(bytes, schema_name: "Reader") }.to raise_error(StandardError) do |error|
      expect { native.decode(bytes, schema_name: "Reader") }.to raise_error(error.class, error.message)
    end
  end
end
