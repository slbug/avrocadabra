# frozen_string_literal: true

require "support/avro_turf_fixture"

RSpec.describe Avrocadabra::AvroTurf::Mapping do
  def mapping(schema)
    described_class.new(schema, Avrocadabra::AvroTurf::SchemaState.new(schema).schemas)
  end

  it "prepares recursive adapter graphs without copying the schema" do
    schema = reference_schema(record_schema("Node", [field("next", %w[null Node])]))
    context, node = mapping(schema).native
    expect(node.first).to equal(schema)
    expect(node.last.first.last.last).to equal(node)
    expect(node).to be_frozen
    expect(context.native.first).to equal(context)
  end

  it "materializes defaults with the reader's logical adapter" do
    schema = reference_schema(record_schema("Day", [field("day", { "type" => "int", "logicalType" => "date" },
                                                          default: 1)]))
    expect(mapping(schema).default_value(schema, "day")).to eq(Date.new(1970, 1, 2))
  end
end
