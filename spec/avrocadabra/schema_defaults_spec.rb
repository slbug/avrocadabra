# frozen_string_literal: true

RSpec.describe Avrocadabra::Schema do
  let(:gvl_modes) { [false, true] }

  it "preserves supplied false and nil and gives string keys precedence" do
    definition = record_schema("Defaults", [field("enabled", "boolean", default: true),
                                            field("label", %w[string null], default: "default")])
    input = { "enabled" => false, enabled: true, "label" => nil, label: "symbol" }.freeze
    expect_interoperable(definition, input, expected: { "enabled" => false, "label" => nil })
  end

  it "writes nil for omitted nullable fields regardless of the schema default" do
    definition = record_schema("Defaults", [field("value", %w[long null], default: 7)])
    schema = native_schema(definition)
    expect(schema.encode({}.freeze)).to eq(reference_encode(definition, {}))
    expect(schema.decode(schema.encode({}))).to eq("value" => nil)
  end

  it "rejects omitted required fields even when the schema supplies a default" do
    definition = record_schema("Defaults", [field("value", "long", default: 7)])
    expect { reference_encode(definition, {}) }.to raise_error(Avro::IO::AvroTypeError)
    expect { native_schema(definition).encode({}) }.to raise_error(Avrocadabra::EncodeError, /value/)
  end

  it "honors Hash default values and procs through the symbol lookup" do
    definition = record_schema("Defaults", [field("value", "long", default: 7)])
    [Hash.new(12), Hash.new { |_hash, key| key == :value ? 34 : 56 }].each do |input|
      expect(native_schema(definition).encode(input.freeze)).to eq(reference_encode(definition, input))
      expect(input).to be_empty
    end
  end

  it "looks up missing nested fields without filling writer schema defaults" do
    child = record_schema("Child", [field("count", "int", default: 7), field("next", %w[null Child])])
    definition = record_schema("Parent", [field("child", child)])
    input = { child: Hash.new(9).merge(next: nil).freeze }.freeze
    expect_interoperable(definition, input, expected: { "child" => { "count" => 9, "next" => nil } })
  end

  it "requires explicit reader defaults for new nullable fields" do
    writer = record_schema("Evolution", [])
    reader = record_schema("Evolution", [field("added", %w[null long])])
    expect { reference_decode(writer, "".b, reader: reader) }.to raise_error(Avro::AvroError)
    expect { native_schema(writer).decode("".b, reader_schema: native_schema(reader)) }
      .to raise_error(Avrocadabra::ResolutionError, /no default/)
  end

  [["float", 0.1], %w[bytes é], [{ "type" => "fixed", "name" => "Pair", "size" => 2 }, "é"]].each do |type, value|
    it "preserves Ruby Avro's reader default for #{type.inspect}" do
      writer = record_schema("Defaults", [])
      reader = record_schema("Defaults", [field("value", type, default: value)])
      expected = reference_decode(writer, "".b, reader: reader)
      gvl_modes.each do |release_gvl|
        actual = native_schema(writer).decode("".b, reader_schema: native_schema(reader), release_gvl: release_gvl)
        expect(value_contract(actual)).to eq(value_contract(expected))
      end
    end
  end

  it "preserves default positions through nested records and collections" do
    child = record_schema("Child", [field("value", "float")])
    evolved = record_schema("Child", [field("value", "float"), field("extra", "float", default: 0.1)])
    writer = record_schema("Outer", [field("items", { "type" => "array", "items" => child })])
    reader = record_schema("Outer", [field("items", { "type" => "array", "items" => evolved }),
                                     field("child", "Child", default: { "value" => 0.2, "extra" => 0.3 }),
                                     field("tail", "bytes", default: "é")])
    bytes = reference_encode(writer, { "items" => [{ "value" => 0.4 }, { "value" => 0.5 }] })
    expected = reference_decode(writer, bytes, reader: reader)
    expect(value_contract(native_schema(writer).decode(bytes, reader_schema: native_schema(reader))))
      .to eq(value_contract(expected))
  end

  ["boolean", %w[boolean null], %w[null boolean]].each do |type|
    it "matches nested false defaults without a field default for #{type.inspect}" do
      child = record_schema("Child", [field("flag", type)])
      writer = record_schema("Defaults", [])
      reader = record_schema("Defaults", [field("child", child, default: { "flag" => false })])
      expected = reference_decode(writer, "".b, reader: reader)
      gvl_modes.each do |release_gvl|
        actual = native_schema(writer).decode("".b, reader_schema: native_schema(reader), release_gvl: release_gvl)
        expect(value_contract(actual)).to eq(value_contract(expected))
      end
    end
  end

  it "ignores extra record default keys when preparing and resolving schemas" do
    child = record_schema("Child", [field("id", "long")])
    reader = record_schema("Defaults", [field("child", child, default: { "id" => 1, "obsolete" => "leftover" })])
    schema = native_schema(reader)
    input = { "child" => { "id" => 2 } }
    expect(schema.encode(input)).to eq(reference_encode(reader, input))
    writer = record_schema("Defaults", [])
    expect(native_schema(writer).decode("".b,
                                        reader_schema: schema)).to eq(reference_decode(writer, "".b, reader: reader))
  end

  it "materializes nested defaults without sharing mutable results between reads" do
    child = record_schema("Child", [field("flag", "boolean")])
    writer = native_schema(record_schema("Defaults", []))
    reader = native_schema(record_schema("Defaults", [
                                           field("children", { "type" => "array", "items" => child },
                                                 default: [{ "flag" => false }]),
                                           field("labels", { "type" => "map", "values" => "bytes" },
                                                 default: { "z" => "é", "a" => "a" })
                                         ]))
    first = writer.decode("".b, reader_schema: reader)
    first.fetch("children").first["flag"] = true
    first.fetch("labels").fetch("z").replace("changed")
    second = writer.decode("".b, reader_schema: reader)
    expect(second).to eq("children" => [{ "flag" => :no_default }], "labels" => { "z" => "é", "a" => "a" })
    expect(second.fetch("labels").keys).to eq(%w[z a])
  end

  it "counts logical defaults once against the decoded item limit" do
    writer = native_schema(record_schema("Defaults", []), max_items: 2)
    date = { "type" => "int", "logicalType" => "date" }
    reader = native_schema(record_schema("Defaults", [field("day", date, default: 1)]), max_items: 2)
    gvl_modes.each do |release_gvl|
      expect(writer.decode("".b, reader_schema: reader, release_gvl: release_gvl))
        .to eq("day" => Date.new(1970, 1, 2))
    end
  end
end
