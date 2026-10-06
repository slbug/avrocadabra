# frozen_string_literal: true

module Avrocadabra
  module AvroTurf
    module Stock
      IO = ::Avro::IO
      VALIDATOR = ::Avro::SchemaValidator
      LOGICAL = ::Avro::LogicalTypes
      AVRO_SOURCE = LOGICAL.const_source_location(:BytesDecimal).first
      IO_SOURCE = IO.const_source_location(:DatumWriter).first
      SCHEMA_SOURCE = ::Avro.const_source_location(:Schema).first
      VALIDATOR_SOURCE = ::Avro.const_source_location(:SchemaValidator).first
      UTIL_SOURCE = $LOADED_FEATURES.find { |feature| feature.end_with?("/bigdecimal/util.rb") }
      SCHEMAS = [::Avro::Schema::PrimitiveSchema, ::Avro::Schema::BytesSchema, ::Avro::Schema::RecordSchema,
                 ::Avro::Schema::ArraySchema, ::Avro::Schema::MapSchema, ::Avro::Schema::UnionSchema,
                 ::Avro::Schema::EnumSchema, ::Avro::Schema::FixedSchema, ::Avro::Schema::Field].freeze
      READERS = { ::Avro::Schema::RecordSchema => %i[fields], ::Avro::Schema::ArraySchema => %i[items],
                  ::Avro::Schema::MapSchema => %i[values], ::Avro::Schema::UnionSchema => %i[schemas],
                  ::Avro::Schema::EnumSchema => %i[symbols], ::Avro::Schema::FixedSchema => %i[size] }.freeze
      LOGICAL_MODULES = %i[Identity IntDate TimestampMillis TimestampMicros TimestampNanos].map do |name|
        LOGICAL.const_get(name)
      end.freeze

      capture = lambda do |receiver, name, source, owner = receiver|
        method = receiver.instance_method(name)
        method if method.owner == owner && method.original_name == name && method.source_location&.first == source
      rescue NameError
        nil
      end
      entries = lambda do |receiver, names, source, owner: receiver, public: true, group: :core|
        names.map { [receiver, it, capture.call(receiver, it, source, owner), public, group] }
      end
      resolve = lambda do |cref, name|
        scope = cref.find { it.const_defined?(name, false) }
        [cref, name, (scope ? scope.const_get(name, false) : cref.first.const_get(name)).object_id]
      end

      readers = SCHEMAS.flat_map do |klass|
        names = klass == ::Avro::Schema::Field ? %i[type name] : %i[type_sym logical_type type_adapter type]
        (names + READERS.fetch(klass, [])).map do |name|
          [klass, name, capture.call(klass, name, SCHEMA_SOURCE, klass.instance_method(name).owner), true, :core]
        end
      end
      validator = %i[validate_simple resolve_datum validate_type type_mismatch_error actual_value_message
                     ruby_to_avro_type ruby_integer_to_avro_type validate_union first_compatible_type
                     validate_array validate_map deeper_path_for_hash fixed_string_message enum_message]
      METHODS = Ractor.make_shareable(
        [*entries.call(IO::DatumWriter, %i[write_data write_fixed write_enum write_array write_map write_union
                                           write_record], IO_SOURCE, public: false),
         *entries.call(IO::DatumWriter, %i[writers_schema], IO_SOURCE),
         *entries.call(IO::BinaryEncoder, %i[write_null write_boolean write_int write_long write_float write_double
                                             write_bytes write_string write writer], IO_SOURCE),
         *entries.call(::Avro::Schema.singleton_class, %i[validate], SCHEMA_SOURCE),
         *entries.call(VALIDATOR.singleton_class, validator, VALIDATOR_SOURCE, public: false),
         *entries.call(VALIDATOR::Result, %i[<< add_error failure? to_s errors], VALIDATOR_SOURCE),
         *entries.call(VALIDATOR::ValidationError, %i[initialize], VALIDATOR_SOURCE, public: false),
         *readers,
         *entries.call(LOGICAL::Identity.singleton_class, %i[encode], AVRO_SOURCE),
         *LOGICAL_MODULES.drop(1).flat_map do |logical|
           entries.call(logical.singleton_class, %i[encode], AVRO_SOURCE, group: :logical)
         end,
         *entries.call(LOGICAL::BytesDecimal, %i[encode precision scale], AVRO_SOURCE, group: :decimal),
         *entries.call(LOGICAL::BytesDecimal, %i[unscaled_value to_byte_array], AVRO_SOURCE, public: false,
                                                                                             group: :decimal),
         *entries.call(LOGICAL::BytesDecimal, %i[loop], "<internal:kernel>", owner: Kernel, public: false,
                                                                             group: :decimal),
         *entries.call(::StringIO, %i[write closed_write? string external_encoding], nil),
         *[::Float, ::Integer, ::BigDecimal].flat_map { entries.call(it, %i[to_d], UTIL_SOURCE, group: :decimal) },
         *[::Float, ::Integer].flat_map do |klass|
           entries.call(klass, %i[BigDecimal], nil, owner: Kernel, public: false, group: :decimal)
         end,
         *entries.call(::BigDecimal, %i[split * to_i finite?], nil, group: :decimal),
         *entries.call(::Integer, %i[[]], nil, group: :decimal),
         *entries.call(::BigDecimal.singleton_class, %i[limit], nil, group: :decimal),
         *entries.call(::BigDecimal, %i[to_f inspect == eql? hash], nil, group: :decimal_value),
         *entries.call(::Time, %i[to_time], nil, group: :time)]
      )
      SUPERS = Ractor.make_shareable(%i[validate! validate_recursive].map do |name|
        budget = VALIDATOR.singleton_class.instance_method(name)
        original = budget.super_method
        stock = budget.owner == NativeSchema::Budget && original&.source_location&.first == VALIDATOR_SOURCE
        [VALIDATOR.singleton_class, name, stock && original]
      end)
      scopes = {
        [IO::DatumWriter, IO, ::Avro] => %i[VALIDATION_OPTIONS Schema AvroTypeError Hash Array],
        [VALIDATOR, ::Avro] => %i[INT_RANGE LONG_RANGE BOOLEAN_VALUES DEFAULT_VALIDATION_OPTIONS ValidationError
                                  Result RECURSIVE_SIMPLE_VALIDATION_OPTIONS TypeMismatchError Hash Array String
                                  Integer Float BigDecimal],
        [::Avro::Schema, ::Avro] => %i[SchemaValidator DEFAULT_VALIDATE_OPTIONS VALID_TYPES_SYM],
        [LOGICAL, ::Avro] => %i[Identity],
        [LOGICAL::BytesDecimal, LOGICAL, ::Avro] => %i[Numeric PACK_UNSIGNED_CHARS]
      }
      CONSTANTS = Ractor.make_shareable(scopes.flat_map { |cref, names| names.map { resolve.call(cref, it) } })
      HOOKED = [*SCHEMAS, IO::DatumWriter, IO::BinaryEncoder, IO::AvroTypeError, VALIDATOR, VALIDATOR::Result,
                VALIDATOR::ValidationError, *LOGICAL_MODULES, LOGICAL::BytesDecimal, ::StringIO, ::BigDecimal,
                ::Time, ::Float, ::Integer, ::String, ::Symbol, ::Hash, ::Array, ::Range, ::Set, ::NilClass,
                ::TrueClass, ::FalseClass, ::UnboundMethod, ::RubyVM]
               .flat_map { [*it.ancestors, *it.singleton_class.ancestors] }
               .reject { it.singleton_class? || it.frozen? }.uniq.freeze
    end
  end
end
