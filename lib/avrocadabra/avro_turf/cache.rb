# frozen_string_literal: true

module Avrocadabra
  module AvroTurf
    class Cache
      LIMIT = 128

      def initialize
        @entries = {}.compare_by_identity
        @mutex = Mutex.new
        @plans = NativeSchema::Plans.new
      end

      def fetch(schema)
        codec = @mutex.synchronize { @entries[schema] }
        return codec if codec&.current?

        @mutex.synchronize do
          codec = @entries[schema]
          return codec if codec&.current?

          codec = Codec.new(schema)
          @entries.shift if !@entries.key?(schema) && @entries.size >= LIMIT
          @entries[schema] = codec
        end
      end
    end
  end
end
