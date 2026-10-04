# frozen_string_literal: true

RSpec.describe Avrocadabra::Schema do
  { "null" => [nil], "boolean" => [false, true],
    "int" => [-(2**31), -64, -1, 0, 1, 64, (2**31) - 1],
    "long" => [-(2**63), -(2**31) - 1, -1, 0, (2**31), (2**63) - 1],
    "float" => [-1.25, 0.0, 1.25], "double" => [-1.25, 0.0, 1.25, Float::MAX],
    "string" => ["", "hello", "Zażółć gęślą jaźń", "日本語 👩🏽‍💻", "a\u0000b"],
    "bytes" => ["".b, "\x00\xff\x80".b] }.each do |type, values|
    values.each do |value|
      it "cross-decodes #{type} #{value.inspect}" do
        expect_interoperable(type, value)
      end
    end
  end

  { "null" => [false, 0, ""], "boolean" => [nil, 0, "false"],
    "int" => [-(2**31) - 1, 2**31, 1.5, "1", nil],
    "long" => [-(2**63) - 1, 2**63, 1.5, "1"],
    "float" => [nil, "1"], "double" => [nil, "1"],
    "string" => [nil, :symbol, 1], "bytes" => [nil, [1, 2], :symbol] }.each do |type, values|
    values.each do |value|
      it "rejects invalid #{type} #{value.inspect}" do
        expect { native_schema(type).encode(value) }.to raise_error(Avrocadabra::EncodeError)
      end
    end
  end

  %w[float double].each do |type|
    it "accepts integer values for a direct #{type} schema" do
      expect(native_schema(type).decode(native_schema(type).encode(42))).to eq(42.0)
    end

    it "converts BigDecimal to IEEE #{type} with the same rounding as Ruby avro" do
      schema = native_schema(type)
      [BigDecimal("1.2345678901234567890123456789"), BigDecimal("-0"), BigDecimal("1e-20")].each do |value|
        reference = reference_encode(type, value)
        expect(schema.encode(value)).to eq(reference)
        expect(schema.decode(reference)).to eq(reference_decode(type, reference))
      end
    end

    it "narrows overflowing numeric values to IEEE #{type} like Ruby Avro" do
      [10**400, -(10**400), BigDecimal("1e400"), BigDecimal("-1e400")].each do |value|
        expect(native_schema(type).encode(value)).to eq(reference_encode(type, value))
      end
    end

    it "preserves #{type} negative zero and non-finite IEEE values" do
      schema = native_schema(type)
      expect(1.0 / schema.decode(schema.encode(-0.0))).to eq(-Float::INFINITY)
      [Float::INFINITY, -Float::INFINITY].each do |value|
        expect(schema.decode(schema.encode(value))).to eq(value)
        expect(reference_decode(type, schema.encode(value))).to eq(value)
      end
      expect(schema.decode(schema.encode(Float::NAN))).to be_nan
      expect(schema.decode(reference_encode(type, Float::NAN))).to be_nan
    end
  end

  it "narrows finite double precision overflow to Avro float infinity" do
    expect_interoperable("float", Float::MAX, expected: Float::INFINITY)
  end

  it "uses UTF-8 for strings and binary encoding for bytes" do
    string = native_schema("string")
    bytes = native_schema("bytes")
    expect(string.decode(string.encode("日本語")).encoding).to eq(Encoding::UTF_8)
    expect(bytes.decode(bytes.encode("日本語")).encoding).to eq(Encoding::BINARY)
  end

  it "rejects invalid UTF-8 on encode and decode" do
    schema = native_schema("string")
    expect { schema.encode("\xff".b) }.to raise_error(Avrocadabra::EncodeError)
    expect { schema.decode("\x02\xff".b) }.to raise_error(Avrocadabra::DecodeError)
  end

  it "transcodes text to UTF-8 without changing the input" do
    schema = native_schema("string")
    text = "caf\xe9".b.force_encoding(Encoding::ISO_8859_1)
    expect(schema.decode(schema.encode(text))).to eq("café")
    expect(text.encoding).to eq(Encoding::ISO_8859_1)
  end
end
