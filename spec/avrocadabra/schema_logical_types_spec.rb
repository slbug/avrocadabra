# frozen_string_literal: true

RSpec.describe Avrocadabra::Schema do
  describe "decimal" do
    let(:definition) { { "type" => "bytes", "logicalType" => "decimal", "precision" => 4, "scale" => 2 } }

    ["-99.99", "-1.28", "-0.01", "0", "0.01", "1.27", "99.99"].each do |text|
      it "cross-decodes exact decimal #{text}" do
        expect_interoperable(definition, BigDecimal(text))
      end
    end

    it "accepts integers and preserves precision beyond machine integers" do
      schema = native_schema(definition)
      expect(schema.decode(schema.encode(12))).to eq(BigDecimal("12"))
      large = { "type" => "bytes", "logicalType" => "decimal", "precision" => 60, "scale" => 30 }
      value = BigDecimal("123456789012345678901234567890.123456789012345678901234567890")
      expect_interoperable(large, value)
    end

    it "preserves exact decimals independently of the ambient BigDecimal precision limit" do
      definition = { "type" => "bytes", "logicalType" => "decimal", "precision" => 40, "scale" => 10 }
      value = BigDecimal("12345678901234567890.0123456789")
      expected_bytes = reference_encode(definition, value)
      schema = native_schema(definition)
      BigDecimal.save_limit do
        BigDecimal.limit(3)
        expect(schema.encode(value)).to eq(expected_bytes)
        expect(schema.decode(expected_bytes)).to eq(value)
      end
    end

    it "accepts exact trailing zeros and values at two's-complement byte boundaries" do
      definition = { "type" => "bytes", "logicalType" => "decimal", "precision" => 6, "scale" => 2 }
      ["-327.69", "-327.68", "-1.29", "-0.00", "1.2300", "1.28", "327.67", "327.68"].each do |text|
        expect_interoperable(definition, BigDecimal(text))
      end
    end

    it "accepts leading fractional zeros while rejecting integral overflow at full scale" do
      definition = { "type" => "bytes", "logicalType" => "decimal", "precision" => 4, "scale" => 4 }
      ["0.0001", "-0.0001", "0.9999"].each { expect_interoperable(definition, BigDecimal(it)) }
      expect { native_schema(definition).encode(1) }.to raise_error(Avrocadabra::EncodeError)
    end

    it "encodes zero without a fractional part under a zero-scale schema" do
      definition = { "type" => "bytes", "logicalType" => "decimal", "precision" => 1, "scale" => 0 }
      schema = native_schema(definition)
      bytes = reference_encode("bytes", "\x00".b)
      [BigDecimal("0"), BigDecimal("-0"), 0].each do |value|
        expect(schema.encode(value)).to eq(bytes)
        expect(reference_decode(definition, schema.encode(value))).to eq(BigDecimal("0"))
      end
      expect(schema.decode(bytes)).to eq(BigDecimal("0"))
    end

    it "explains an excess decimal scale in its encoding error" do
      expect { native_schema(definition).encode(BigDecimal("1.001")) }
        .to raise_error(Avrocadabra::EncodeError, /excess scale/)
    end

    ["100.00", "-100.00", "0.001", "NaN", "Infinity", "-Infinity"].each do |text|
      it "rejects decimal #{text} without rounding or overflow" do
        expect { native_schema(definition).encode(BigDecimal(text)) }.to raise_error(Avrocadabra::EncodeError)
      end
    end

    it "accepts decimal Floats through the same conversion as Ruby Avro" do
      expect_interoperable(definition, 1.25, expected: BigDecimal("1.25"))
    end

    it "rejects String and nil decimal inputs" do
      ["1.25", nil].each do |value|
        expect { native_schema(definition).encode(value) }.to raise_error(Avrocadabra::EncodeError)
      end
    end

    it "rejects encoded decimals exceeding the declared precision" do
      bytes = reference_encode("bytes", [10_000].pack("s>"))
      expect { native_schema(definition).decode(bytes) }.to raise_error(Avrocadabra::DecodeError)
    end

    it "encodes fixed decimals as signed, big-endian integers with sign extension" do
      fixed = { "type" => "fixed", "name" => "DecimalValue", "size" => 2,
                "logicalType" => "decimal", "precision" => 4, "scale" => 2 }
      underlying = { "type" => "fixed", "name" => "DecimalValue", "size" => 2 }
      schema = native_schema(fixed)
      [-9999, -128, -1, 0, 1, 127, 9999].each do |unscaled|
        value = BigDecimal(unscaled) / 100
        bytes = [unscaled].pack("s>")
        expect(schema.encode(value)).to eq(bytes)
        expect(schema.decode(reference_encode(underlying, bytes))).to eq(value)
        expect(reference_decode(underlying, schema.encode(value))).to eq(bytes)
      end
      expect { schema.decode([10_000].pack("s>")) }.to raise_error(Avrocadabra::DecodeError)
    end

    it "requires matching decimal precision and scale during schema resolution" do
      writer = native_schema(definition)
      [{ "precision" => 5 }, { "scale" => 1 }].each do |change|
        reader = native_schema(definition.merge(change))
        expect { writer.decode(writer.encode(BigDecimal("1.23")), reader_schema: reader) }
          .to raise_error(Avrocadabra::DecodeError)
      end
    end

    [{}, { "precision" => 0 }, { "precision" => -1 }, { "precision" => 2.5 },
     { "precision" => "2" }, { "precision" => nil }, { "precision" => true },
     { "precision" => 2, "scale" => -1 }, { "precision" => 2, "scale" => 0.5 },
     { "precision" => 2, "scale" => "0" }, { "precision" => 2, "scale" => nil },
     { "precision" => 2, "scale" => 3 }].each do |metadata|
      it "cross-decodes bytes when decimal metadata is invalid: #{metadata.inspect}" do
        schema = native_schema(definition.slice("type", "logicalType").merge(metadata))
        value = "\x00\xff".b
        expect(reference_decode("bytes", schema.encode(value))).to eq(value)
        expect(schema.decode(reference_encode("bytes", value))).to eq(value)
        expect(schema.decode(schema.encode(value)).encoding).to eq(Encoding::BINARY)
        expect { schema.encode(BigDecimal("1")) }.to raise_error(Avrocadabra::EncodeError)
      end
    end

    it "uses fixed storage when decimal precision exceeds its capacity" do
      [[0, 1], [1, 3], [1, (2**64) - 1]].each do |size, precision|
        physical = { "type" => "fixed", "name" => "Raw", "size" => size }
        schema = native_schema(physical.merge("logicalType" => "decimal", "precision" => precision))
        value = "\xff".b * size
        expect(reference_decode(physical, schema.encode(value))).to eq(value)
        expect(schema.decode(reference_encode(physical, value))).to eq(value)
        expect { schema.encode("#{value}x") }.to raise_error(Avrocadabra::EncodeError)
      end
    end

    it "preserves named dependencies and physical reader defaults after decimal fallback" do
      fixed = { "type" => "fixed", "name" => "Raw", "namespace" => "wire", "size" => 1 }
      reference = fixed.merge("logicalType" => "decimal", "precision" => 3)
      writer = native_schema(record_schema("Record", []))
      reader = native_schema(record_schema("Record", [field("value", "wire.Raw", default: "x")]),
                             references: [reference])
      value = { "value" => "x".b }
      physical = record_schema("Record", [field("value", fixed)])
      expect(writer.decode("".b, reader_schema: reader)).to eq(value)
      expect(reference_decode(physical, reader.encode(value))).to eq(value)
      expect(reader.decode(reference_encode(physical, value))).to eq(value)
    end

    it "resolves invalid decimal annotations as physical types in both directions" do
      invalid = native_schema({ "type" => "bytes", "logicalType" => "decimal", "precision" => 0 })
      physical = native_schema("bytes")
      value = "\xff".b
      expect(invalid.decode(invalid.encode(value), reader_schema: physical)).to eq(value)
      expect(physical.decode(physical.encode(value), reader_schema: invalid)).to eq(value)
    end

    it "ignores decimal annotations on incompatible physical types" do
      [%w[string 日本語], ["long", 42], [record_schema("Record", []), {}]].each do |physical, value|
        definition = physical.is_a?(Hash) ? physical : { "type" => physical }
        schema = native_schema(definition.merge("logicalType" => "decimal", "precision" => 2))
        expect(reference_decode(physical, schema.encode(value))).to eq(value)
        expect(schema.decode(reference_encode(physical, value))).to eq(value)
      end
    end

    it "rejects valid decimal precision above the resource limit" do
      definition = { "type" => "bytes", "logicalType" => "decimal", "precision" => 4097 }
      expect { native_schema(definition) }.to raise_error(Avrocadabra::SchemaError, /4096/)
      definition = { "type" => "fixed", "name" => "Wide", "size" => 2048,
                     "logicalType" => "decimal", "precision" => 4097 }
      expect { native_schema(definition) }.to raise_error(Avrocadabra::SchemaError, /4096/)
    end
  end

  describe "date" do
    let(:definition) { { "type" => "int", "logicalType" => "date" } }

    [Date.new(1969, 12, 31), Date.new(1970, 1, 1), Date.new(2000, 2, 29), Date.new(2026, 10, 3)].each do |value|
      it "cross-decodes date #{value}" do
        expect_interoperable(definition, value)
      end
    end

    it "uses the proleptic Gregorian calendar before the historical calendar change" do
      value = Date.new(1000, 1, 1, Date::GREGORIAN)
      schema = native_schema(definition)
      days = (value - Date.new(1970, 1, 1)).to_i
      expect(schema.encode(value)).to eq(reference_encode("int", days))
      expect(schema.decode(avro_long(days)).iso8601).to eq("1000-01-01")
    end

    it "accepts DateTime and numeric days" do
      schema = native_schema(definition)
      expect(schema.decode(schema.encode(DateTime.new(1970, 1, 1)))).to eq(Date.new(1970, 1, 1))
      expect(schema.decode(schema.encode(1.9))).to eq(Date.new(1970, 1, 2))
    end

    it "rejects unrelated values and dates outside the Avro int range" do
      invalid = ["1970-01-01", Time.at(0),
                 Date.jd(Date.new(1970, 1, 1).jd + (2**31))]
      invalid.each do |value|
        expect { native_schema(definition).encode(value) }.to raise_error(Avrocadabra::EncodeError)
      end
    end
  end

  { "time-millis" => ["int", 86_400_000], "time-micros" => ["long", 86_400_000_000] }.each do |logical, (type, day)|
    it "cross-decodes #{logical} as integer ticks since midnight" do
      definition = { "type" => type, "logicalType" => logical }
      [0, 1, day - 1].each { expect_interoperable(definition, it) }
      schema = native_schema(definition)
      [-1, day, 1.5].each do |value|
        expect { schema.encode(value) }.to raise_error(Avrocadabra::EncodeError)
      end
      [-1, day].each do |value|
        expect { schema.decode(reference_encode(type, value)) }.to raise_error(Avrocadabra::DecodeError)
      end
    end
  end

  { "timestamp-millis" => 1_000, "timestamp-micros" => 1_000_000,
    "timestamp-nanos" => 1_000_000_000 }.each do |logical, scale|
    it "cross-decodes #{logical} as exact UTC Time, including negative epochs" do
      definition = { "type" => "long", "logicalType" => logical }
      ticks_to_check = [-1_234_567, -1, 0, 1, 1_234_567]
      ticks_to_check.each do |ticks|
        value = Time.at(Rational(ticks, scale)).utc
        expect_interoperable(definition, value)
        expect(native_schema(definition).decode(avro_long(ticks))).to be_utc
      end
    end

    it "uses numeric ticks and truncates #{logical} fractions to the declared unit" do
      schema = native_schema({ "type" => "long", "logicalType" => logical })
      expect(schema.encode(Time.at(Rational(1, scale * 10)))).to eq(avro_long(0))
      expect(schema.encode(Time.at(Rational(-1, scale * 10)))).to eq(avro_long(-1))
      expect(schema.encode(1.9)).to eq(avro_long(1))
      expect { schema.encode("1970-01-01") }.to raise_error(Avrocadabra::EncodeError)
    end

    it "preserves #{logical} signed 64-bit boundaries and normalizes offsets to UTC" do
      schema = native_schema({ "type" => "long", "logicalType" => logical })
      [-(2**63), (2**63) - 1].each do |ticks|
        time = Time.at(Rational(ticks, scale)).utc
        expect(schema.encode(time)).to eq(avro_long(ticks))
        expect(schema.decode(avro_long(ticks))).to eq(time)
      end
      local_time = Time.at(Rational(1_234_567, scale)).getlocal("+09:30")
      expect(schema.decode(schema.encode(local_time))).to eq(local_time.getutc)
      expect(schema.decode(schema.encode(local_time))).to be_utc
      [-((2**63) + 1), 2**63].each do |ticks|
        expect { schema.encode(Time.at(Rational(ticks, scale))) }.to raise_error(Avrocadabra::EncodeError)
      end
    end
  end

  %w[local-timestamp-millis local-timestamp-micros local-timestamp-nanos].each do |logical|
    it "cross-decodes #{logical} as integer ticks without a timezone conversion" do
      definition = { "type" => "long", "logicalType" => logical }
      [-(2**63), -1, 0, 1_234_567, (2**63) - 1].each { expect_interoperable(definition, it) }
      expect { native_schema(definition).encode(Time.at(0)) }.to raise_error(Avrocadabra::EncodeError)
    end
  end

  it "normalizes Ruby logical-conversion failures to EncodeError" do
    value = Time.at(0)
    def value.to_r = raise(TypeError, "invalid timestamp conversion")

    schema = native_schema({ "type" => "long", "logicalType" => "timestamp-millis" })
    expect { schema.encode(value) }.to raise_error(Avrocadabra::EncodeError, /invalid timestamp conversion/)
  end

  describe "uuid" do
    let(:uuid) { "123e4567-e89b-12d3-a456-426614174000" }

    it "cross-decodes UUID strings" do
      definition = { "type" => "string", "logicalType" => "uuid" }
      expect_interoperable(definition, uuid)
      expect { native_schema(definition).encode("not-a-uuid") }.to raise_error(Avrocadabra::EncodeError)
      expect { native_schema(definition).decode(reference_encode("string", "not-a-uuid")) }
        .to raise_error(Avrocadabra::DecodeError)
    end

    it "maps fixed UUIDs to the same canonical string and exactly 16 bytes" do
      definition = { "type" => "fixed", "name" => "Identifier", "size" => 16, "logicalType" => "uuid" }
      underlying = { "type" => "fixed", "name" => "Identifier", "size" => 16 }
      bytes = [uuid.delete("-")].pack("H*")
      schema = native_schema(definition)
      expect(schema.encode(uuid)).to eq(bytes)
      expect(schema.decode(reference_encode(underlying, bytes))).to eq(uuid)
      expect(reference_decode(underlying, schema.encode(uuid))).to eq(bytes)
    end
  end

  describe "duration" do
    let(:definition) { { "type" => "fixed", "name" => "Elapsed", "size" => 12, "logicalType" => "duration" } }

    it "maps three unsigned 32-bit components without conflating months and days" do
      value = Avrocadabra::Duration.new(months: 13, days: 35, milliseconds: (2**32) - 1)
      schema = native_schema(definition)
      bytes = [13, 35, (2**32) - 1].pack("V3")
      underlying = { "type" => "fixed", "name" => "Elapsed", "size" => 12 }
      expect(schema.encode(value)).to eq(bytes)
      expect(schema.decode(reference_encode(underlying, bytes))).to eq(value)
      expect(reference_decode(underlying, schema.encode(value))).to eq(bytes)
      decoded = schema.decode(bytes)
      expect(decoded).to be_frozen
      expect([decoded.months, decoded.days, decoded.milliseconds]).to eq([13, 35, (2**32) - 1])
    end

    it "rejects out of range duration components" do
      [-1, 2**32, 1.5].each do |value|
        expect do
          native_schema(definition).encode(Avrocadabra::Duration.new(months: value, days: 0, milliseconds: 0))
        end.to raise_error(Avrocadabra::EncodeError)
      end
    end
  end

  it "ignores unknown logical annotations as required by the Avro specification" do
    expect_interoperable({ "type" => "string", "logicalType" => "future-custom-type" }, "value")
  end
end
