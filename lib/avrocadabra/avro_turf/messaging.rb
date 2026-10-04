# frozen_string_literal: true

module Avrocadabra
  module AvroTurf
    class Messaging < ::AvroTurf::Messaging
      def initialize(...)
        @avrocadabra_codecs = Cache.new
        super
      end
    end
  end
end
