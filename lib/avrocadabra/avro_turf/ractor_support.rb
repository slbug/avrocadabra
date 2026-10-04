# frozen_string_literal: true

module Avrocadabra
  module AvroTurf
    module RactorSupport
      JSON_ERROR = ::MultiJson::ParseError
      EXCON_DEFAULTS = Ractor.make_shareable(::Excon::DEFAULTS, copy: true)
      LOGICAL_TYPES = Ractor.make_shareable(::Avro::LogicalTypes::TYPES, copy: true)
      REQUEST_KEYS = Ractor.make_shareable(::Excon::VALID_REQUEST_KEYS, copy: true)
      CONNECTION_KEYS = Ractor.make_shareable(::Excon::VALID_CONNECTION_KEYS, copy: true)
      STATUS_ERRORS = Ractor.make_shareable(::Excon::Error.status_errors, copy: true)

      module LogicalTypes
        def type_adapter(type, logical_type, schema = nil)
          return super if Ractor.main?
          return unless logical_type

          adapter = LOGICAL_TYPES.fetch(type, {}).fetch(logical_type, ::Avro::LogicalTypes::Identity)
          adapter.is_a?(Class) ? adapter.new(schema) : adapter
        end
      end

      module StatusErrors
        def status_errors
          return super if Ractor.main?

          Ractor.store_if_absent(:avrocadabra_status_errors) { STATUS_ERRORS.transform_values(&:dup) }
        end
      end

      module ConnectionKeys
        def valid_request_keys(middlewares)
          return super if Ractor.main?

          valid_middleware_keys(middlewares) + REQUEST_KEYS
        end

        private

        def validate_params(validation, params, middlewares)
          return super if Ractor.main?

          keys = case validation
                 when :connection then valid_middleware_keys(middlewares) + CONNECTION_KEYS
                 when :request then valid_request_keys(middlewares)
                 else raise ArgumentError, "Invalid validation type '#{validation}'"
                 end
          if validation == :connection && params[:omit_default_port] != true
            ::Excon.display_warning("The `omit_default_port` connection option is deprecated, " \
                                    "please use `include_default_port` instead.")
          end
          invalid = params.keys - keys
          return if invalid.empty?

          ::Excon.display_warning("Invalid Excon #{validation} keys: #{invalid.map(&:inspect).join(", ")}")
          return unless validation == :request

          warn_deprecated_keys(invalid)
        end

        def warn_deprecated_keys(invalid)
          messages = (invalid & ::Excon::DEPRECATED_VALID_REQUEST_KEYS.keys).map do |key|
            "#{key}: #{::Excon::DEPRECATED_VALID_REQUEST_KEYS[key]}"
          end
          ::Excon.display_warning("The following request keys are only valid with the associated middleware: " \
                                  "#{messages.join(", ")}")
        end
      end

      module Parsing
        def parse(json)
          return super if Ractor.main?

          real_parse(RactorSupport.parse_json(json), {})
        end
      end

      module Serialization
        def to_s
          return super if Ractor.main?

          JSON.generate(to_avro)
        end
      end

      module Configuration
        def disable_enum_symbol_validation
          return super if Ractor.main?

          @disable_enum_symbol_validation || ENV.fetch("AVRO_DISABLE_ENUM_SYMBOL_VALIDATION", "") != ""
        end

        def disable_field_default_validation
          return super if Ractor.main?

          @disable_field_default_validation || ENV.fetch("AVRO_DISABLE_FIELD_DEFAULT_VALIDATION", "") != ""
        end

        def disable_schema_name_validation
          return super if Ractor.main?

          @disable_schema_name_validation || ENV.fetch("AVRO_DISABLE_SCHEMA_NAME_VALIDATION", "") != ""
        end
      end

      module ExconState
        def defaults
          return super if Ractor.main?

          Ractor.store_if_absent(:avrocadabra_excon_defaults) { RactorSupport.excon_defaults }
        end

        def defaults=(value)
          if Ractor.main?
            super
          else
            Ractor[:avrocadabra_excon_defaults] = value || RactorSupport.excon_defaults
          end
        end
      end

      class << self
        def excon_defaults
          EXCON_DEFAULTS.transform_values do |value|
            value.is_a?(Hash) || value.is_a?(Array) ? value.dup : value
          end
        end

        def prepare
          %i[PRIMITIVE_TYPES NAMED_TYPES VALID_TYPES PRIMITIVE_TYPES_SYM NAMED_TYPES_SYM VALID_TYPES_SYM].each do |name|
            Ractor.make_shareable(::Avro::Schema.const_get(name))
          end
          Ractor.make_shareable(::Avro::LogicalTypes::IntDate::EPOCH_START)
          ::Avro::LogicalTypes.singleton_class.prepend(LogicalTypes)
          ::Excon::Error.singleton_class.prepend(StatusErrors)
          ::Excon::Connection.prepend(ConnectionKeys)
          ::Avro::Schema.singleton_class.prepend(Parsing)
          ::Avro::Schema.prepend(Serialization)
          ::Avro.singleton_class.prepend(Configuration)
          ::Excon.singleton_class.prepend(ExconState)
        end

        def parse_json(json)
          json = json.read if json.respond_to?(:read)
          return if json.nil? || json.strip.empty?

          JSON.parse(json)
        rescue JSON::ParserError => e
          raise JSON_ERROR.build(e, json)
        end
      end
    end
  end
end
