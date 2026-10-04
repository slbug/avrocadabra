# frozen_string_literal: true

require "avrocadabra"
require "avro_turf/messaging"

module Avrocadabra
  module AvroTurf
    RactorSupport.prepare

    class << self
      def with_codecs(cache)
        previous = Thread.current[:avrocadabra_codecs]
        Thread.current[:avrocadabra_codecs] = cache
        yield
      ensure
        Thread.current[:avrocadabra_codecs] = previous
      end
    end

    ::AvroTurf::Messaging.prepend(Routing)
    ::Avro::IO::DatumWriter.prepend(DatumWriter)
    ::Avro::IO::DatumReader.prepend(DatumReader)
    ::Avro::SchemaValidator.singleton_class.prepend(Validation)
  end
end
