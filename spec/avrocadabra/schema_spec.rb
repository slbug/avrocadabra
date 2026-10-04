# frozen_string_literal: true

RSpec.describe Avrocadabra::Schema do
  it "exposes version 0.0.2 and distinct public errors" do
    expect(Avrocadabra::VERSION).to eq("0.0.2")
    errors = [Avrocadabra::SchemaError, Avrocadabra::EncodeError, Avrocadabra::DecodeError]
    expect(errors).to all(be < Avrocadabra::Error)
    expect(errors).to all(be < StandardError)
  end

  it "accepts JSON and string or symbol keyed schema hashes without changing them" do
    definition = { type: "record", name: "Flag", fields: [{ name: "enabled", type: "boolean" }] }
    snapshot = Marshal.dump(definition)
    [JSON.generate(definition), definition].each do |input|
      schema = described_class.new(input)
      expect(schema.decode(schema.encode({ enabled: false }))).to eq("enabled" => false)
    end
    expect(Marshal.dump(definition)).to eq(snapshot)
  end

  it "owns a prepared schema independently of mutable constructor arguments" do
    definition = { "type" => "record", "name" => "Owned", "fields" => [field("value", "string")] }
    schema = described_class.new(definition)
    definition.fetch("fields").clear
    definition["name"] = "Changed"
    expect(schema.decode(schema.encode({ "value" => "original" }))).to eq("value" => "original")
  end

  it "accepts frozen schema JSON and deeply frozen schema hashes" do
    definition = { "type" => "array", "items" => "string" }.freeze
    [definition, JSON.generate(definition).freeze].each do |input|
      schema = described_class.new(input)
      expect(schema.decode(schema.encode(["frozen"].freeze))).to eq(["frozen"])
    end
  end

  it "accepts a union supplied as a Ruby schema array" do
    schema = described_class.new(%w[null string])
    expect(schema.decode(schema.encode("text"))).to eq("text")
    expect(schema.decode(schema.encode(nil))).to be_nil
  end

  it "rejects references supplied in a non-array container" do
    expect { described_class.new('"int"', references: {}) }.to raise_error(Avrocadabra::SchemaError, /references/)
  end

  it "requires a prepared reader schema" do
    schema = native_schema("int")
    expect { schema.decode(schema.encode(1), reader_schema: '"long"') }
      .to raise_error(Avrocadabra::DecodeError, /reader_schema/)
  end

  [nil, 12, :int, "not json", "{}", "{", '"missing.Name"', "[]", '["int", "int"]',
   '{"type":"fixed","name":"Zero","size":-1}',
   '{"type":"enum","name":"Duplicate","symbols":["X","X"]}'].each do |input|
    it "rejects invalid schema #{input.inspect}" do
      expect { described_class.new(input) }.to raise_error(Avrocadabra::SchemaError)
    end
  end

  it "rejects duplicate record fields" do
    definition = record_schema("Duplicate", [field("value", "int"), field("value", "int")])
    expect { described_class.new(definition) }.to raise_error(Avrocadabra::SchemaError)
  end

  it "rejects excessively nested schema input safely" do
    definition = "int"
    150.times { definition = { "type" => "array", "items" => definition } }
    expect { described_class.new(definition) }.to raise_error(Avrocadabra::SchemaError)
  end

  it "rejects excessive schema JSON size before parsing" do
    input = JSON.generate({ "type" => "string", "doc" => "x" * 1_048_577 })
    expect { described_class.new(input) }.to raise_error(Avrocadabra::SchemaError)
  end

  it "rejects excessive dependency depth without aborting the Ruby process" do
    code = <<~RUBY
      references = Array.new(512) do |index|
        child = index == 511 ? "null" : "Node\#{index + 1}"
        {type: "record", name: "Node\#{index}", fields: [{name: "next", type: child}]}
      end
      begin
        Avrocadabra::Schema.new(references.shift, references: references)
        puts "unexpected success"
      rescue Avrocadabra::SchemaError
        puts "rejected"
      end
    RUBY
    output, errors, status = ruby_subprocess(code)
    expect(status.success?).to be(true), errors
    expect(output.strip).to eq("rejected")
  end

  it "does not interpret custom metadata as nested Avro schemas" do
    definition = { "type" => "string", "custom" => { "logicalType" => "big-decimal" } }
    schema = described_class.new(definition)
    expect(schema.decode(schema.encode("value"))).to eq("value")
  end

  [{ max_depth: 0 }, { max_depth: 129 }, { max_bytes: 0 }, { max_bytes: 67_108_865 },
   { max_items: 0 }, { max_items: 1_000_001 }].each do |options|
    it "rejects unsafe limits #{options}" do
      expect { native_schema("int", **options) }.to raise_error(Avrocadabra::SchemaError)
    end
  end

  it "resolves explicit named dependencies and owns them after construction" do
    dependency = record_schema("Nested", [field("value", "string")], namespace: "example")
    definition = record_schema("Wrapper", [field("nested", "example.Nested")], namespace: "example")
    schema = described_class.new(definition, references: [dependency])
    dependency.fetch("fields").clear
    GC.start
    expect(schema.decode(schema.encode({ nested: { value: "sample" } }))).to eq("nested" => { "value" => "sample" })
  end

  it "resolves dependency chains regardless of references order" do
    leaf = record_schema("Leaf", [field("text", "string")], namespace: "example")
    middle = record_schema("Middle", [field("leaf", "example.Leaf")], namespace: "example")
    top = record_schema("Top", [field("middle", "example.Middle")], namespace: "example")
    schema = described_class.new(top, references: [JSON.generate(middle), leaf])
    datum = { "middle" => { "leaf" => { "text" => "ok" } } }
    expect(schema.decode(schema.encode(datum))).to eq(datum)
  end

  it "rejects unresolved or conflicting named dependencies" do
    definition = record_schema("Top", [field("value", "example.Missing")])
    expect { described_class.new(definition) }.to raise_error(Avrocadabra::SchemaError)
    first = record_schema("Same", [field("value", "int")])
    second = record_schema("Same", [field("value", "string")])
    expect { described_class.new(first, references: [second]) }.to raise_error(Avrocadabra::SchemaError)
  end
end
