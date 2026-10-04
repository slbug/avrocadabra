# frozen_string_literal: true

require "objspace"

RSpec.describe Avrocadabra::Schema do
  it "reports owned schema data beyond the native wrapper" do
    small = native_schema("null")
    large = native_schema({ "type" => "enum", "name" => "Symbols", "symbols" => Array.new(2000) { "S#{it}" } })
    expect(ObjectSpace.memsize_of(large.send(:native)) - ObjectSpace.memsize_of(small.send(:native))).to be > 48_000
  end

  it "accounts for each retained resolution plan once and stops at the cache bound" do
    writer = native_schema(record_schema("Record", []))
    native = writer.send(:native)
    previous = ObjectSpace.memsize_of(native)
    12.times do |index|
      reader = native_schema(record_schema("Record", [field("value", "string", default: "x" * 8192)]))
      expect(writer.decode("".b, reader_schema: reader).fetch("value").bytesize).to eq(8192)
      current = ObjectSpace.memsize_of(native)
      expect(current - previous).to(index < 8 ? be >= 8192 : eq(0))
      writer.decode("".b, reader_schema: reader)
      expect(ObjectSpace.memsize_of(native)).to eq(current)
      previous = current
    end
  end

  it "triggers collection from native schema allocation pressure" do
    output, errors, status = ruby_fixture("native_memory.rb")
    expect(status.success?).to be(true), errors
    result = JSON.parse(output)
    expect(result.fetch("collections")).to be_positive
    expect(result.fetch("native_bytes")).to be > 480_000
  end
end
