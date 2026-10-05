# frozen_string_literal: true

require "support/schema_registry"

RSpec.describe Avrocadabra::AvroTurf::Messaging do
  include_context "with a schema registry"

  let(:definition) do
    choices = %w[Left Right].map { record_schema(it, [field("node", "Node")]) }
    record_schema("Node", [field("value", "int"), field("next", ["null", *choices])])
  end
  let(:options) { { registry: registry, logger: Logger.new(nil) } }
  let(:reference) { AvroTurf::Messaging.new(**options) }
  let(:native) { described_class.new(**options) }
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

  it "charges rejected union branches to the work budget" do
    stub_const("Avrocadabra::Schema::MAX_ITEMS", 100)
    branches = Array.new(20) { { "type" => "fixed", "name" => "Size#{it}", "size" => it + 1 } }
    id = registry.register("fixed", reference_schema({ "type" => "array", "items" => branches }))
    expect { native.encode(Array.new(10) { "x" * 20 }, schema_id: id) }.to raise_error(Avro::IO::AvroTypeError)
    expect(native.encode(Array.new(2) { "x" * 20 }, schema_id: id)).to eq(reference.encode(Array.new(2) {
      "x" * 20
    }, schema_id: id))
  end

  it "leaves stock validation and its exception unchanged" do
    stub_const("Avrocadabra::Schema::MAX_ITEMS", 100)
    counter = [0]
    expect { reference.encode(rejected_tree(counter), schema_id: schema_id) }.to raise_error(Avro::IO::AvroTypeError)
    expect(counter.first).to be > 100
    expect(Thread.current[:avrocadabra_codecs]).to be_nil
  end

  it "restores the depth budget after a rejected union branch" do
    stub_const("Avrocadabra::Schema::MAX_DEPTH", 3)
    nested = lambda do |name, leaf|
      record_schema(name, [field("v", { "type" => "array", "items" => { "type" => "array", "items" => leaf } })])
    end
    id = registry.register("nested", reference_schema([nested.call("Longs", "long"), nested.call("Texts", "string")]))
    datum = { "v" => [["x"]] }
    expect(native.encode(datum, schema_id: id, validate: true))
      .to eq(reference.encode(datum, schema_id: id, validate: true))
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
