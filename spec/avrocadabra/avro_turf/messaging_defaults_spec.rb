# frozen_string_literal: true

require "support/schema_registry"

RSpec.describe Avrocadabra::AvroTurf::Messaging do
  include_context "with a schema registry"

  let(:store) { AvroTurfFixture.schema_store }
  let(:options) { { registry: registry, schema_store: store, logger: Logger.new(nil) } }
  let(:native) { described_class.new(**options) }
  let(:reference) { AvroTurf::Messaging.new(**options) }

  def expect_defaults(fields, input: {}, written: [])
    writer = record_schema("Defaults", written)
    reader = reference_schema(record_schema("Defaults", fields))
    allow(store).to receive(:find).with("Reader", nil).and_return(reader)
    id = registry.register("defaults", reference_schema(writer))
    bytes = reference.encode(input, schema_id: id)
    actual = native.decode(bytes, schema_name: "Reader")
    expected = reference.decode(bytes, schema_name: "Reader")
    expect(value_contract(actual)).to eq(value_contract(expected))
    actual
  end

  [["float", 0.1], ["bytes", "é"], ["string", "é"],
   [{ "type" => "fixed", "name" => "Pair", "size" => 2 }, "é"],
   [{ "type" => "array", "items" => "float" }, [0.1]],
   [{ "type" => "map", "values" => "bytes" }, { "label" => "é" }],
   [{ "type" => "int", "logicalType" => "date" }, 1],
   [{ "type" => "bytes", "logicalType" => "decimal", "precision" => 4, "scale" => 2 }, "{"]].each do |type, value|
    it "preserves the value, class and encoding of a #{type.inspect} reader default" do
      expect_defaults([field("added", type, default: value)])
    end
  end

  it "uses Ruby Avro's truthiness fallback inside record defaults" do
    child = record_schema("Child", [field("flag", "boolean", default: true), field("number", "float", default: 0.1)])
    result = expect_defaults([field("child", child, default: { "flag" => false, "number" => 0.2 })])
    expect(result).to eq("child" => { "flag" => true, "number" => 0.2 })
  end

  it "preserves Ruby Avro's sentinel for a false nested default without a field default" do
    child = record_schema("Child", [field("flag", "boolean")])
    expect(expect_defaults([field("child", child, default: { "flag" => false })]))
      .to eq("child" => { "flag" => :no_default })
  end

  it "distinguishes nested defaults from values actually written" do
    old = record_schema("Child", [field("number", "float")])
    day = field("day", { "type" => "int", "logicalType" => "date" }, default: 1)
    current = record_schema("Child", [field("number", "float"), day])
    written = [field("children", { "type" => "array", "items" => old })]
    fields = [field("children", { "type" => "array", "items" => current }),
              field("extra", "Child", default: { "number" => 0.1, "day" => 2 }), field("tail", "float", default: 0.3)]
    expect_defaults(fields, written: written, input: { "children" => [{ "number" => 0.4 }, { "number" => 0.5 }] })
  end

  it "preserves mutable default string identity" do
    result = expect_defaults([field("label", "bytes", default: "é")])
    reader = store.find("Reader", nil)
    expect(result.fetch("label")).to equal(reader.fields.first.default)
  end

  it "ignores extra keys inside record defaults during resolution" do
    child = record_schema("Child", [field("id", "long")])
    expect(expect_defaults([field("child", child, default: { "id" => 1, "obsolete" => "leftover" })]))
      .to eq("child" => { "id" => 1 })
  end

  it "encodes supplied values when an unused record default contains extra keys" do
    child = record_schema("Child", [field("id", "long")])
    schema = record_schema("Defaults", [field("child", child, default: { "id" => 1, "obsolete" => "leftover" })])
    id = registry.register("defaults", reference_schema(schema))
    input = { "child" => { "id" => 2 } }
    expect(native.encode(input, schema_id: id)).to eq(reference.encode(input, schema_id: id))
  end
end
