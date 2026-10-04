# frozen_string_literal: true

RSpec.describe Avrocadabra::Union do
  it "infers branches when Ruby representations are distinct" do
    definition = ["null", "boolean", "int", "string", { "type" => "array", "items" => "long" }]
    [nil, false, 42, "hello", [1, 2]].each do |value|
      expect_interoperable(definition, value)
    end
  end

  it "selects the first valid numeric branch" do
    expect(native_schema(%w[int long]).encode(1)).to eq(avro_long(0) + reference_encode("int", 1))
    expect(native_schema(%w[int long]).encode(2**40)).to eq(avro_long(1) + reference_encode("long", 2**40))
    expect(native_schema(%w[float double]).encode(1.5)).to eq(avro_long(0) + reference_encode("float", 1.5))
  end

  %w[float double].each do |type|
    it "infers a sole #{type} union branch for Float, Integer and BigDecimal" do
      schema = native_schema(["null", type])
      [1.25, 42, BigDecimal("1.25")].each do |value|
        bytes = schema.encode(value)
        expect(bytes).to eq(avro_long(1) + reference_encode(type, value))
        expect(schema.decode(bytes)).to eq(value.to_f)
      end
    end
  end

  it "allows tags to override the first accepting numeric branch" do
    [1, BigDecimal("1.25")].each do |value|
      expect(native_schema(%w[float double]).encode(value)).to eq(avro_long(0) + reference_encode("float", value))
    end
    expect(native_schema(%w[long float]).encode(1)).to eq(avro_long(0) + reference_encode("long", 1))
    schema = native_schema(%w[float double])
    expect(schema.encode(described_class.new("double", BigDecimal("1.25"))))
      .to eq(avro_long(1) + reference_encode("double", BigDecimal("1.25")))
  end

  it "selects numeric branches explicitly by index or primitive name" do
    definition = %w[int long]
    schema = native_schema(definition)
    [[0, "int", 1], [1, "long", 2**40]].each do |index, name, value|
      bytes = schema.encode(described_class.new(index, value))
      expect(bytes).to eq(avro_long(index) + reference_encode(name, value))
      expect(schema.encode(described_class.new(name, value))).to eq(bytes)
      expect(reference_decode(definition, bytes)).to eq(value)
    end
  end

  it "selects string branches in schema order unless explicitly tagged" do
    definition = ["string", "bytes", { "type" => "enum", "name" => "Code", "symbols" => ["AB"] },
                  { "type" => "fixed", "name" => "Pair", "size" => 2 }]
    schema = native_schema(definition)
    expect(schema.encode("AB")).to eq(avro_long(0) + reference_encode("string", "AB"))
    %w[string bytes Code Pair].each_with_index do |branch, index|
      bytes = schema.encode(described_class.new(branch, "AB"))
      expect(bytes.getbyte(0)).to eq(index * 2)
      expect(reference_decode(definition, bytes)).to eq("AB")
      expect(schema.decode(bytes)).to eq("AB")
    end
  end

  it "selects named branches by fully qualified names" do
    definition = [record_schema("Item", [field("value", "int")], namespace: "first"),
                  record_schema("Item", [field("value", "string")], namespace: "second")]
    schema = native_schema(definition)
    first = schema.encode(described_class.new("first.Item", { value: 1 }))
    second = schema.encode(described_class.new("second.Item", { value: "two" }))
    expect(schema.encode({ value: 1 })).to eq(first)
    expect(schema.encode({ value: "two" })).to eq(second)
    expect(reference_decode(definition, first)).to eq("value" => 1)
    expect(reference_decode(definition, second)).to eq("value" => "two")
    expect(schema.decode(first)).to eq("value" => 1)
    expect(schema.decode(second)).to eq("value" => "two")
  end

  it "selects the first valid map or record branch" do
    definition = [{ "type" => "map", "values" => "int" }, record_schema("Record", [field("value", "int")])]
    schema = native_schema(definition)
    expect(schema.encode({ "value" => 1 })).to eq(avro_long(0) + reference_encode(definition.first, { "value" => 1 }))
    expect(schema.decode(schema.encode(described_class.new("Record", { value: 1 })))).to eq("value" => 1)
  end

  it "validates explicit selectors and selected branch values" do
    schema = native_schema(%w[int long])
    [-1, 2, "missing", "string"].each do |branch|
      expect { schema.encode(described_class.new(branch, 1)) }.to raise_error(Avrocadabra::EncodeError)
    end
    expect { schema.encode(described_class.new("int", 2**31)) }.to raise_error(Avrocadabra::EncodeError)
    expect { schema.encode(described_class.new("long", "1")) }.to raise_error(Avrocadabra::EncodeError)
  end

  it "rejects values that match no branch" do
    expect { native_schema(%w[null int]).encode("1") }.to raise_error(Avrocadabra::EncodeError)
  end
end
