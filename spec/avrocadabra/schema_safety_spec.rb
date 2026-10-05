# frozen_string_literal: true

RSpec.describe Avrocadabra::Schema do
  [false, true].each do |release_gvl|
    it "applies the reader byte limit to the encoded input with release_gvl=#{release_gvl}" do
      writer = native_schema("long")
      reader = native_schema("long", max_bytes: 1)
      bytes = writer.encode(128)
      expect { reader.decode(bytes, release_gvl: release_gvl) }.to raise_error(Avrocadabra::DecodeError)
      expect { writer.decode(bytes, reader_schema: reader, release_gvl: release_gvl) }
        .to raise_error(Avrocadabra::DecodeError, /max_bytes/)
      expect(writer.decode(writer.encode(63), reader_schema: reader, release_gvl: release_gvl)).to eq(63)
    end

    it "counts discarded writer items against reader limits with release_gvl=#{release_gvl}" do
      writer = native_schema(record_schema("Record", [field("discarded", { "type" => "array", "items" => "null" })]))
      reader = native_schema(record_schema("Record", []), max_items: 1)
      bytes = writer.encode({ discarded: Array.new(100) })
      expect { writer.decode(bytes, reader_schema: reader, release_gvl: release_gvl) }
        .to raise_error(Avrocadabra::DecodeError, /item count/)
    end

    it "bounds discarded writer nesting using reader limits with release_gvl=#{release_gvl}" do
      nested = { "type" => "array", "items" => { "type" => "array", "items" => "null" } }
      writer = native_schema(record_schema("Record", [field("discarded", nested)]))
      reader = native_schema(record_schema("Record", []), max_depth: 1)
      bytes = writer.encode({ discarded: [[nil]] })
      expect { writer.decode(bytes, reader_schema: reader, release_gvl: release_gvl) }
        .to raise_error(Avrocadabra::DecodeError, /depth/)
      boundary = native_schema(record_schema("Record", []), max_depth: 3, max_items: 4)
      expect(writer.decode(bytes, reader_schema: boundary, release_gvl: release_gvl)).to eq({})
    end
  end

  it "consumes exactly one datum, including a zero-byte null datum" do
    ["null", "int", "string",
     { "type" => "array", "items" => "int" }].zip([nil, 1, "text", [1, 2]]).each do |definition, value|
      schema = native_schema(definition)
      bytes = schema.encode(value)
      expect(schema.decode(bytes + "\x00".b)).to eq(value)
      expect(schema.decode(bytes)).to eq(value)
    end
  end

  it "rejects every truncation of representative datums" do
    examples = [["long", (2**63) - 1], ["float", 1.25], ["double", 1.25], %w[string 日本語],
                ["bytes", "\x00\xff".b], [{ "type" => "fixed", "name" => "Digest", "size" => 4 }, "1234"],
                [{ "type" => "array", "items" => "long" }, [1, 2]],
                [{ "type" => "map", "values" => "long" }, { "one" => 1 }],
                [record_schema("Record", [field("id", "long"), field("text", "string")]),
                 { "id" => 1, "text" => "ok" }]]
    examples.each do |definition, value|
      schema = native_schema(definition)
      bytes = reference_encode(definition, value)
      bytes.bytesize.times do |length|
        expect { schema.decode(bytes.byteslice(0, length)) }.to raise_error(Avrocadabra::DecodeError)
      end
    end
  end

  it "rejects invalid booleans, negative lengths, invalid union and enum indices" do
    cases = [["boolean", "\x02".b], ["bytes", avro_long(-1)], ["string", avro_long(-1)],
             [%w[null string], avro_long(-1)], [%w[null string], avro_long(2)],
             [{ "type" => "enum", "name" => "One", "symbols" => ["A"] }, avro_long(-1)],
             [{ "type" => "enum", "name" => "One", "symbols" => ["A"] }, avro_long(1)]]
    cases.each do |definition, bytes|
      expect { native_schema(definition).decode(bytes) }.to raise_error(Avrocadabra::DecodeError)
    end
  end

  it "rejects overflowing, overlong and unterminated variable-length integers" do
    schema = native_schema("long")
    ["\x80".b * 10, ("\xff".b * 9) + "\x02".b, ("\x80".b * 11) + "\x00".b].each do |bytes|
      expect { schema.decode(bytes) }.to raise_error(Avrocadabra::DecodeError)
    end
    expect { native_schema("int").decode(avro_long(2**31)) }.to raise_error(Avrocadabra::DecodeError)
    expect { native_schema("int").decode(avro_long(-(2**31) - 1)) }.to raise_error(Avrocadabra::DecodeError)
  end

  it "rejects enormous declared lengths without allocating their contents" do
    ["string", "bytes", { "type" => "array", "items" => "null" },
     { "type" => "map", "values" => "null" }].each do |definition|
      schema = native_schema(definition, max_bytes: 1024, max_items: 16)
      expect { schema.decode(avro_long((2**63) - 1)) }.to raise_error(Avrocadabra::DecodeError)
      expect { schema.decode(avro_long(-(2**63))) }.to raise_error(Avrocadabra::DecodeError)
    end
  end

  it "decodes valid negative-count array and map blocks" do
    array = native_schema({ "type" => "array", "items" => "int" })
    array_bytes = avro_long(-2) + avro_long(2) + avro_long(1) + avro_long(2) + avro_long(0)
    expect(array.decode(array_bytes)).to eq([1, 2])
    expect(reference_decode({ "type" => "array", "items" => "int" }, array_bytes)).to eq([1, 2])
    map = native_schema({ "type" => "map", "values" => "int" })
    map_bytes = avro_long(-1) + avro_long(3) + reference_encode("string", "a") + avro_long(1) + avro_long(0)
    expect(map.decode(map_bytes)).to eq("a" => 1)
  end

  it "rejects negative or incorrect block byte counts" do
    schema = native_schema({ "type" => "array", "items" => "int" })
    [-1, 0, 1, 3].each do |size|
      bytes = avro_long(-2) + avro_long(size) + avro_long(1) + avro_long(2) + avro_long(0)
      expect { schema.decode(bytes) }.to raise_error(Avrocadabra::DecodeError)
    end
  end

  it "bounds total container items across successive blocks, including null items" do
    definition = { "type" => "array", "items" => "null" }
    schema = native_schema(definition, max_items: 6)
    expect { schema.encode(Array.new(6)) }.to raise_error(Avrocadabra::EncodeError)
    expect { schema.decode(avro_long(3) + avro_long(3) + avro_long(0)) }.to raise_error(Avrocadabra::DecodeError)
    expect(schema.decode(avro_long(5) + avro_long(0))).to eq(Array.new(5))
  end

  it "stops encoding once written bytes exceed max_bytes" do
    schema = native_schema({ "type" => "array", "items" => "long" }, max_bytes: 16)
    yielded = 0
    values = Class.new(Array) do
      define_method(:each) do |&block|
        1_000_000.times do
          yielded += 1
          block.call(2**40)
        end
      end
    end.new([1])
    expect { schema.encode(values) }.to raise_error(Avrocadabra::EncodeError, /max_bytes/)
    expect(yielded).to be < 10
  end

  it "enforces the datum byte limit on encode and decode" do
    schema = native_schema("bytes", max_bytes: 32)
    expect { schema.encode("x" * 33) }.to raise_error(Avrocadabra::EncodeError)
    expect { schema.decode(reference_encode("bytes", "x" * 33)) }.to raise_error(Avrocadabra::DecodeError)
    expect(schema.decode(schema.encode("x" * 16))).to eq(("x" * 16).b)
  end

  it "bounds recursive data and rejects cyclic Ruby objects safely" do
    definition = record_schema("Node", [field("next", %w[null Node])])
    datum = { "next" => nil }
    20.times { datum = { "next" => datum } }
    bounded = native_schema(definition, max_depth: 8)
    expect { bounded.encode(datum) }.to raise_error(Avrocadabra::EncodeError)
    bytes = native_schema(definition).encode(datum)
    expect { bounded.decode(bytes) }.to raise_error(Avrocadabra::DecodeError)
    cyclic = {}
    cyclic["next"] = cyclic
    expect { bounded.encode(cyclic) }.to raise_error(Avrocadabra::EncodeError)
  end

  it "preserves a frozen encoded input and its original Ruby encoding" do
    schema = native_schema("string")
    bytes = schema.encode("safe").force_encoding(Encoding::UTF_8).freeze
    snapshot = bytes.dup
    expect(schema.decode(bytes)).to eq("safe")
    expect(bytes).to eq(snapshot)
    expect(bytes.encoding).to eq(Encoding::UTF_8)
  end

  it "bounds cumulative union search across rejected recursive branches" do
    choices = %w[Left Right].map { record_schema(it, [field("node", "Node")]) }
    definition = record_schema("Node", [field("value", "int"), field("next", ["null", *choices])])
    lookups = 0
    leaf = Hash.new do
      lookups += 1
      "invalid"
    end
    value = 12.times.reduce(leaf) { |child, _index| { "value" => 1, "next" => { "node" => child } } }
    schema = native_schema(definition, max_items: 100)
    expect { schema.encode(value) }.to raise_error(Avrocadabra::EncodeError, /union search/)
    expect(lookups).to be < 100
  end

  it "raises DecodeError for non-string input" do
    [nil, 1, [], Object.new].each do |value|
      expect { native_schema("int").decode(value) }.to raise_error(Avrocadabra::DecodeError)
    end
  end
end
