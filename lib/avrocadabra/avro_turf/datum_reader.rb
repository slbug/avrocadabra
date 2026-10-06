# frozen_string_literal: true

module Avrocadabra
  module AvroTurf
    module DatumReader
      def read(decoder)
        cache = NativeSchema.codecs
        return super unless cache && decoder.instance_of?(::Avro::IO::BinaryDecoder) && decoder.reader.is_a?(StringIO)

        self.readers_schema = writers_schema unless readers_schema
        writer = cache.fetch(writers_schema)
        reader = readers_schema.equal?(writers_schema) ? writer : cache.fetch(readers_schema)
        writer.read(decoder, reader)
      end
    end
  end
end
