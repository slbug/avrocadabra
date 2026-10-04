# frozen_string_literal: true

RSpec.describe Avrocadabra::Schema do
  it "preserves Hash insertion order in bytes and decoded keys" do
    definition = { "type" => "map", "values" => { "type" => "map", "values" => "long" } }
    datum = { "third" => { "z" => 1, "a" => 2 }, "first" => {}, "second" => { "b" => 3 } }
    schema = native_schema(definition)
    expected = reference_encode(definition, datum)
    8.times do
      expect(schema.encode(datum)).to eq(expected)
      decoded = schema.decode(expected)
      expect(decoded.keys).to eq(datum.keys)
      expect(decoded.fetch("third").keys).to eq(%w[z a])
    end
  end

  it "retains the first position and last value of duplicate wire map keys" do
    definition = { "type" => "map", "values" => "long" }
    entries = [["b", 1], ["a", 2], ["b", 3]]
    payload = entries.map { |key, value| reference_encode("string", key) + avro_long(value) }.join
    bytes = "#{avro_long(3)}#{payload}#{avro_long(0)}"
    decoded = native_schema(definition).decode(bytes)
    expect(decoded.to_a).to eq(reference_decode(definition, bytes).to_a)
    expect(decoded.to_a).to eq([["b", 3], ["a", 2]])
  end
end
