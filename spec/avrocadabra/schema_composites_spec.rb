# frozen_string_literal: true

RSpec.describe Avrocadabra::Schema do
  it "cross-decodes nested records, arrays, maps, enums and fixed" do
    definition = record_schema("Event", [
                                 field("id", "long"),
                                 field("status", { "type" => "enum", "name" => "Status", "symbols" => %w[NEW DONE] }),
                                 field("digest", { "type" => "fixed", "name" => "Digest", "size" => 4 }),
                                 field("tags", { "type" => "array", "items" => "string" }),
                                 field("counts", { "type" => "map", "values" => "int" }),
                                 field("child", record_schema("Child", [field("enabled", "boolean")]))
                               ], namespace: "example.events")
    datum = { "id" => 2**40, "status" => "DONE", "digest" => "\x00\xff\x80\x7f".b,
              "tags" => %w[a 日本語], "counts" => { "z" => -2, "a" => 3 }, "child" => { "enabled" => false } }
    expect_interoperable(definition, datum)
    decoded = native_schema(definition).decode(reference_encode(definition, datum))
    expect(decoded.fetch("digest").encoding).to eq(Encoding::BINARY)
    expect(decoded.keys).to all(be_a(String))
    expect(decoded.fetch("counts").keys).to all(be_a(String))
  end

  it "preserves empty containers" do
    expect_interoperable({ "type" => "array", "items" => "null" }, [])
    expect_interoperable({ "type" => "map", "values" => "null" }, {})
    expect_interoperable(record_schema("Empty", []), {})
  end

  it "accepts symbol record keys with string keys taking precedence even for false and nil" do
    definition = record_schema("Keys", [field("flag", "boolean"), field("optional", %w[null string])])
    datum = { flag: true, "flag" => false, optional: "symbol", "optional" => nil }.freeze
    schema = native_schema(definition)
    expect(schema.decode(schema.encode(datum))).to eq("flag" => false, "optional" => nil)
    expect(schema.decode(schema.encode({ flag: false, optional: nil }))).to eq("flag" => false, "optional" => nil)
  end

  it "allows omitted nullable fields and requires other writer fields" do
    definition = record_schema("Required", [field("flag", "boolean", default: false),
                                            field("optional", %w[string null])])
    schema = native_schema(definition)
    expect { schema.encode({}) }.to raise_error(Avrocadabra::EncodeError, /flag/)
    expect(schema.decode(schema.encode({ flag: false }))).to eq("flag" => false, "optional" => nil)
    expect(schema.decode(schema.encode({ flag: false, optional: nil }))).to eq("flag" => false, "optional" => nil)
  end

  it "includes nested field names in conversion errors" do
    definition = record_schema("Envelope", [field("items", { "type" => "array", "items" =>
      record_schema("Item", [field("count", "int")]) })])
    expect { native_schema(definition).encode({ items: [{ count: "bad" }] }) }
      .to raise_error(Avrocadabra::EncodeError, /items.*count/)
  end

  it "rejects symbol map keys and invalid UTF-8 map keys" do
    schema = native_schema({ "type" => "map", "values" => "long" })
    expect { schema.encode({ key: 1 }) }.to raise_error(Avrocadabra::EncodeError)
    expect { schema.encode({ "\xff".b => 1 }) }.to raise_error(Avrocadabra::EncodeError)
  end

  it "validates enum symbols and fixed lengths" do
    enum = native_schema({ "type" => "enum", "name" => "Status", "symbols" => %w[NEW DONE] })
    fixed = native_schema({ "type" => "fixed", "name" => "Pair", "size" => 2 })
    ["MISSING", :NEW, 1].each do |value|
      expect { enum.encode(value) }.to raise_error(Avrocadabra::EncodeError)
    end
    ["", "x", "xxx", nil].each do |value|
      expect { fixed.encode(value) }.to raise_error(Avrocadabra::EncodeError)
    end
    expect_interoperable({ "type" => "fixed", "name" => "Pair", "size" => 2 }, "\x00\xff".b)
  end

  it "matches enum values using their original bytes and encoding" do
    definition = { "type" => "enum", "name" => "Letter", "symbols" => %w[A B] }
    [Encoding::UTF_8, Encoding::US_ASCII, Encoding::BINARY, Encoding::ISO_8859_1].each do |encoding|
      expect_interoperable(definition, "A".encode(encoding))
    end
    ["A".encode(Encoding::UTF_16LE), "A".dup.force_encoding(Encoding::UTF_16LE)].each do |value|
      expect { reference_encode(definition, value) }.to raise_error(Avro::IO::AvroTypeError)
      expect { native_schema(definition).encode(value) }.to raise_error(Avrocadabra::EncodeError)
    end
  end

  it "resolves repeated named definitions within their namespace" do
    child = record_schema("Child", [field("value", "long")])
    definition = record_schema("Parent", [field("first", child), field("second", "Child")], namespace: "example")
    expect_interoperable(definition, { "first" => { "value" => 1 }, "second" => { "value" => 2 } })
  end

  it "cross-decodes recursive named records" do
    definition = record_schema("Node", [field("value", "int"), field("next", %w[null Node])], namespace: "tree")
    datum = { "value" => 1, "next" => { "value" => 2, "next" => { "value" => 3, "next" => nil } } }
    expect_interoperable(definition, datum)
  end

  it "does not mutate frozen nested inputs" do
    definition = record_schema("Frozen", [
                                 field("texts", { "type" => "array", "items" => "string" }),
                                 field("blob", "bytes"), field("counts", { "type" => "map", "values" => "int" })
                               ])
    datum = { "texts" => %w[a 日本語].freeze,
              "blob" => "\x00\xff".b.freeze, "counts" => { "one" => 1 }.freeze }.freeze
    snapshot = Marshal.dump(datum)
    expect_interoperable(definition, datum)
    expect(Marshal.dump(datum)).to eq(snapshot)
    expect(datum.fetch("blob").encoding).to eq(Encoding::BINARY)
  end

  it "rejects incorrect composite representations" do
    [{ "type" => "array", "items" => "int" }, { "type" => "map", "values" => "int" },
     record_schema("Record", [field("value", "int")])].each do |definition|
      expect { native_schema(definition).encode(Object.new) }.to raise_error(Avrocadabra::EncodeError)
    end
  end
end
