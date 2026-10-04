# frozen_string_literal: true

require "objspace"

definition = JSON.generate(type: "enum", name: "Symbols", symbols: Array.new(20_000) { "S#{it}" })
GC.start
before = GC.count
120.times { Avrocadabra::Schema.new(definition) }
collections = GC.count - before
sample = Avrocadabra::Schema.new(definition)
puts JSON.generate(collections: collections, native_bytes: ObjectSpace.memsize_of(sample.send(:native)))
