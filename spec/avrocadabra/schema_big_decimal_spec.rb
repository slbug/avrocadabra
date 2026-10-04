# frozen_string_literal: true

RSpec.describe Avrocadabra::Schema do
  let(:definition) { { "type" => "bytes", "logicalType" => "big-decimal" } }
  let(:schema) { native_schema(definition) }

  vectors = JSON.parse(File.read(File.expand_path("../fixtures/big_decimal_vectors.json", __dir__)))
  vectors.fetch("vectors").each do |vector|
    it "cross-decodes the Java #{vector.fetch("name")} vector exactly" do
      expected = BigDecimal(vector.fetch("value"))
      java_bytes = [vector.fetch("wire_hex")].pack("H*")
      native_bytes = [vector.fetch("canonical_wire_hex")].pack("H*")
      expect(schema.decode(java_bytes)).to eq(expected)
      expect(schema.encode(expected)).to eq(native_bytes)
      expect(native_bytes.encoding).to eq(Encoding::BINARY)
      expect(schema.decode(native_bytes)).to be_a(BigDecimal)
      expect(reference_decode("bytes", native_bytes))
        .to eq(reference_decode("bytes", big_decimal_bytes(Integer(vector.fetch("canonical_coefficient")),
                                                           vector.fetch("canonical_scale"))))
    end
  end

  it "accepts integers without a machine-sized precision limit" do
    [-((10**100) - 1), -1, 0, 1, (10**100) - 1].each do |value|
      expected = big_decimal_bytes(value, 0)
      expect(schema.encode(value)).to eq(expected)
      expect(schema.decode(expected)).to eq(BigDecimal(value))
    end
  end

  it "normalizes insignificant zeros while preserving the exact value" do
    [BigDecimal("1.2300"), BigDecimal("1.23")].each do |value|
      expect(schema.encode(value)).to eq(big_decimal_bytes(123, 2))
    end
    [BigDecimal("-0"), BigDecimal("0e1000000"), 0].each do |value|
      expect(schema.encode(value)).to eq(big_decimal_bytes(0, 0))
    end
    expect(schema.decode(big_decimal_bytes(12_300, 4))).to eq(BigDecimal("1.23"))
  end

  it "preserves precision independently of the ambient BigDecimal limit" do
    value = BigDecimal("12345678901234567890.01234567890123456789")
    bytes = big_decimal_bytes(1_234_567_890_123_456_789_001_234_567_890_123_456_789, 20)
    BigDecimal.save_limit do
      BigDecimal.limit(3)
      expect(schema.encode(value)).to eq(bytes)
      expect(schema.decode(bytes)).to eq(value)
    end
  end

  it "does not expand extreme exponents or impose an arbitrary scale limit" do
    bounded = native_schema(definition, max_bytes: 32)
    [-(2**63) + 32, -1_000_000, 1_000_000, (2**63) - 1].each do |scale|
      value = BigDecimal("1e#{-scale}")
      bytes = big_decimal_bytes(1, scale)
      expect(value).to be_finite
      expect(value).not_to be_zero
      expect(bounded.decode(bytes)).to eq(value)
      expect(bounded.encode(value)).to eq(bytes)
    end
  end

  it "rejects an exponent Ruby cannot represent instead of returning Infinity" do
    expect { schema.decode(big_decimal_bytes(1, -(2**63))) }.to raise_error(Avrocadabra::DecodeError)
    expect(schema.decode(big_decimal_bytes(0, -(2**63)))).to eq(BigDecimal("0"))
  end

  ["NaN", "Infinity", "-Infinity"].each do |value|
    it "rejects nonfinite #{value} without rounding" do
      expect { schema.encode(BigDecimal(value)) }.to raise_error(Avrocadabra::EncodeError, /finite/)
    end
  end

  it "accepts Float decimals and rejects unrelated Ruby input values" do
    expect(schema.decode(schema.encode(1.25))).to eq(BigDecimal("1.25"))
    ["1.25", nil, false, Rational(1, 2)].each do |value|
      expect { schema.encode(value) }.to raise_error(Avrocadabra::EncodeError)
    end
  end

  it "applies max_bytes to the coefficient and framed output" do
    exact = big_decimal_bytes(1, 0)
    bounded = native_schema(definition, max_bytes: exact.bytesize)
    expect(bounded.encode(1)).to eq(exact)
    expect(bounded.decode(exact)).to eq(BigDecimal("1"))
    too_small = native_schema(definition, max_bytes: exact.bytesize - 1)
    expect { too_small.encode(1) }.to raise_error(Avrocadabra::EncodeError)
    expect { too_small.decode(exact) }.to raise_error(Avrocadabra::DecodeError)
    expect { bounded.encode(BigDecimal("9" * 1024)) }.to raise_error(Avrocadabra::EncodeError)
  end

  it "infers decimal branches and uses the first numeric branch for integers" do
    union = native_schema(["null", "long", definition])
    expect(union.decode(union.encode(BigDecimal("1.25")))).to eq(BigDecimal("1.25"))
    expect(union.decode(union.encode(nil))).to be_nil
    expect(union.encode(1)).to eq(avro_long(1) + avro_long(1))
    [2, "bytes"].each do |branch|
      expect(union.encode(Avrocadabra::Union.new(branch, 1))).to eq(avro_long(2) + big_decimal_bytes(1, 0))
    end
    expect(union.encode(Avrocadabra::Union.new("long", 1))).to eq(avro_long(1) + avro_long(1))
  end

  it "supports exact big-decimals nested in records, arrays and maps" do
    nested = record_schema("Readings", [field("single", definition),
                                        field("series", { "type" => "array", "items" => definition }),
                                        field("index", { "type" => "map", "values" => definition })])
    physical = record_schema("Readings", [field("single", "bytes"),
                                          field("series", { "type" => "array", "items" => "bytes" }),
                                          field("index", { "type" => "map", "values" => "bytes" })])
    value = BigDecimal("-1.23")
    payload = reference_decode("bytes", big_decimal_bytes(-123, 2))
    datum = { "single" => value, "series" => [value], "index" => { "sample" => value } }
    encoded = { "single" => payload, "series" => [payload], "index" => { "sample" => payload } }
    prepared = native_schema(nested)
    expect(prepared.decode(reference_encode(physical, encoded))).to eq(datum)
    expect(reference_decode(physical, prepared.encode(datum))).to eq(encoded)
  end

  it "resolves logical values to and from their physical bytes" do
    value = BigDecimal("-12.345")
    bytes = big_decimal_bytes(-12_345, 3)
    payload = reference_decode("bytes", bytes)
    physical = native_schema("bytes")
    expect(schema.decode(bytes, reader_schema: physical)).to eq(payload)
    expect(physical.decode(bytes, reader_schema: schema)).to eq(value)
    expect(schema.decode(bytes, reader_schema: native_schema(definition))).to eq(value)
  end

  it "resolves reordered reader union branches" do
    writer = native_schema(["null", definition])
    reader = native_schema(["long", definition, "null"])
    bytes = avro_long(1) + big_decimal_bytes(12_345, 3)
    expect(writer.decode(bytes, reader_schema: reader)).to eq(BigDecimal("12.345"))
  end

  it "selects an earlier compatible string reader branch before the logical bytes branch" do
    writer = native_schema(["null", definition])
    reader = native_schema(["string", definition, "null"])
    bytes = big_decimal_bytes(12_345, 3)
    expected = reference_decode("bytes", bytes)
    decoded = writer.decode(avro_long(1) + bytes, reader_schema: reader)
    expect(decoded).to eq(expected)
    expect(decoded.encoding).to eq(expected.encoding)
  end

  it "uses physical byte defaults when the reader adds a logical field" do
    payload = reference_decode("bytes", big_decimal_bytes(-129, 3)).bytes.pack("U*")
    writer = native_schema(record_schema("Defaulted", []))
    reader = native_schema(record_schema("Defaulted", [field("reading", definition, default: payload)]))
    expect(writer.decode("".b, reader_schema: reader)).to eq("reading" => BigDecimal("-0.129"))
    expect { reader.encode({}) }.to raise_error(Avrocadabra::EncodeError, /reading/)
  end

  it "rejects malformed inner bytes in reader defaults during preparation" do
    ["", "\x02\x01", "\x02\x01\x00\x00"].each do |payload|
      invalid = record_schema("InvalidDefault", [field("reading", definition, default: payload)])
      expect { native_schema(invalid) }.to raise_error(Avrocadabra::SchemaError)
    end
  end

  it "resolves compatible UTF-8 physical strings to and from logical bytes" do
    bytes = big_decimal_bytes(1, 0)
    payload = reference_decode("bytes", bytes).force_encoding(Encoding::UTF_8)
    physical = native_schema("string")
    expect(schema.decode(bytes, reader_schema: physical)).to eq(payload)
    expect(physical.decode(reference_encode("string", payload), reader_schema: schema)).to eq(BigDecimal("1"))
  end

  it "reports a nested field path for an invalid logical value" do
    record = native_schema(record_schema("Record", [field("reading", definition)]))
    expect { record.encode({ reading: BigDecimal("NaN") }) }
      .to raise_error(Avrocadabra::EncodeError, /reading/)
    expect { record.decode(reference_encode("bytes", "\x02\x01\x80".b)) }
      .to raise_error(Avrocadabra::DecodeError, /reading/)
  end

  {
    "empty payload" => "".b,
    "negative coefficient length" => "\x01".b,
    "empty coefficient" => "\x00\x00".b,
    "truncated coefficient" => "\x04\x01".b,
    "missing scale" => "\x02\x01".b,
    "truncated scale" => "\x02\x01\x80".b,
    "overflowing scale" => "\x02\x01".b + ("\xff".b * 9) + "\x02".b,
    "trailing inner bytes" => "\x02\x01\x00\x00".b
  }.each do |name, payload|
    it "rejects a malformed #{name}" do
      expect { schema.decode(reference_encode("bytes", payload)) }.to raise_error(Avrocadabra::DecodeError)
    end
  end

  it "rejects a huge inner length without allocating the declared coefficient" do
    payload = avro_long((2**63) - 1) + "\x00".b
    bounded = native_schema(definition, max_bytes: 64)
    expect { bounded.decode(reference_encode("bytes", payload)) }.to raise_error(Avrocadabra::DecodeError)
  end

  it "decodes one decimal when more bytes follow" do
    expect(schema.decode(big_decimal_bytes(123, 2) + "\x00".b)).to eq(BigDecimal("1.23"))
  end
end
