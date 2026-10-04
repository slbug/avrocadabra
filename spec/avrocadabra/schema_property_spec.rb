# frozen_string_literal: true

RSpec.describe Avrocadabra::Schema do
  let(:random) { Random.new(Integer(ENV.fetch("AVRO_FUZZ_SEED", "194857"))) }
  let(:iterations) { Integer(ENV.fetch("AVRO_FUZZ_CASES", "150")) }

  it "cross-decodes generated boundary-rich records against Ruby avro" do
    definition = record_schema("Generated", [field("id", "long"), field("enabled", "boolean"),
                                             field("label", %w[null string]), field("bytes", "bytes"),
                                             field("values", { "type" => "array", "items" => "int" }),
                                             field("counts", { "type" => "map", "values" => "long" })])
    schema = native_schema(definition)
    iterations.times do
      id = random.rand(-(2**63)..((2**63) - 1))
      alphabet = ["a", "ż", "日", "🌍", "\u0000"]
      label = Array.new(random.rand(0..12)) { alphabet.sample(random: random) }.join
      label = nil if random.rand(3).zero?
      datum = { "id" => id, "enabled" => random.rand(2).zero?, "label" => label,
                "bytes" => random.bytes(random.rand(0..20)),
                "values" => Array.new(random.rand(0..12)) { random.rand(-(2**31)..((2**31) - 1)) },
                "counts" => { "first" => random.rand(-100..100), "second" => random.rand(-100..100) } }
      expect(reference_decode(definition, schema.encode(datum))).to eq(datum)
      expect(schema.decode(reference_encode(definition, datum))).to eq(datum)
    end
  end

  it "preserves randomly generated arbitrary precision decimals exactly" do
    definition = { "type" => "bytes", "logicalType" => "decimal", "precision" => 40, "scale" => 12 }
    schema = native_schema(definition)
    iterations.times do
      value = BigDecimal(random.rand(-((10**40) - 1)..((10**40) - 1))) / (10**12)
      expect(reference_decode(definition, schema.encode(value))).to eq(value)
      expect(schema.decode(reference_encode(definition, value))).to eq(value)
    end
  end

  it "returns values or DecodeError for arbitrary bounded binary input" do
    definitions = ["null", "boolean", "int", "long", "float", "double", "bytes", "string", %w[null long string],
                   { "type" => "array", "items" => "null" }, { "type" => "array", "items" => "long" },
                   { "type" => "map", "values" => "long" }, { "type" => "bytes", "logicalType" => "big-decimal" },
                   { "type" => "enum", "name" => "Enum", "symbols" => %w[A B] },
                   { "type" => "fixed", "name" => "Fixed", "size" => 4 },
                   record_schema("Recursive", [field("value", "int"), field("next", %w[null Recursive])])]
    schemas = definitions.map { native_schema(it, max_depth: 12, max_bytes: 64, max_items: 32) }
    expect do
      iterations.times do
        bytes = random.bytes(random.rand(0..32))
        schemas.each do |schema|
          schema.decode(bytes, release_gvl: random.rand(2).zero?)
        rescue Avrocadabra::DecodeError
          next
        end
      end
    end.not_to raise_error
  end

  it "cross-decodes random big-decimal coefficients and signed scales with Ruby Avro framing" do
    schema = native_schema({ "type" => "bytes", "logicalType" => "big-decimal" })
    iterations.times do
      coefficient = random.rand(-((10**120) - 1)..((10**120) - 1))
      scale = random.rand(-1_000_000..1_000_000)
      value = BigDecimal("#{coefficient}e#{-scale}")
      expect(schema.decode(big_decimal_bytes(coefficient, scale))).to eq(value)
      inner = reference_decode("bytes", schema.encode(value))
      decoder = Avro::IO::BinaryDecoder.new(StringIO.new(inner))
      bytes = decoder.read_bytes
      decoded_coefficient = bytes.unpack1("H*").to_i(16)
      decoded_coefficient -= 1 << (bytes.bytesize * 8) if bytes.getbyte(0) >= 128
      expect(BigDecimal("#{decoded_coefficient}e#{-decoder.read_long}")).to eq(value)
    end
  end

  it "rejects invalid Ruby values without invoking coercion methods" do
    hostile = Object.new
    def hostile.to_str = raise("unexpected coercion")
    def hostile.to_int = raise("unexpected coercion")
    def hostile.to_hash = raise("unexpected coercion")
    def hostile.to_ary = raise("unexpected coercion")

    ["int", "string", "bytes", { "type" => "array", "items" => "int" },
     { "type" => "map", "values" => "int" }, record_schema("Record", [field("id", "int")])].each do |definition|
      expect { native_schema(definition).encode(hostile) }.to raise_error(Avrocadabra::EncodeError)
    end
  end
end
