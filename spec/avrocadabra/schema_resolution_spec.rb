# frozen_string_literal: true

RSpec.describe Avrocadabra::Schema do
  it "adds reader defaults, drops writer fields and reads reordered fields" do
    writer = record_schema("Event", [field("id", "int"), field("removed", "string"), field("flag", "boolean")])
    reader = record_schema("Event", [field("flag", "boolean"), field("label", "string", default: "new"),
                                     field("id", "long"), field("optional", %w[null string], default: nil)])
    datum = { "id" => 123, "removed" => "old", "flag" => false }
    expected = { "flag" => false, "label" => "new", "id" => 123, "optional" => nil }
    schema = native_schema(writer)
    evolved = native_schema(reader)
    expect(schema.decode(reference_encode(writer, datum), reader_schema: evolved)).to eq(expected)
    expect(reference_decode(writer, schema.encode(datum), reader: reader)).to eq(expected)
    expect(schema.decode(schema.encode(datum), reader_schema: evolved)).to eq(expected)
  end

  it "does not share mutable reader defaults between decoded records" do
    writer = record_schema("Defaults", [])
    reader = record_schema("Defaults",
                           [field("tags", { "type" => "array", "items" => "string" }, default: ["initial"])])
    schema = native_schema(writer)
    evolved = native_schema(reader)
    first = schema.decode("".b, reader_schema: evolved)
    first.fetch("tags") << "changed"
    expect(schema.decode("".b, reader_schema: evolved)).to eq("tags" => ["initial"])
  end

  it "does not interpret arbitrary map default keys as schema annotations" do
    writer = native_schema(record_schema("Defaults", []))
    default = { "logicalType" => "decimal", "precision" => "plain data", "type" => "big-decimal" }
    reader = native_schema(record_schema("Defaults", [
                                           field("value", { "type" => "map", "values" => "string" }, default: default)
                                         ]))
    expect(writer.decode("".b, reader_schema: reader)).to eq("value" => default)
  end

  it "resolves named and field aliases" do
    writer = record_schema("OldRecord", [field("value", "string")], namespace: "v1")
    reader = record_schema("NewRecord", [field("renamed", "string", aliases: ["value"])],
                           namespace: "v2", aliases: ["v1.OldRecord"])
    datum = { "value" => "text" }
    schema = native_schema(writer)
    expected = { "renamed" => "text" }
    expect(schema.decode(reference_encode(writer, datum), reader_schema: native_schema(reader))).to eq(expected)
    expect(reference_decode(writer, schema.encode(datum), reader: reader)).to eq(expected)
  end

  it "rejects unrelated record names even when fields are identical" do
    first = native_schema(record_schema("First", [field("value", "int")]))
    second = native_schema(record_schema("Second", [field("value", "int")]))
    expect { first.decode(first.encode({ value: 1 }), reader_schema: second) }.to raise_error(Avrocadabra::DecodeError)
  end

  it "requires explicit defaults for missing reader fields, including nullable fields" do
    writer = native_schema(record_schema("Event", []))
    nullable = native_schema(record_schema("Event", [field("note", %w[string null])]))
    expect { writer.decode("".b, reader_schema: nullable) }.to raise_error(Avrocadabra::ResolutionError, /note/)
    reader = native_schema(record_schema("Event", [field("required", "string")]))
    expect { writer.decode("".b, reader_schema: reader) }.to raise_error(Avrocadabra::DecodeError, /required/)
  end

  { "int" => %w[long float double], "long" => %w[float double], "float" => ["double"] }.each do |writer, readers|
    readers.each do |reader|
      it "promotes #{writer} to #{reader}" do
        schema = native_schema(writer)
        value = writer == "float" ? 1.25 : 42
        bytes = reference_encode(writer, value)
        expected = reference_decode(writer, bytes, reader: reader)
        decoded = schema.decode(bytes, reader_schema: native_schema(reader))
        expect(decoded).to eq(expected)
        expect(decoded.class).to eq(expected.class)
      end
    end
  end

  [["long", "int", 1], ["double", "float", 1.0], ["double", "long", 1.0],
   %w[string int 1]].each do |writer, reader, value|
    it "rejects #{writer} to #{reader} demotion or incompatible resolution" do
      schema = native_schema(writer)
      expect { schema.decode(schema.encode(value), reader_schema: native_schema(reader)) }
        .to raise_error(Avrocadabra::DecodeError)
    end
  end

  it "preserves the writer string encoding during string/bytes promotion like Ruby Avro" do
    [%w[string bytes 日本語], ["bytes", "string", "日本語".b],
     ["bytes", "string", "\xff".b]].each do |writer, reader, value|
      bytes = reference_encode(writer, value)
      expected = reference_decode(writer, bytes, reader: reader)
      actual = native_schema(writer).decode(bytes, reader_schema: native_schema(reader))
      expect(actual).to eq(expected)
      expect(actual.encoding).to eq(expected.encoding)
    end
  end

  it "resolves writer and reader unions by matching their branches" do
    writer = native_schema(%w[null int])
    reader = native_schema(%w[string null long])
    [nil, 12].each do |value|
      expect(writer.decode(writer.encode(value), reader_schema: reader)).to eq(value)
    end
    int = native_schema("int")
    expect(int.decode(int.encode(12), reader_schema: reader)).to eq(12)
    expect(writer.decode(writer.encode(12), reader_schema: native_schema("long"))).to eq(12)
    expect { writer.decode(writer.encode(nil), reader_schema: native_schema("long")) }.to raise_error(Avrocadabra::DecodeError)
  end

  it "uses an enum reader default for removed symbols" do
    writer = { "type" => "enum", "name" => "State", "symbols" => %w[NEW OLD] }
    reader = { "type" => "enum", "name" => "State", "symbols" => %w[NEW UNKNOWN], "default" => "UNKNOWN" }
    schema = native_schema(writer)
    expect(schema.decode(reference_encode(writer, "OLD"), reader_schema: native_schema(reader))).to eq("UNKNOWN")
    expect(reference_decode(writer, schema.encode("OLD"), reader: reader)).to eq("UNKNOWN")
    reader.delete("default")
    expect(schema.decode(schema.encode("OLD"), reader_schema: native_schema(reader)))
      .to eq(reference_decode(writer, schema.encode("OLD"), reader: reader))
  end

  it "checks fixed names and sizes while accepting reader aliases" do
    writer = native_schema({ "type" => "fixed", "name" => "Old", "size" => 2 })
    compatible = native_schema({ "type" => "fixed", "name" => "New", "aliases" => ["Old"], "size" => 2 })
    bytes = writer.encode("ok")
    expect(writer.decode(bytes, reader_schema: compatible)).to eq("ok".b)
    [native_schema({ "type" => "fixed", "name" => "Old", "size" => 3 }),
     native_schema({ "type" => "fixed", "name" => "Other", "size" => 2 })].each do |reader|
      expect { writer.decode(bytes, reader_schema: reader) }.to raise_error(Avrocadabra::DecodeError)
    end
  end
end
