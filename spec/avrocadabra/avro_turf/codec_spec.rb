# frozen_string_literal: true

require "support/avro_turf_fixture"

RSpec.describe Avrocadabra::AvroTurf::Codec do
  let(:cache) { Avrocadabra::AvroTurf::Cache.new }

  def native_read(definition, stream, reader: definition)
    codec = Avro::IO::DatumReader.new(reference_schema(definition), reference_schema(reader))
    Avrocadabra::AvroTurf.with_codecs(cache) { codec.read(Avro::IO::BinaryDecoder.new(stream)) }
  end

  [["null", nil], ["boolean", false], ["long", 2**40], ["float", 1.25], ["double", 1.25],
   ["bytes", "\xff".b], %w[string 日本語], [%w[null string], "hello"],
   [{ "type" => "fixed", "name" => "Token", "size" => 3 }, "abc"],
   [{ "type" => "enum", "name" => "State", "symbols" => %w[A B] }, "B"],
   [{ "type" => "array", "items" => "long" }, [1, 2]],
   [{ "type" => "map", "values" => "long" }, { "a" => 1 }]].each do |definition, value|
    it "uses the shared decoder and stream positioning for #{definition.inspect}" do
      bytes = reference_encode(definition, value)
      stream = StringIO.new("#{bytes}tail".b)
      expect(native_read(definition, stream)).to eq(reference_decode(definition, bytes))
      expect(stream.pos).to eq(bytes.bytesize)
      expect(stream.read).to eq("tail")
      bytes.bytesize.times do |length|
        truncated = bytes.byteslice(0, length)
        expect { native_schema(definition).decode(truncated) }.to raise_error(Avrocadabra::DecodeError)
        expect { native_read(definition, StringIO.new(truncated)) }.to raise_error(EOFError)
      end
    end
  end

  it "uses enum defaults and preserves unknown symbols without a default like Ruby Avro" do
    writer = { "type" => "enum", "name" => "Choice", "symbols" => %w[A B] }
    reader = writer.merge("symbols" => ["A"], "default" => "A")
    bytes = reference_encode(writer, "B")
    expect(native_read(writer, StringIO.new(bytes), reader: reader)).to eq("A")
    expect(native_read(writer, StringIO.new(bytes), reader: reader.except("default")))
      .to eq(reference_decode(writer, bytes, reader: reader.except("default")))
  end

  it "uses the same promoted Ruby values as Ruby Avro" do
    [["int", "double", 42], ["long", "float", 42], ["bytes", "string", "α".b],
     %w[string bytes α]].each do |writer, reader, value|
      bytes = reference_encode(writer, value)
      actual = native_read(writer, StringIO.new(bytes), reader: reader)
      expected = reference_decode(writer, bytes, reader: reader)
      expect(actual).to eq(expected)
      expect(actual.class).to eq(expected.class)
      expect(actual.encoding).to eq(expected.encoding) if actual.is_a?(String)
    end
  end

  it "translates schema preparation and decoding failures" do
    invalid = reference_schema({ "type" => "fixed", "name" => "Large", "size" => (16 * 1024 * 1024) + 1 })
    expect { described_class.new(invalid) }.to raise_error(Avro::SchemaParseError)
    expect { reference_decode("long", "\x02".b, reader: "string") }
      .to raise_error(Avro::IO::SchemaMatchException) do |error|
        expect { native_read("long", StringIO.new("\x02".b), reader: "string") }
          .to raise_error(error.class, error.message)
      end
  end

  it "commits a stream position only after logical conversion succeeds" do
    schema = reference_schema("long")
    adapter = Object.new
    allow(adapter).to receive(:decode).and_raise(RangeError, "adapter failed")
    schema.instance_variable_set(:@type_adapter, adapter)
    codec = described_class.new(schema)
    stream = StringIO.new("prefix\x02suffix".b)
    stream.pos = 6
    expect { codec.read(Avro::IO::BinaryDecoder.new(stream), codec) }.to raise_error(RangeError, "adapter failed")
    expect(stream.pos).to eq(6)
  end

  it "preserves the cursor when a repeated union adapter call fails after compaction" do
    calls = []
    adapter = Object.new
    allow(adapter).to receive(:decode) do |value|
      calls << value
      GC.compact
      raise RangeError, "second conversion" if calls.size == 2

      value + 1
    end
    reader = reference_schema("long")
    reader.instance_variable_set(:@type_adapter, adapter)
    codec = described_class.new(reference_schema(%w[null long]))
    stream = StringIO.new("prefix#{reference_encode(%w[null long], 41)}tail".b)
    stream.pos = 6
    expect { codec.read(Avro::IO::BinaryDecoder.new(stream), described_class.new(reader)) }
      .to raise_error(RangeError, "second conversion")
    expect(calls).to eq([41, 42])
    expect(stream.pos).to eq(6)
  end

  it "uses stock codecs for custom encoders and non-StringIO readers" do
    schema = reference_schema("long")
    encoder = instance_spy(Avro::IO::BinaryEncoder)
    reader, writer = IO.pipe
    writer.write(reference_encode("long", 42))
    writer.close
    Avrocadabra::AvroTurf.with_codecs(cache) do
      Avro::IO::DatumWriter.new(schema).write(42, encoder)
      expect(Avro::IO::DatumReader.new(schema).read(Avro::IO::BinaryDecoder.new(reader))).to eq(42)
    end
    expect(encoder).to have_received(:write_long).with(42)
  ensure
    reader&.close
    writer&.close unless writer&.closed?
  end

  it "encodes natively without Ruby Avro validation" do
    definition = ["null", record_schema("Item", [field("id", "long")])]
    codec = described_class.new(reference_schema(definition))
    validations = 0
    bytes = TracePoint.new(:call) { validations += 1 }.enable(target: Avro::Schema.method(:validate)) do
      codec.encode({ "id" => 1 })
    end
    expect(bytes).to eq(reference_encode(definition, { "id" => 1 }))
    expect(validations).to eq(0)
  end

  context "with a prepared native codec" do
    let(:native) { instance_spy(Avrocadabra::NativeSchema) }
    let(:prepared) { instance_double(Avrocadabra::Schema, native: native) }
    let(:codec) { described_class.new(reference_schema("long")) }

    before { allow(Avrocadabra::Schema).to receive(:new).and_return(prepared) }

    it "encodes through Ruby Avro's writer instead of the standalone codec" do
      expect(codec.encode(42)).to eq(reference_encode("long", 42))
      expect(native).not_to have_received(:encode)
    end

    it "retains the GVL and unwraps unions for Messaging decoding" do
      stream = StringIO.new("\x02".b)
      codec.read(Avro::IO::BinaryDecoder.new(stream), codec)
      expect(native).to have_received(:decode).with(stream, native, false, false, kind_of(Array))
    end
  end
end
