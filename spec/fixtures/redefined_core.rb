# frozen_string_literal: true

require "avrocadabra"

schema = Avrocadabra::Schema.new({ type: "record", name: "Probe", fields: [
                                   { name: "id", type: "long" },
                                   { name: "items", type: { type: "array", items: "long" } },
                                   { name: "index", type: { type: "map", values: "long" } },
                                   { name: "label", type: "string" }
                                 ] })
datum = { "id" => 1, "items" => [1], "index" => { "k" => 2 }, "label" => "x" }
Array.prepend(Module.new { def each = super { yield it * 10 } })
Hash.prepend(Module.new do
  def each = super { |key, value| yield key, value * 10 }
  def [](key) = key == "id" ? 7 : super
end)
String.prepend(Module.new { def encode(*) = "changed" })
puts JSON.generate(schema.decode(schema.encode(datum)))
