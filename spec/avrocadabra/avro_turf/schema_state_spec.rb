# frozen_string_literal: true

require "support/avro_turf_fixture"

RSpec.describe Avrocadabra::AvroTurf::SchemaState do
  it "observes cyclic defaults without recursive snapshot expansion" do
    schema = reference_schema(record_schema("Recursive", [field("items", { "type" => "array", "items" => "long" },
                                                                default: [])]))
    cycle = []
    cycle << cycle
    schema.fields.first.instance_variable_set(:@default, cycle)
    state = described_class.new(schema)
    expect(state).to be_current
    cycle << 1
    expect(state).not_to be_current
  end
end
