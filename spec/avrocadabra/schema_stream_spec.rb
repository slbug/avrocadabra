# frozen_string_literal: true

RSpec.describe Avrocadabra::Schema do
  [false, true].each do |release_gvl|
    it "preserves position after Ruby conversion fails with release_gvl=#{release_gvl}" do
      schema = native_schema({ "type" => "bytes", "logicalType" => "big-decimal" })
      io = StringIO.new("prefix#{big_decimal_bytes(1, -(2**63))}".b)
      io.pos = 6
      expect { schema.decode(io, release_gvl: release_gvl) }.to raise_error(Avrocadabra::DecodeError)
      expect(io.pos).to eq(6)
    end

    it "decodes consecutive datums from the current position with release_gvl=#{release_gvl}" do
      schema = native_schema("string")
      first = schema.encode("first")
      second = schema.encode("second")
      io = StringIO.new("prefix#{first}#{second}tail".b)
      io.pos = 6
      expect(schema.decode(io, release_gvl: release_gvl)).to eq("first")
      expect(io.pos).to eq(6 + first.bytesize)
      expect(schema.decode(io, release_gvl: release_gvl)).to eq("second")
      expect(io.read).to eq("tail")
    end

    it "bounds only the consumed datum before copying with release_gvl=#{release_gvl}" do
      writer = native_schema("long")
      reader = native_schema("long", max_bytes: 1)
      bytes = "\x02#{"x" * 100_000}".b
      [bytes, StringIO.new(bytes)].each do |input|
        expect(writer.decode(input, reader_schema: reader, release_gvl: release_gvl)).to eq(1)
      end
      io = StringIO.new(writer.encode(128))
      expect { writer.decode(io, reader_schema: reader, release_gvl: release_gvl) }
        .to raise_error(Avrocadabra::DecodeError, /max_bytes/)
      expect(io.pos).to eq(0)
    end
  end

  it "does not advance streams when a datum is truncated or invalid" do
    schema = native_schema("string")
    ["\x06ab".b, "\x02\xff".b].each do |bytes|
      io = StringIO.new("prefix#{bytes}".b)
      io.pos = 6
      expect { schema.decode(io) }.to raise_error(Avrocadabra::DecodeError)
      expect(io.pos).to eq(6)
    end
  end

  it "preserves zero-byte datum positions and rejects closed readers" do
    schema = native_schema("null")
    io = StringIO.new("tail")
    expect(schema.decode(io)).to be_nil
    expect(io.pos).to eq(0)
    io.close_read
    expect { schema.decode(io) }.to raise_error(IOError)
  end
end
