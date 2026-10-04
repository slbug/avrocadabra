# frozen_string_literal: true

require "support/schema_registry"

RSpec.describe Avrocadabra::AvroTurf::Messaging do
  include_context "with a schema registry"

  let(:definition) { record_schema("Value", [field("id", "long")]) }
  let(:options) { { registry: registry, logger: Logger.new(nil) } }
  let(:native) { described_class.new(**options) }
  let(:reference) { AvroTurf::Messaging.new(**options) }
  let(:schema_id) { registry.register("values", reference_schema(definition)) }

  def expect_accessor_contract(factory, expected)
    ruby_input = factory.call
    native_input = factory.call
    bytes = reference.encode(ruby_input, schema_id: schema_id)
    expect(reference.decode(bytes)).to eq("id" => expected)
    expect(native.encode(native_input, schema_id: schema_id)).to eq(bytes)
    expect(native_schema(definition).encode(factory.call)).to eq(bytes.byteslice(5..).b)
    expect(native_input).to eq(ruby_input)
  end

  ["id", :id].each do |key|
    it "dispatches overridden Hash accessors for #{key.inspect} keys" do
      klass = Class.new(Hash) do
        def [](key) = super * 10
      end
      expect_accessor_contract(-> { klass.new.merge(key => 12).freeze }, 120)
    end
  end

  it "dispatches singleton accessors on ordinary hashes" do
    factory = lambda do
      value = { "id" => 12 }
      def value.[](key) = super * 10
      value.freeze
    end
    expect_accessor_contract(factory, 120)
  end

  it "uses the overridden key predicate to choose string or symbol lookup" do
    klass = Class.new(Hash) do
      def key?(_key) = false
    end
    expect_accessor_contract(-> { klass.new.merge("id" => 12, id: 34).freeze }, 34)
  end

  it "uses Ruby truthiness for custom key predicates" do
    klass = Class.new(Hash) do
      define_method(:key?) { |_key| "present" }
      def [](key) = key == "id" ? 56 : 78
    end
    expect_accessor_contract(-> { klass.new.freeze }, 56)
  end

  it "honors prepended accessors and overridden Hash defaults" do
    overrides = Module.new do
      def [](key) = super * 10
    end
    klass = Class.new(Hash) do
      def default(_key) = 9
    end
    klass.prepend(overrides)
    expect_accessor_contract(-> { klass.new.freeze }, 90)
  end

  it "preserves accessor exceptions" do
    klass = Class.new(Hash) do
      def [](_key) = raise(IOError, "lookup failed")
    end
    expect { reference.encode(klass.new, schema_id: schema_id) }.to raise_error(IOError, "lookup failed")
    expect { native.encode(klass.new, schema_id: schema_id) }.to raise_error(IOError, "lookup failed")
  end

  %i[private protected].product(%i[key? []], [false, true]).each do |visibility, method, override|
    it "preserves #{visibility} visibility for #{method} with override #{override}" do
      klass = Class.new(Hash)
      klass.define_method(method) { |key| super(key) } if override
      klass.__send__(visibility, method)
      value = klass.new.merge("id" => 12)
      expect { reference.encode(value, schema_id: schema_id) }.to raise_error(NoMethodError)
      expect { native.encode(value, schema_id: schema_id) }.to raise_error(NoMethodError)
      expect { native_schema(definition).encode(value) }.to raise_error(NoMethodError)
    end
  end

  it "dispatches undefined accessors through method_missing" do
    klass = Class.new(Hash) do
      undef_method :[]
      def method_missing(name, *args) = name == :[] ? 120 : super
      def respond_to_missing?(name, include_private = false) = name == :[] || super
    end
    expect_accessor_contract(-> { klass.new.merge("id" => 12).freeze }, 120)
  end

  it "observes accessor replacement during a record conversion" do
    schema = record_schema("Changing", [field("first", "long"), field("second", "long")])
    id = registry.register("changing", reference_schema(schema))
    results = [reference, native].map do |client|
      value = Hash.new do |hash, _key|
        def hash.[](_key) = 22
        GC.compact
        11
      end
      client.decode(client.encode(value, schema_id: id))
    end
    expect(results).to eq([{ "first" => 11, "second" => 22 }] * 2)
  end
end
