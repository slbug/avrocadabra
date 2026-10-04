# frozen_string_literal: true

module Avrocadabra
  module AvroTurf
    class SchemaState
      FIELD_ATTRIBUTES = %i[name type default default? aliases].freeze

      attr_reader :schemas

      def initialize(source)
        @schemas = Set.new.compare_by_identity
        @attributes = []
        @containers = []
        visit(source, Set.new.compare_by_identity)
      end

      def current?
        NativeSchema.unchanged?(@attributes, @containers)
      end

      private

      def visit(schema, observed)
        return if @schemas.include?(schema)

        @schemas.add(schema)
        %i[type_sym logical_type type_adapter name namespace aliases precision scale size symbols
           default].each do |attribute|
          observe(schema, attribute, observed) if schema.respond_to?(attribute)
        end
        case schema.type_sym
        when :record, :error
          observe(schema, :fields, observed).each do |field|
            FIELD_ATTRIBUTES.each { observe(field, it, observed) }
            visit(field.type, observed)
          end
        when :array then visit(observe(schema, :items, observed), observed)
        when :map then visit(observe(schema, :values, observed), observed)
        when :union then observe(schema, :schemas, observed).each { visit(it, observed) }
        end
      end

      def observe(object, attribute, observed)
        value = object.public_send(attribute)
        @attributes.push(object, attribute, value)
        watch(value, observed)
        value
      end

      def watch(value, observed)
        return if observed.include?(value)

        observed.add(value)
        case value
        when String
          @containers.push(value, value.dup.freeze) unless value.frozen?
        when Array
          @containers.push(value, value.dup.freeze) unless value.frozen?
          value.each { watch(it, observed) }
        when Hash
          @containers.push(value, value.to_a.flatten(1).freeze) unless value.frozen?
          value.each do |key, child|
            watch(key, observed)
            watch(child, observed)
          end
        end
      end
    end
  end
end
