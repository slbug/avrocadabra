# frozen_string_literal: true

require "avrocadabra"
require "avro_turf/messaging"

module Avrocadabra
  module AvroTurf
    RactorSupport.prepare

    class << self
      def with_codecs(cache)
        previous = NativeSchema.codecs
        NativeSchema.codecs = cache
        yield
      ensure
        NativeSchema.codecs = previous
      end
    end

    Validation = NativeSchema::Budget

    ::AvroTurf::Messaging.prepend(Routing)
    ::Avro::IO::DatumWriter.prepend(NativeSchema::Writer)
    ::Avro::IO::DatumReader.prepend(DatumReader)
    ::Avro::SchemaValidator.singleton_class.prepend(NativeSchema::Budget)
    # Ahead of a module's own singleton methods, hooks survive a `method_added` that skips super.
    Module.prepend(NativeSchema::Hooks)
    Stock::HOOKED.each { it.singleton_class.prepend(NativeSchema::Hooks) }
  end
end
