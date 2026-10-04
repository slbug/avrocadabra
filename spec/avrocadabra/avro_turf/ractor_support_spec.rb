# frozen_string_literal: true

RSpec::Matchers::BuiltIn.constants(false).each { RSpec::Matchers::BuiltIn.const_get(it) }

RSpec.describe Avrocadabra::AvroTurf::RactorSupport do
  describe ".prepare" do
    let(:schema_class) do
      Class.new do
        def self.real_parse(document, _names)
          document
        end

        def to_avro
          { "type" => "long" }
        end
      end
    end

    before do
      types = %i[PRIMITIVE_TYPES NAMED_TYPES VALID_TYPES PRIMITIVE_TYPES_SYM NAMED_TYPES_SYM VALID_TYPES_SYM]
      types.each { schema_class.const_set(it, Avro::Schema.const_get(it).dup) }
      epoch = Avro::LogicalTypes::IntDate::EPOCH_START.dup
      identity = Avro::LogicalTypes::Identity
      stub_const("Avro", Module.new)
      stub_const("Avro::Schema", schema_class)
      stub_const("Avro::LogicalTypes", Module.new)
      stub_const("Avro::LogicalTypes::Identity", identity)
      stub_const("Avro::LogicalTypes::IntDate", Module.new)
      stub_const("Avro::LogicalTypes::IntDate::EPOCH_START", epoch)
      stub_const("Excon", Module.new)
      stub_const("Excon::Error", Class.new(StandardError))
      stub_const("Excon::Connection", Class.new do
        def valid_middleware_keys(_middlewares)
          []
        end
      end)
      described_class.prepare
    end

    it "makes schema type tables and the date epoch readable from workers" do
      tables, epoch = Ractor.new do
        [Avro::Schema::PRIMITIVE_TYPES, Avro::LogicalTypes::IntDate::EPOCH_START]
      end.value
      expect(tables).to include("long", "string")
      expect(epoch).to eq(Date.new(1970, 1, 1))
    end

    it "enables worker schema parsing, serialization and validation settings" do
      result = Ractor.new do
        [Avro::Schema.parse('"long"'), Avro::Schema.new.to_s, Avro.disable_schema_name_validation]
      end.value
      expect(result).to eq(["long", '{"type":"long"}', false])
    end

    it "enables worker logical adapters and HTTP configuration" do
      result = Ractor.new do
        [Avro::LogicalTypes.type_adapter("int", "date").decode(1),
         Excon::Error.status_errors.fetch(404).last,
         Excon::Connection.new.valid_request_keys([]).include?(:headers), Excon.defaults[:connect_timeout]]
      end.value
      expect(result).to eq([Date.new(1970, 1, 2), "Not Found", true, described_class::EXCON_DEFAULTS[:connect_timeout]])
    end
  end

  it "leaves the main Ractor's dependency registries mutable" do
    types = Avro::LogicalTypes::TYPES.fetch("bytes")
    types["probe"] = Avro::LogicalTypes::IntDate
    Excon::VALID_REQUEST_KEYS << :probe
    Excon::VALID_CONNECTION_KEYS << :probe
    Excon::Error.status_errors[599] = [Excon::Error, "Probe"]
    expect(Avro::LogicalTypes.type_adapter("bytes", "probe")).to equal(Avro::LogicalTypes::IntDate)
    expect(described_class::REQUEST_KEYS).not_to include(:probe)
    expect(described_class::STATUS_ERRORS).not_to have_key(599)
  ensure
    types&.delete("probe")
    Excon::VALID_REQUEST_KEYS.delete(:probe)
    Excon::VALID_CONNECTION_KEYS.delete(:probe)
    Excon::Error.status_errors.delete(599)
  end

  context "with main Ractor JSON settings" do
    around do |example|
      parsing = MultiJson.parse_options
      generation = MultiJson.generate_options
      example.run
    ensure
      MultiJson.parse_options = parsing
      MultiJson.generate_options = generation
    end

    it "retains the configured schema parser options" do
      MultiJson.parse_options = { max_nesting: 1 }
      definition = { "type" => "array", "items" => { "type" => "array", "items" => "long" } }
      expect { Avro::Schema.parse(JSON.generate(definition)) }.to raise_error(MultiJson::ParseError)
    end

    it "retains the configured schema serializer options" do
      MultiJson.generate_options = { pretty: true }
      schema = reference_schema("type" => "array", "items" => "long")
      expect(schema.to_s).to eq(JSON.pretty_generate(schema.to_avro))
    end
  end

  describe ".parse_json" do
    it "parses strings and streams without changing their contents" do
      input = '{"type":"string","doc":"日本語"}'
      expected = { "type" => "string", "doc" => "日本語" }
      expect(described_class.parse_json(input)).to eq(expected)
      expect(described_class.parse_json(StringIO.new(input))).to eq(expected)
      expect(described_class.parse_json(input.b)).to eq(expected)
    end

    it "retains Ruby Avro's handling of blank JSON" do
      [nil, "", " \n\t", "null"].each { expect(described_class.parse_json(it)).to be_nil }
    end

    it "retains the JSON adapter's exception class, cause and input" do
      expect { described_class.parse_json("{") }.to raise_error(described_class::JSON_ERROR) do |error|
        expect(error.data).to eq("{")
        expect(error.cause).to be_a(JSON::ParserError)
      end
    end
  end

  context "with worker schema methods" do
    before { allow(Ractor).to receive(:main?).and_return(false) }

    it "parses and serializes schemas without invoking MultiJson" do
      definition = record_schema("Sample", [field("name", "string"), field("value", "long", default: 2)])
      schema = Avro::Schema.parse(JSON.generate(definition))
      expect(schema.fullname).to eq("Sample")
      expect(JSON.parse(schema.to_s)).to eq(definition)
    end

    it "rejects malformed and empty schema documents using the original exception classes" do
      expect { Avro::Schema.parse("{") }.to raise_error(described_class::JSON_ERROR)
      expect { Avro::Schema.parse(" ") }.to raise_error(Avro::SchemaParseError)
    end

    it "selects built-in logical adapters without reading the mutable registry" do
      decimal = reference_schema({ "type" => "bytes", "logicalType" => "decimal", "precision" => 8, "scale" => 2 })
      expect(decimal.type_adapter).to be_a(Avro::LogicalTypes::BytesDecimal)
      expect(decimal.type_adapter.decode(decimal.type_adapter.encode(1.25))).to eq(BigDecimal("1.25"))
      expect(Avro::LogicalTypes.type_adapter("string", "uuid")).to equal(Avro::LogicalTypes::Identity)
      expect(Avro::LogicalTypes.type_adapter("string", nil)).to be_nil
    end
  end

  describe "worker Excon configuration" do
    let(:connection) { Excon.new("https://example.invalid") }

    it "retains main-Ractor extensions to Excon's accepted options" do
      Excon::VALID_REQUEST_KEYS << :probe
      Excon::VALID_CONNECTION_KEYS << :probe
      allow(Excon).to receive(:display_warning)
      expect(connection.valid_request_keys([])).to include(:probe)
      connection.send(:validate_params, :connection, { probe: true, omit_default_port: true }, [])
      expect(Excon).not_to have_received(:display_warning)
    ensure
      Excon::VALID_REQUEST_KEYS.delete(:probe)
      Excon::VALID_CONNECTION_KEYS.delete(:probe)
    end

    it "provides mutable status mappings local to the worker" do
      original = Excon::Error.status_errors
      allow(Ractor).to receive(:main?).and_return(false)
      mapping = Excon::Error.status_errors
      mapping[599] = [Excon::Error, "Worker"]
      expect(Excon::Error.status_errors).to equal(mapping)
      expect(mapping).to include(599)
      expect(original).not_to have_key(599)
      expect(described_class::STATUS_ERRORS).not_to have_key(599)
    ensure
      Ractor[:avrocadabra_status_errors] = nil
    end

    [[:connection, { omit_default_port: true }], [:connection, { invalid: true }],
     [:request, { method: :get }], [:request, { retry_limit: 3, invalid: true }]].each do |validation, parameters|
      it "retains Excon warnings for #{validation} #{parameters.inspect}" do
        client = connection
        warnings = []
        allow(Excon).to receive(:display_warning) { warnings << it }
        client.send(:validate_params, validation, parameters, [])
        expected = warnings.dup
        warnings.clear
        allow(Ractor).to receive(:main?).and_return(false)
        client.send(:validate_params, validation, parameters, [])
        expect(warnings).to eq(expected)
      end
    end

    it "retains invalid validation errors" do
      client = connection
      allow(Ractor).to receive(:main?).and_return(false)
      expect { client.send(:validate_params, :invalid, {}, []) }
        .to raise_error(ArgumentError, "Invalid validation type 'invalid'")
    end
  end

  %w[enum_symbol field_default schema_name].each do |validation|
    describe "#{validation} validation" do
      let(:setting) { "disable_#{validation}_validation" }
      let(:environment) { "AVRO_DISABLE_#{validation.upcase}_VALIDATION" }
      let(:configuration) do
        name = setting
        Class.new do
          define_method(name) { :inherited }
        end.prepend(described_class::Configuration).new
      end

      before { allow(ENV).to receive(:fetch).and_call_original }

      it "preserves the main Ractor's configuration method" do
        expect(configuration.public_send(setting)).to eq(:inherited)
      end

      it "honors configured and environment values without writing module state" do
        allow(Ractor).to receive(:main?).and_return(false)
        allow(ENV).to receive(:fetch).with(environment, "").and_return("")
        expect(configuration.public_send(setting)).to be(false)
        expect(configuration.instance_variables).to be_empty
        allow(ENV).to receive(:fetch).with(environment, "").and_return("1")
        expect(configuration.public_send(setting)).to be(true)
        configuration.instance_variable_set(:"@#{setting}", true)
        allow(ENV).to receive(:fetch).with(environment, "").and_return("")
        expect(configuration.public_send(setting)).to be(true)
      end
    end
  end

  describe "Excon defaults" do
    let(:connection) { Class.new { attr_accessor :defaults }.prepend(described_class::ExconState).new }

    around do |example|
      previous = Ractor[:avrocadabra_excon_defaults]
      Ractor[:avrocadabra_excon_defaults] = described_class.excon_defaults
      example.run
    ensure
      Ractor[:avrocadabra_excon_defaults] = previous
    end

    it "preserves the main Ractor's getter and setter" do
      connection.defaults = { read_timeout: 3 }
      expect(connection.defaults).to eq(read_timeout: 3)
    end

    it "copies mutable settings once per Ractor and preserves scalar settings" do
      allow(Ractor).to receive(:main?).and_return(false)
      defaults = connection.defaults
      defaults[:headers]["X-Worker"] = "one"
      defaults[:middlewares] << Object
      expect(connection.defaults).to equal(defaults)
      expect(defaults[:connect_timeout]).to eq(described_class::EXCON_DEFAULTS[:connect_timeout])
      expect(defaults[:uri_parser]).to equal(URI)
      expect(described_class::EXCON_DEFAULTS[:headers]).not_to have_key("X-Worker")
      expect(described_class::EXCON_DEFAULTS[:middlewares].include?(Object)).to be(false)
    end

    it "replaces only the current Ractor's settings" do
      allow(Ractor).to receive(:main?).and_return(false)
      connection.defaults = { read_timeout: 3 }
      expect(connection.defaults).to eq(read_timeout: 3)
      expect(described_class::EXCON_DEFAULTS[:read_timeout]).not_to eq(3)
    end

    it "resets cleared worker settings to fresh defaults" do
      allow(Ractor).to receive(:main?).and_return(false)
      connection.defaults[:headers]["X-Worker"] = "one"
      connection.defaults = nil
      expect(connection.defaults).to eq(described_class::EXCON_DEFAULTS)
    end
  end
end
