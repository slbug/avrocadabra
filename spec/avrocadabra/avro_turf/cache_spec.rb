# frozen_string_literal: true

require "objspace"
require "support/avro_turf_fixture"

RSpec.describe Avrocadabra::AvroTurf::Cache do
  def pause_worker_check(codec, ready, resume)
    controller = Thread.current
    allow(codec).to receive(:current?).and_wrap_original do |method|
      result = method.call
      unless Thread.current == controller
        ready << true
        resume.pop
      end
      result
    end
  end

  it "reuses a schema without serializing its unchanged definition" do
    cache = described_class.new
    schema = reference_schema(record_schema("Node", [field("next", %w[null Node])]))
    codec = cache.fetch(schema)
    allow(schema).to receive(:to_avro).and_call_original
    expect(cache.fetch(schema)).to equal(codec)
    expect(schema).not_to have_received(:to_avro)
  end

  it "replaces a changed entry without evicting an unrelated entry at capacity" do
    cache = described_class.new
    schemas = Array.new(described_class::LIMIT) do |index|
      reference_schema({ "type" => "enum", "name" => "Choice#{index}", "symbols" => ["A"] })
    end
    codecs = schemas.map { cache.fetch(it) }
    schemas.last.symbols << "B"
    expect(cache.fetch(schemas.last)).not_to equal(codecs.last)
    expect(cache.fetch(schemas.first)).to equal(codecs.first)
  end

  it "observes in-place edits to strings, union branches and map defaults" do
    labels = field("labels", { "type" => "map", "values" => "string" }, default: { "a" => "b" })
    definition = record_schema("Mutable", [field("choice", %w[null string]), labels])
    schema = reference_schema(definition)
    cache = described_class.new
    codec = cache.fetch(schema)
    expect(cache.fetch(schema)).to equal(codec)
    schema.fields.last.default.fetch("a").replace("c")
    expect(cache.fetch(schema)).not_to equal(codec)
    codec = cache.fetch(schema)
    schema.fields.last.default["d"] = "e"
    expect(cache.fetch(schema)).not_to equal(codec)
    codec = cache.fetch(schema)
    schema.fields.first.type.schemas[1] = reference_schema("bytes")
    expect(cache.fetch(schema)).not_to equal(codec)
  end

  it "observes replacement attributes and default hash ordering" do
    definition = record_schema("Mutable", [field("labels", { "type" => "map", "values" => "string" },
                                                 default: { "a" => "b", "c" => "d" })])
    schema = reference_schema(definition)
    cache = described_class.new
    codec = cache.fetch(schema)
    labels = schema.fields.first.default
    labels["a"] = labels.delete("a")
    expect(cache.fetch(schema)).not_to equal(codec)
    codec = cache.fetch(schema)
    schema.fields.first.instance_variable_set(:@default, { "x" => "y" })
    expect(cache.fetch(schema)).not_to equal(codec)
  end

  it "recompiles a schema after its adapter changes" do
    schema = reference_schema("long")
    cache = described_class.new
    first = cache.fetch(schema)
    adapter = Object.new
    allow(adapter).to receive(:decode).with(2).and_return(7)
    schema.instance_variable_set(:@type_adapter, adapter)
    replacement = cache.fetch(schema)
    expect(replacement).not_to equal(first)
    decoder = Avro::IO::BinaryDecoder.new(StringIO.new(reference_encode("long", 2)))
    expect(replacement.read(decoder, replacement)).to eq(7)
  end

  it "reuses a replacement published while another thread checks a stale entry" do
    schema = reference_schema({ "type" => "enum", "name" => "Choice", "symbols" => ["A"] })
    cache = described_class.new
    stale = cache.fetch(schema)
    ready = Queue.new
    resume = Queue.new
    pause_worker_check(stale, ready, resume)
    schema.symbols << "B"
    worker = Thread.new { cache.fetch(schema) }
    expect(ready.pop(timeout: 2)).to be(true)
    replacement = Timeout.timeout(2) { cache.fetch(schema) }
    resume << true
    expect(worker.value).to equal(replacement)
  ensure
    resume << true if resume
    worker&.join(2)
  end

  it "observes mutable children of frozen containers and string encoding changes" do
    schema = reference_schema(record_schema("Mutable", [field("labels", { "type" => "array", "items" => "string" },
                                                              default: ["label"])]))
    schema.fields.first.default.freeze
    cache = described_class.new
    codec = cache.fetch(schema)
    schema.fields.first.default.first.force_encoding(Encoding::BINARY)
    expect(cache.fetch(schema)).not_to equal(codec)
    GC.verify_compaction_references(double_heap: true, toward: :empty)
    expect(cache.fetch(schema)).to equal(cache.fetch(schema))
  end

  it "observes mutable keys in identity hashes used as reader defaults" do
    schema = reference_schema(record_schema("Mutable", [field("labels", { "type" => "map", "values" => "long" },
                                                              default: {})]))
    key = +"first"
    defaults = {}.compare_by_identity
    defaults[key] = 1
    schema.fields.first.instance_variable_set(:@default, defaults)
    cache = described_class.new
    codec = cache.fetch(schema)
    key.replace("second")
    expect(cache.fetch(schema)).not_to equal(codec)
  end

  it "observes mutable values inside frozen hash defaults" do
    schema = reference_schema(record_schema("Indexed", [field("index", { "type" => "map", "values" => "string" },
                                                              default: { "key" => "value" })]))
    schema.fields.first.default.freeze
    cache = described_class.new
    codec = cache.fetch(schema)
    expect(cache.fetch(schema)).to equal(codec)
    schema.fields.first.default.fetch("key").replace("changed")
    expect(cache.fetch(schema)).not_to equal(codec)
  end

  it "shrinks write plans back to the limit once nested encodes release them" do
    cache = described_class.new
    schemas = Array.new(131) { reference_schema(record_schema("Held#{it}", [field("x", "long")])) }
    encode = lambda do |index|
      nested = Class.new(Hash) do
        define_method(:key?) do |key|
          encode.call(index + 1) if index < 129
          super(key)
        end
      end
      Avrocadabra::AvroTurf.with_codecs(cache) do
        Avro::IO::DatumWriter.new(schemas[index])
                             .write(nested.new.update("x" => 1), Avro::IO::BinaryEncoder.new(StringIO.new(+"".b)))
      end
    end
    encode.call(0)
    Avrocadabra::AvroTurf.with_codecs(cache) do
      Avro::IO::DatumWriter.new(schemas.last).write({ "x" => 1 }, Avro::IO::BinaryEncoder.new(StringIO.new(+"".b)))
    end
    roots = ObjectSpace.reachable_objects_from(cache.instance_variable_get(:@plans))
    expect(roots.count { it.is_a?(Avro::Schema::RecordSchema) }).to eq(128)
  end
end
