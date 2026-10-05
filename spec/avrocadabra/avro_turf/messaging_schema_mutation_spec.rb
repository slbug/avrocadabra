# frozen_string_literal: true

require "support/schema_registry"

RSpec.describe Avrocadabra::AvroTurf::Messaging do
  include_context "with a schema registry"

  let(:store) { AvroTurfFixture.schema_store }
  let(:options) { { registry: registry, schema_store: store, logger: Logger.new(nil) } }
  let(:native) { described_class.new(**options) }
  let(:reference) { AvroTurf::Messaging.new(**options) }

  it "recompiles an enum after its exposed symbols array changes" do
    definition = { "type" => "enum", "name" => "Choice", "symbols" => %w[A B] }
    id = registry.register("choices", reference_schema(definition))
    [reference, native].each do |client|
      expect(client.decode(client.encode("B", schema_id: id))).to eq("B")
      client.fetch_schema_by_id(id).first.symbols << "C"
    end
    bytes = reference.encode("C", schema_id: id)
    expect(native.encode("C", schema_id: id)).to eq(bytes)
    expect(native.decode(bytes)).to eq(reference.decode(bytes))
  end

  it "recompiles a writer after its fields are reordered" do
    definition = record_schema("Pair", [field("left", "int"), field("right", "string")])
    id = registry.register("pairs", reference_schema(definition))
    value = { "left" => 1, "right" => "two" }
    [reference, native].each do |client|
      expect(client.decode(client.encode(value, schema_id: id))).to eq(value)
      client.fetch_schema_by_id(id).first.fields.reverse!
    end
    bytes = reference.encode(value, schema_id: id)
    expect(native.encode(value, schema_id: id)).to eq(bytes)
    expect(native.decode(bytes).to_a).to eq(reference.decode(bytes).to_a)
  end

  it "recompiles a writer after its fields reader is replaced" do
    definition = record_schema("Pair", [field("left", "int"), field("right", "string")])
    id = registry.register("pairs", reference_schema(definition))
    value = { "left" => 1, "right" => "two" }
    [reference, native].each do |client|
      expect(client.decode(client.encode(value, schema_id: id))).to eq(value)
      schema = client.fetch_schema_by_id(id).first
      fields = schema.fields.reverse
      schema.define_singleton_method(:fields) { fields }
    end
    expect(native.encode(value, schema_id: id)).to eq(reference.encode(value, schema_id: id))
  end

  it "invalidates cached resolution when a reader's nested default changes" do
    writer = record_schema("Event", [])
    definition = record_schema("Event", [field("tags", { "type" => "array", "items" => "string" }, default: ["a"])])
    reader = reference_schema(definition)
    allow(store).to receive(:find).with("Reader", nil).and_return(reader)
    id = registry.register("events", reference_schema(writer))
    bytes = reference.encode({}, schema_id: id)
    expect(native.decode(bytes, schema_name: "Reader")).to eq("tags" => ["a"])
    reader.fields.first.default << "b"
    expected = reference.decode(bytes, schema_name: "Reader")
    expect(value_contract(native.decode(bytes, schema_name: "Reader"))).to eq(value_contract(expected))
    expect(expected).to eq("tags" => %w[a b])
  end

  it "requires an explicit reader default for a newly added nullable field" do
    writer = record_schema("Event", [])
    reader = reference_schema(record_schema("Event", [field("value", %w[null long])]))
    allow(store).to receive(:find).with("Reader", nil).and_return(reader)
    id = registry.register("events", reference_schema(writer))
    bytes = reference.encode({}, schema_id: id)
    expect { reference.decode(bytes, schema_name: "Reader") }.to raise_error(Avro::AvroError) do |error|
      expect { native.decode(bytes, schema_name: "Reader") }.to raise_error do |actual|
        expect(actual.class).to eq(error.class)
      end
    end
  end
end
