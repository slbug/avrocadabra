# frozen_string_literal: true

require "avrocadabra"

record = { type: "record", name: "Probe", fields: [{ name: "id", type: "long" }] }
cases = [
  [Array, :each, :private, { type: "array", items: "long" }, [1]],
  [Array, :size, :protected, { type: "array", items: "long" }, [1]],
  [Hash, :each, :private, { type: "map", values: "long" }, { "k" => 1 }],
  [Hash, :key?, :private, record, { "id" => 1 }],
  [String, :encode, :private, "string", "x"]
]
results = cases.map do |owner, name, visibility, definition, datum|
  schema = Avrocadabra::Schema.new(JSON.generate(definition))
  owner.send(visibility, name)
  begin
    schema.encode(datum).unpack1("H*")
  rescue NoMethodError => e
    e.message
  ensure
    owner.send(:public, name)
  end
end
puts JSON.generate(results)
