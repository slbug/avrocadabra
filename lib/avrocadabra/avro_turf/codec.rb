# frozen_string_literal: true

module Avrocadabra
  module AvroTurf
    class Codec
      attr_reader :source

      def initialize(source)
        @source = source
        @state = SchemaState.new(source)
        @mapping = Mapping.new(source, @state.schemas)
        @native = Schema.new(JSON.generate(document(source, Set.new))).__send__(:native)
      rescue SchemaError => e
        raise ::Avro::SchemaParseError, e.message
      end

      def encode(datum)
        stream = StringIO.new(+"".b)
        AvroTurf.with_codecs(@cache ||= Cache.new) do
          ::Avro::IO::DatumWriter.new(@source).write(datum, ::Avro::IO::BinaryEncoder.new(stream))
        end
        stream.string
      end

      def current?
        @state.current?
      end

      def read(decoder, reader)
        @native.decode(decoder.reader, reader.native, false, false, reader.mapping.native)
      rescue ResolutionError => e
        if e.message.include?("reader field is absent from writer schema and has no default")
          raise ::Avro::AvroError, e.message
        end

        raise ::Avro::IO::SchemaMatchException.new(source, reader.source), cause: e
      rescue DecodeError => e
        raise EOFError, e.message if e.message.include?("truncated")

        raise ::Avro::AvroError, e.message
      end

      private

      def document(schema, names)
        result = case schema.type_sym
                 when :record, :error
                   named = ::Avro::Schema::NamedSchema.instance_method(:to_avro).bind_call(schema, names)
                   return named unless named.is_a?(Hash)

                   named.merge("fields" => schema.fields.map { field_document(it, names) })
                 when :array then { "type" => "array", "items" => document(schema.items, names) }
                 when :map then { "type" => "map", "values" => document(schema.values, names) }
                 when :union then return schema.schemas.map { document(it, names) }
                 else schema.to_avro(names)
                 end
        result.delete("logicalType") if result.is_a?(Hash)
        result
      end

      def field_document(field, names)
        entry = { "name" => field.name, "type" => document(field.type, names) }
        entry["default"] = field.default if field.default?
        entry["aliases"] = field.aliases if field.aliases
        entry
      end

      protected

      attr_reader :mapping, :native
    end
  end
end
