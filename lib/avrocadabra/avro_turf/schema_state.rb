# frozen_string_literal: true

module Avrocadabra
  module AvroTurf
    class SchemaState
      FIELD_ATTRIBUTES = %i[name type default default? aliases].freeze
      CHECK_SIZE = 512

      attr_reader :schemas

      def initialize(source)
        @schemas = Set.new.compare_by_identity
        @objects = []
        @attributes = []
        @values = []
        @containers = []
        visit(source, Set.new.compare_by_identity)
        @checks = @attributes.each_slice(CHECK_SIZE).with_index.map do |attributes, chunk|
          [reader_check(attributes, chunk * CHECK_SIZE), @values.slice(chunk * CHECK_SIZE, CHECK_SIZE).freeze]
        end
      end

      def current?
        @checks.all? { |check, values| NativeSchema.unchanged?([check.call(@objects), values]) } &&
          NativeSchema.unchanged?(@containers)
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

      def reader_check(attributes, offset)
        reads = attributes.each_with_index.map { |attribute, index| "o[#{offset + index}].#{attribute}" }
        source = "->(o) { [\n#{reads.join(",\n")}\n] }"
        instance_eval(source, __FILE__, __LINE__)
      end

      def observe(object, attribute, observed)
        value = object.public_send(attribute)
        @objects << object
        @attributes << attribute
        @values << value
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
