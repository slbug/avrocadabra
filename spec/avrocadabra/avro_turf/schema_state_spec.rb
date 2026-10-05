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

  it "checks every reader across generated check chunks" do
    fields = Array.new(120) { field("value#{it}", "long") }
    schema = reference_schema(record_schema("Wide", fields))
    state = described_class.new(schema)
    expect(state).to be_current
    schema.fields.last.instance_variable_set(:@name, "renamed")
    expect(state).not_to be_current
  end

  it "reports generated reader checks at their source line" do
    schema = reference_schema("int")
    state = described_class.new(schema)
    allow(schema).to receive(:type_sym).and_raise(IOError, "reader failed")
    source = File.expand_path("../../../lib/avrocadabra/avro_turf/schema_state.rb", __dir__)
    line = File.readlines(source).index { it.include?("instance_eval(source") } + 2
    expect { state.current? }.to raise_error(IOError) do |error|
      expect(error.backtrace_locations.find { it.path == source }.lineno).to eq(line)
    end
  end
end
