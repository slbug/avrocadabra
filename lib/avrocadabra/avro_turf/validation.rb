# frozen_string_literal: true

module Avrocadabra
  module AvroTurf
    module Validation
      def validate!(schema, value, options = ::Avro::SchemaValidator::DEFAULT_VALIDATION_OPTIONS)
        if Thread.current[:avrocadabra_codecs] && !options.key?(:avrocadabra_budget)
          options = options.merge(avrocadabra_budget: [Schema::MAX_ITEMS, Schema::MAX_DEPTH + 1])
        end
        super
      rescue EncodeError => e
        raise ::Avro::IO::AvroTypeError.new(schema, value), cause: e
      end

      private

      def validate_recursive(schema, value, path, result, options)
        budget = options[:avrocadabra_budget]
        return super unless budget

        depth = schema.type_sym == :union ? 0 : 1
        budget[0] -= 1
        budget[1] -= depth
        raise EncodeError, "#{path}: union search exceeds maximum item count" if budget[0].negative?
        raise EncodeError, "#{path}: union search exceeds maximum depth" if budget[1].negative?

        super
      ensure
        budget[1] += depth if budget
      end
    end
  end
end
