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

  it "selects union branches using Ruby Avro validation" do
    schema = reference_schema(["null", { "type" => "int", "logicalType" => "date" }, "string"])
    adapter = mapping(schema)
    expect(adapter.union_index(schema, Date.new(2000, 1, 1), [100, 10])).to eq(1)
    expect(adapter.union_index(schema, "today", [100, 10])).to eq(2)
    expect { adapter.union_index(schema, [], [100, 10]) }.to raise_error(Avro::IO::AvroTypeError)
  end

  it "charges rejected null branches one budget item" do
    schema = reference_schema(%w[null string])
    adapter = mapping(schema)
    budget = [3, 10]
    allow(Avro::Schema).to receive(:validate).and_call_original
    expect(adapter.union_index(schema, nil, budget)).to eq(0)
    expect(adapter.union_index(schema, "value", budget)).to eq(1)
    expect(budget).to eq([0, 10])
    expect(Avro::Schema).not_to have_received(:validate).with(schema.schemas.first, "value", anything)
    expect { adapter.union_index(schema, "value", budget) }
      .to raise_error(Avro::IO::AvroTypeError, /schema "null"/) { expect(it.cause.message).to include("maximum item") }
  end

  it "recognizes stock Ruby Avro and bigdecimal decimal methods" do
    expect(mapping(reference_schema("int"))).to be_native_decimal
  end

  it "trusts built-in module adapters only for values they convert natively" do
    adapter = mapping(reference_schema("int"))
    expect([Integer, Float, Time].map { adapter.stock_modules?(it) }).to all(be(true))
    expect([Date, DateTime, BigDecimal, String, Symbol, Rational].map { adapter.stock_modules?(it) }).to all(be(false))
  end

  it "materializes defaults with the reader's logical adapter" do
    schema = reference_schema(record_schema("Day", [field("day", { "type" => "int", "logicalType" => "date" },
                                                          default: 1)]))
    expect(mapping(schema).default_value(schema, "day")).to eq(Date.new(1970, 1, 2))
  end
end
