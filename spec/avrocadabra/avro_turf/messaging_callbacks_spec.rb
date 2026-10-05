# frozen_string_literal: true

require "support/schema_registry"

RSpec.describe Avrocadabra::AvroTurf::Messaging do
  include_context "with a schema registry"

  let(:options) { { registry: registry, logger: Logger.new(nil) } }
  let(:native) { described_class.new(**options) }
  let(:reference) { AvroTurf::Messaging.new(**options) }

  def expect_callback_contract(definition, factory, expected)
    id = registry.register("callbacks", reference_schema(definition))
    bytes = reference.encode(factory.call, schema_id: id)
    expect(reference.decode(bytes)).to eq(expected)
    expect(native.encode(factory.call, schema_id: id)).to eq(bytes)
    expect(native_schema(definition).encode(factory.call)).to eq(bytes.byteslice(5..).b)
  end

  %w[redefined_during_encode.rb redefined_adapter.rb redefined_conversion.rb].each do |fixture|
    it "matches Ruby Avro with methods redefined by #{fixture}" do
      results = [ruby_fixture(fixture, "stock"), ruby_fixture(fixture, "native")].map do |output, errors, status|
        expect(status.success?).to be(true), errors
        output
      end
      expect(results.last).to eq(results.first)
    end
  end

  %w[date date_time].each do |kind|
    it "matches Ruby Avro when #{kind} conversion calls a redefined Time constructor" do
      results = [ruby_fixture("redefined_time_constructor.rb", "stock", kind),
                 ruby_fixture("redefined_time_constructor.rb", "native", kind)].map do |output, errors, status|
        expect(status.success?).to be(true), errors
        output
      end
      expect(results.last).to eq(results.first)
    end
  end

  it "dispatches Array iteration before encoding each yielded item" do
    klass = Class.new(Array) do
      def each
        super { yield(it * 10) }
      end
    end
    expect_callback_contract({ "type" => "array", "items" => "long" }, -> { klass.new([12]).freeze }, [120])
  end

  it "dispatches Hash iteration for maps" do
    klass = Class.new(Hash) do
      def each
        super { |key, value| yield key, value * 10 }
      end
    end
    expect_callback_contract({ "type" => "map", "values" => "long" },
                             -> { klass.new.merge("value" => 12).freeze }, "value" => 120)
  end

  it "encodes yielded records before the iterator mutates them" do
    klass = Class.new(Array) do
      def each
        super do |value|
          yield value
          value["id"] = 99
          GC.compact
        end
      end
    end
    schema = { "type" => "array", "items" => record_schema("Item", [field("id", "long")]) }
    expect_callback_contract(schema, -> { klass.new([{ "id" => 12 }]) }, [{ "id" => 12 }])
  end

  [Encoding::UTF_8, Encoding::US_ASCII, Encoding::ISO_8859_1].each do |encoding|
    it "dispatches String transcoding from #{encoding}" do
      klass = Class.new(String) do
        def encode(*) = "changed"
      end
      expect_callback_contract("string", -> { klass.new("original").force_encoding(encoding).freeze }, "changed")
    end
  end

  it "preserves errors raised by custom transcoding" do
    value = +"original"
    def value.encode(*) = raise(IOError, "transcoding failed")
    id = registry.register("strings", reference_schema("string"))
    [reference, native].each do |client|
      expect { client.encode(value, schema_id: id) }.to raise_error(IOError, "transcoding failed")
    end
  end

  it "selects enum symbols without transcoding the value" do
    klass = Class.new(String) do
      def encode(*) = "B"
    end
    schema = { "type" => "enum", "name" => "Letter", "symbols" => %w[A B] }
    expect_callback_contract(schema, -> { klass.new("A").freeze }, "A")
  end

  it "does not invoke a failing enum transcoder" do
    klass = Class.new(String) do
      def encode(*) = raise(IOError, "unexpected transcoding")
    end
    schema = { "type" => "enum", "name" => "Letter", "symbols" => %w[A B] }
    expect_callback_contract(schema, -> { klass.new("A").freeze }, "A")
  end

  it "rejects enum values whose original encoding cannot match a symbol" do
    schema = { "type" => "enum", "name" => "Letter", "symbols" => %w[A B] }
    id = registry.register("letters", reference_schema(schema))
    value = "A".encode(Encoding::UTF_16LE).freeze
    [reference, native].each do |client|
      expect { client.encode(value, schema_id: id) }.to raise_error(Avro::IO::AvroTypeError)
    end
    expect { native_schema(schema).encode(value) }.to raise_error(Avrocadabra::EncodeError)
  end

  it "dispatches enum index lookup on the current schema" do
    schema = { "type" => "enum", "name" => "Letter", "symbols" => %w[A B] }
    id = registry.register("letters", reference_schema(schema))
    [reference, native].each do |client|
      expect(client.decode(client.encode("A", schema_id: id))).to eq("A")
      client.fetch_schema_by_id(id).first.symbols.define_singleton_method(:index) { |_value| 1 }
    end
    bytes = reference.encode("A", schema_id: id)
    expect(reference.decode(bytes)).to eq("B")
    expect(native.encode("A", schema_id: id)).to eq(bytes)
  end

  [Array, Hash].each do |parent|
    it "preserves private #{parent} iteration visibility" do
      klass = Class.new(parent) { private :each }
      schema = parent == Array ? { "type" => "array", "items" => "long" } : { "type" => "map", "values" => "long" }
      value = parent == Array ? klass.new([12]) : klass.new.merge("id" => 12)
      id = registry.register("private", reference_schema(schema))
      [reference, native].each do |client|
        expect { client.encode(value, schema_id: id) }.to raise_error(NoMethodError)
      end
    end
  end
end
