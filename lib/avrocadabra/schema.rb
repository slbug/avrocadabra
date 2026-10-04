# frozen_string_literal: true

module Avrocadabra
  class Schema
    MAX_DEPTH = 64
    MAX_ITEMS = 1_000_000

    def initialize(schema, references: [], max_depth: MAX_DEPTH, max_bytes: 16 * 1024 * 1024, max_items: MAX_ITEMS)
      raise SchemaError, "references must be an Array of schemas" unless references.is_a?(Array)

      @native = NativeSchema.new(
        schema_json(schema), references.map { schema_json(it) }, max_depth, max_bytes, max_items
      )
      Ractor.make_shareable(self)
    rescue JSON::JSONError, TypeError, ArgumentError, RangeError => e
      raise SchemaError, e.message
    end

    def encode(data, release_gvl: false)
      @native.encode(data, release_gvl, nil)
    rescue TypeError, ArgumentError, RangeError, EncodingError => e
      raise EncodeError, e.message
    end

    def decode(bytes, reader_schema: nil, release_gvl: false, tagged_unions: false)
      unless reader_schema.nil? || reader_schema.is_a?(Schema)
        raise DecodeError, "reader_schema must be an Avrocadabra::Schema"
      end

      @native.decode(bytes, reader_schema&.native, release_gvl, tagged_unions, nil)
    rescue TypeError, ArgumentError, RangeError, EncodingError => e
      raise DecodeError, e.message
    end

    protected

    attr_reader :native

    private

    def schema_json(schema)
      return schema if schema.is_a?(String)
      raise SchemaError, "schema must be JSON, a Hash, or an Array" unless schema.is_a?(Hash) || schema.is_a?(Array)

      JSON.generate(schema, max_nesting: 128)
    end
  end
end
