# frozen_string_literal: true

module Avrocadabra
  module AvroTurf
    module DatumWriter
      def write(datum, encoder)
        cache = Thread.current[:avrocadabra_codecs]
        return super unless cache && encoder.instance_of?(::Avro::IO::BinaryEncoder)

        encoder.write(cache.fetch(writers_schema).encode(datum))
      end
    end
  end
end
