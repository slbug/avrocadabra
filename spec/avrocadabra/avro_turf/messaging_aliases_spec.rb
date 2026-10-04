# frozen_string_literal: true

require "support/schema_registry"

RSpec.describe Avrocadabra::AvroTurf::Messaging do
  include_context "with a schema registry"

  let(:store) { AvroTurfFixture.schema_store }
  let(:options) { { registry: registry, schema_store: store, logger: Logger.new(nil) } }
  let(:native) { described_class.new(**options) }
  let(:reference) { AvroTurf::Messaging.new(**options) }

  def compare_resolution(writer_fields, reader_fields, datum)
    writer = record_schema("Value", writer_fields)
    reader = record_schema("Value", reader_fields)
    allow(store).to receive(:find).with("Reader", nil).and_return(reference_schema(reader))
    id = registry.register("values", reference_schema(writer))
    bytes = reference.encode(datum, schema_id: id)
    expected = reference.decode(bytes, schema_name: "Reader")
    expect(value_contract(native.decode(bytes, schema_name: "Reader"))).to eq(value_contract(expected))
    expect(value_contract(native_schema(writer).decode(bytes.byteslice(5..), reader_schema: native_schema(reader))))
      .to eq(value_contract(expected))
    expected
  end

  [%w[old_a old_b], %w[old_b old_a], %w[current old_a], %w[old_a current]].each do |names|
    it "applies alias overwrites in writer order #{names.inspect}" do
      written = names.map { field(it, "long") }
      reader = [field("current", "long", aliases: %w[old_a old_b])]
      datum = names.zip([1, 2]).to_h
      expect(compare_resolution(written, reader, datum)).to eq("current" => 2)
    end
  end

  [false, true].each do |reverse|
    it "gives exact names precedence over aliases with reversed reader order #{reverse}" do
      written = [field("old", "long")]
      reader = [field("current", "long", aliases: ["old"], default: 7), field("old", "long")]
      reader.reverse! if reverse
      expect(compare_resolution(written, reader, { "old" => 1 }).to_a).to eq([["old", 1], ["current", 7]])
    end
  end

  it "keeps nested reader defaults aligned across overwritten records" do
    inner = record_schema("Inner", [field("id", "long")])
    evolved = record_schema("Inner", [field("id", "long"), field("flag", "boolean", default: true)])
    written = [field("old_a", inner), field("old_b", "Inner")]
    reader = [field("current", evolved, aliases: %w[old_a old_b]), field("tail", "long", default: 3)]
    datum = { "old_a" => { "id" => 1 }, "old_b" => { "id" => 2 } }
    expect(compare_resolution(written, reader, datum)).to eq("current" => { "id" => 2, "flag" => true }, "tail" => 3)
  end

  it "rejects an incompatible earlier alias even when a later value would overwrite it" do
    writer = record_schema("Value", [field("old_a", "long"), field("old_b", "string")])
    reader = record_schema("Value", [field("current", "string", aliases: %w[old_a old_b])])
    allow(store).to receive(:find).with("Reader", nil).and_return(reference_schema(reader))
    id = registry.register("values", reference_schema(writer))
    bytes = reference.encode({ "old_a" => 1, "old_b" => "last" }, schema_id: id)
    expect { reference.decode(bytes, schema_name: "Reader") }.to raise_error(Avro::IO::SchemaMatchException)
    expect { native.decode(bytes, schema_name: "Reader") }.to raise_error(Avro::IO::SchemaMatchException)
  end

  it "retains the first insertion position when later aliases overwrite a value" do
    written = %w[old_a middle old_b].map { field(it, "long") }
    reader = [field("middle", "long"), field("current", "long", aliases: %w[old_b old_a])]
    expect(compare_resolution(written, reader, { "old_a" => 1, "middle" => 2, "old_b" => 3 }).to_a)
      .to eq([["current", 3], ["middle", 2]])
  end

  it "decodes every overwritten value through its reader adapter" do
    writer = record_schema("Value", %w[old_a old_b].map { field(it, "long") })
    reader = reference_schema(record_schema("Value", [field("current", "long", aliases: %w[old_a old_b])]))
    calls = []
    adapter = Object.new
    allow(adapter).to receive(:decode) do |value|
      calls << value
      value * 10
    end
    reader.fields.first.type.instance_variable_set(:@type_adapter, adapter)
    allow(store).to receive(:find).with("Reader", nil).and_return(reader)
    id = registry.register("values", reference_schema(writer))
    bytes = reference.encode({ "old_a" => 1, "old_b" => 2 }, schema_id: id)
    results = [reference, native].map do |client|
      calls.clear
      [client.decode(bytes, schema_name: "Reader"), calls.dup]
    end
    expect(results).to eq([[{ "current" => 20 }, [1, 2]]] * 2)
  end
end
