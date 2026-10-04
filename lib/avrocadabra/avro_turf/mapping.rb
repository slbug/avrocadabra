# frozen_string_literal: true

module Avrocadabra
  module AvroTurf
    class Mapping
      attr_reader :native

      def initialize(source, schemas)
        nodes = {}.compare_by_identity
        schemas.each do |schema|
          adapter = schema.type_adapter
          nodes[schema] = [schema, adapter == ::Avro::LogicalTypes::Identity ? nil : adapter]
        end
        nodes.each do |schema, node|
          node.push(children(schema).map { nodes.fetch(it) }.freeze).freeze
        end
        @native = [self, nodes.fetch(source)].freeze
        @reader = ::Avro::IO::DatumReader.new
      end

      def union_index(schema, value, budget)
        schema.schemas.index { ::Avro::Schema.validate(it, value, avrocadabra_budget: budget) } ||
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
