# frozen_string_literal: true

module Avrocadabra
  module AvroTurf
    class Codec
      attr_reader :source

      def initialize(source)
        @source = source
        @state = SchemaState.new(source)
        @mapping = Mapping.new(source, @state.schemas)
        document = source.to_avro
        prepare_document(source, document)
        @native = Schema.new(JSON.generate(document)).__send__(:native)
      rescue SchemaError => e
        raise ::Avro::SchemaParseError, e.message
      end

      def encode(datum)
        @native.encode(datum, false, @mapping.native)
      rescue EncodeError => e
        raise ::Avro::IO::AvroTypeError.new(@source, datum), cause: e
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

      def prepare_document(source, document)
        document.delete("logicalType") if document.is_a?(Hash)
        case source.type_sym
        when :record, :error
          return if document.is_a?(String)

          source.fields.zip(document.fetch("fields")) do |field, entry|
            entry["aliases"] = field.aliases if field.aliases
            prepare_document(field.type, entry.fetch("type"))
          end
        when :array
          prepare_document(source.items, document.fetch("items"))
        when :map
          prepare_document(source.values, document.fetch("values"))
        when :union
          source.schemas.zip(document) { |branch, entry| prepare_document(branch, entry) }
        end
      end

      protected

      attr_reader :mapping, :native
    end
  end
end
