# frozen_string_literal: true

module Avrocadabra
  module AvroTurf
    class Mapping
      LOGICAL = ::Avro::LogicalTypes
      MODULES = [LOGICAL::IntDate, LOGICAL::TimestampMillis, LOGICAL::TimestampMicros, LOGICAL::TimestampNanos].freeze
      AVRO_SOURCE = LOGICAL.const_source_location(:BytesDecimal).first
      UTIL_SOURCE = $LOADED_FEATURES.find { |feature| feature.end_with?("/bigdecimal/util.rb") }
      CONVERSIONS = Ractor.make_shareable({ Time => %i[to_time] })
      DECIMAL_METHODS = Ractor.make_shareable(
        [*%i[encode unscaled_value to_byte_array precision scale].map { [LOGICAL::BytesDecimal, it, AVRO_SOURCE] },
         *[Float, Integer, BigDecimal].map { [it, :to_d, UTIL_SOURCE] },
         [Kernel, :BigDecimal, nil], *%i[split * to_i].map { [BigDecimal, it, nil] }]
      )

      attr_reader :native

      def initialize(source, schemas)
        nodes = {}.compare_by_identity
        schemas.each do |schema|
          adapter = schema.type_adapter
          nodes[schema] = [schema, adapter == LOGICAL::Identity ? nil : adapter]
        end
        nodes.each do |schema, node|
          node.push(builtin_class(node[1]), field_names(schema), children(schema).map { nodes.fetch(it) }.freeze).freeze
        end
        @native = [self, nodes.fetch(source)].freeze
        @reader = ::Avro::IO::DatumReader.new
      end

      def stock_modules?(value_class)
        return false unless value_class == Integer || value_class == Float || CONVERSIONS.key?(value_class)

        MODULES.all? { stock_source?(it.singleton_class, :encode, AVRO_SOURCE) } &&
          CONVERSIONS.fetch(value_class, []).all? { stock_source?(value_class, it, nil) }
      end

      def native_decimal?
        DECIMAL_METHODS.all? { |owner, name, source| stock_source?(owner, name, source) }
      end

      def union_index(schema, value, budget)
        schema.schemas.index { valid_branch?(it, value, budget) } ||
          raise(encoding_error(schema, value))
      end

      def default_value(schema, name)
        field = schema.fields_hash.fetch(name)
        @reader.read_default_value(field.type, field.default)
      end

      def encoding_error(schema, value)
        ::Avro::IO::AvroTypeError.new(schema, value)
      end

      private

      def stock_source?(owner, name, source)
        owner.instance_method(name).source_location&.first == source
      end

      def builtin_class(adapter)
        return adapter.singleton_class if MODULES.include?(adapter)

        LOGICAL::BytesDecimal if Kernel.instance_method(:class).bind_call(adapter).equal?(LOGICAL::BytesDecimal)
      end

      def field_names(schema)
        schema.fields.map(&:name).freeze if %i[record error].include?(schema.type_sym)
      end

      def valid_branch?(branch, value, budget)
        if branch.type_sym == :null && !value.nil? && budget[0].positive?
          budget[0] -= 1
          return false
        end

        ::Avro::Schema.validate(branch, value, avrocadabra_budget: budget)
      end

      def children(schema)
        case schema.type_sym
        when :record, :error then schema.fields.map(&:type)
        when :array then [schema.items]
        when :map then [schema.values]
        when :union then schema.schemas
        else []
        end
      end
    end
  end
end
