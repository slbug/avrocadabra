# frozen_string_literal: true

conversions = {
  "time" => [Time, :to_time, { type: "long", logicalType: "timestamp-millis" }, Time.at(1)],
  "float" => [Float, :to_i, { type: "int", logicalType: "date" }, 10.9],
  "decimal" => [BigDecimal, :to_f, "double", BigDecimal("1.5")]
}
owner, conversion, type, value = conversions.fetch(ARGV.fetch(0))
schema = Avrocadabra::Schema.new({ type: "record", name: "Converted", fields: [
                                   { name: "at", type: type },
                                   { name: "second", type: "long" }
                                 ] })
owner.prepend(Module.new do
  define_method(conversion) do
    Hash.prepend(Module.new { def [](key) = key == "second" ? 22 : super })
    super()
  end
end)
puts schema.decode(schema.encode({ "at" => value, "second" => 2 })).fetch("second")
