# frozen_string_literal: true

module Avrocadabra
  module AvroTurf
    module Routing
      def encode(...)
        AvroTurf.with_codecs(@avrocadabra_codecs) { super }
      end

      def decode_message(...)
        AvroTurf.with_codecs(@avrocadabra_codecs) { super }
      end
    end
  end
end
