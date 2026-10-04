# frozen_string_literal: true

require "support/schema_registry"

RSpec.describe Avrocadabra::AvroTurf::Validation do
  include_context "with a schema registry"

  let(:definition) do
    choices = %w[Left Right].map { record_schema(it, [field("node", "Node")]) }
    record_schema("Node", [field("value", "int"), field("next", ["null", *choices])])
  end
  let(:options) { { registry: registry, logger: Logger.new(nil) } }
  let(:reference) { AvroTurf::Messaging.new(**options) }
  let(:native) { Avrocadabra::AvroTurf::Messaging.new(**options) }
  let(:schema_id) { registry.register("bounded", reference_schema(definition)) }

  def rejected_tree(counter)
    leaf = Hash.new do
      counter[0] += 1
      "invalid"
    end
    8.times.reduce(leaf) { |child, _index| { "value" => 1, "next" => { "node" => child } } }
  end

  [false, true].each do |validate|
    it "bounds Messaging union search with validate: #{validate}" do
      stub_const("Avrocadabra::Schema::MAX_ITEMS", 100)
      counter = [0]
      expect { native.encode(rejected_tree(counter), schema_id: schema_id, validate: validate) }
        .to raise_error(Avro::IO::AvroTypeError) { expect(it.cause.message).to include("maximum item count") }
      expect(counter.first).to be < 100
    end
  end

  it "leaves stock validation and its exception unchanged" do
    stub_const("Avrocadabra::Schema::MAX_ITEMS", 100)
    counter = [0]
    expect { reference.encode(rejected_tree(counter), schema_id: schema_id) }.to raise_error(Avro::IO::AvroTypeError)
    expect(counter.first).to be > 100
    expect(Thread.current[:avrocadabra_codecs]).to be_nil
  end

  it "shares a caller's work budget across branch attempts" do
    schema = reference_schema(["null", { "type" => "array", "items" => "long" }])
    mapping = Avrocadabra::AvroTurf::Mapping.new(schema, Avrocadabra::AvroTurf::SchemaState.new(schema).schemas)
    budget = [4, 10]
    expect(mapping.union_index(schema, [1, 2], budget)).to eq(1)
    expect(budget).to eq([0, 10])
    expect { mapping.union_index(schema, [3], budget) }.to raise_error(Avro::IO::AvroTypeError)
  end

  it "bounds depth while restoring the caller's depth budget after errors" do
    schema = reference_schema({ "type" => "array", "items" => { "type" => "array", "items" => "long" } })
    budget = [100, 1]
    expect { Avro::Schema.validate(schema, [[1]], avrocadabra_budget: budget) }
      .to raise_error(Avro::IO::AvroTypeError) { expect(it.cause.message).to include("maximum depth") }
    expect(budget).to eq([98, 1])
  end

  it "accepts values at the native depth limit with validation enabled" do
    schema = record_schema("Link", [field("next", %w[null Link])])
    id = registry.register("depth", reference_schema(schema))
    datum = 64.times.reduce(nil) { |child, _| { "next" => child } }
    bytes = reference.encode(datum, schema_id: id, validate: true)
    expect(native.encode(datum, schema_id: id, validate: true)).to eq(bytes)
    expect { native.encode({ "next" => datum }, schema_id: id, validate: true) }
      .to raise_error(Avro::IO::AvroTypeError)
  end
end
