# frozen_string_literal: true

require "support/schema_registry"

RSpec.describe Avrocadabra::AvroTurf::Messaging do
  include_context "with a schema registry"

  let(:schema) { reference_schema("long") }
  let(:id) { registry.register("integers", schema) }
  let(:options) { { registry: registry, logger: Logger.new(nil) } }
  let(:client) { described_class.new(**options) }

  def with_locked_cache
    locked = Queue.new
    release = Queue.new
    mutex = client.instance_variable_get(:@avrocadabra_codecs).instance_variable_get(:@mutex)
    expect(mutex).to be_a(Mutex)
    worker = Thread.new do
      mutex.synchronize do
        locked << true
        release.pop
      end
    end
    locked.pop
    yield
  ensure
    release << true
    worker.join
  end

  it "isolates routing between fibers on the same thread" do
    cache = client.instance_variable_get(:@avrocadabra_codecs)
    fiber = Fiber.new do
      Avrocadabra::AvroTurf.with_codecs(cache) do
        Fiber.yield(Thread.current[:avrocadabra_codecs])
        Thread.current[:avrocadabra_codecs]
      end
    end
    expect(fiber.resume).to equal(cache)
    expect(Thread.current[:avrocadabra_codecs]).to be_nil
    expect(AvroTurf::Messaging.new(**options).decode(client.encode(42, schema_id: id))).to eq(42)
    expect(fiber.resume).to equal(cache)
    expect(Thread.current[:avrocadabra_codecs]).to be_nil
  end

  it "restores outer routing after nested stock calls and exceptions" do
    cache = client.instance_variable_get(:@avrocadabra_codecs)
    stock = AvroTurf::Messaging.new(**options)
    Avrocadabra::AvroTurf.with_codecs(cache) do
      expect(stock.decode(stock.encode(42, schema_id: id))).to eq(42)
      expect(Thread.current[:avrocadabra_codecs]).to equal(cache)
      expect { stock.encode("bad", schema_id: id) }.to raise_error(Avro::IO::AvroTypeError)
      expect(Thread.current[:avrocadabra_codecs]).to equal(cache)
    end
    expect(Thread.current[:avrocadabra_codecs]).to be_nil
  end

  it "does not retain routing after an exception from a native client" do
    expect { client.encode("bad", schema_id: id) }.to raise_error(Avro::IO::AvroTypeError)
    expect(Thread.current[:avrocadabra_codecs]).to be_nil
    expect(client.decode(client.encode(42, schema_id: id))).to eq(42)
  end

  it "survives GC and compaction with populated schema and resolution caches" do
    bytes = client.encode(42, schema_id: id)
    expect(client.decode(bytes)).to eq(42)
    GC.verify_compaction_references(double_heap: true, toward: :empty)
    expect(client.decode(bytes)).to eq(42)
    expect(client.encode(43, schema_id: id)).to eq(AvroTurf::Messaging.new(**options).encode(43, schema_id: id))
  end

  it "can reuse a client after forking while another thread holds its cache mutex" do
    skip "fork is unavailable" unless Process.respond_to?(:fork)

    bytes = client.encode(42, schema_id: id)
    result, status = with_locked_cache do
      fork_result do
        Thread.new do
          sleep 5
          exit! 2
        end
        [client.decode(bytes), client.decode(client.encode(43, schema_id: id))]
      end
    end
    expect(status.success?).to be(true)
    expect(result).to eq([42, 43])
  end

  it "bounds the per-instance native cache and prepares evicted schemas again" do
    cache = client.instance_variable_get(:@avrocadabra_codecs)
    allow(Avrocadabra::AvroTurf::Codec).to receive(:new).and_call_original
    first = reference_schema("long")
    cache.fetch(first)
    Avrocadabra::AvroTurf::Cache::LIMIT.times { cache.fetch(reference_schema("long")) }
    cache.fetch(first)
    expect(cache.instance_variable_get(:@entries).size).to eq(Avrocadabra::AvroTurf::Cache::LIMIT)
    expect(Avrocadabra::AvroTurf::Codec).to have_received(:new).with(equal(first)).twice
  end

  it "supports reused Ruby datum codecs with changing schemas and stream positions" do
    cache = client.instance_variable_get(:@avrocadabra_codecs)
    writer = Avro::IO::DatumWriter.new(schema)
    reader = Avro::IO::DatumReader.new(schema)
    io = StringIO.new("".b)
    encoder = Avro::IO::BinaryEncoder.new(io)
    decoder = Avro::IO::BinaryDecoder.new(io)
    Avrocadabra::AvroTurf.with_codecs(cache) do
      writer.write(42, encoder)
      string = reference_schema("string")
      writer.writers_schema = string
      writer.write("text", encoder)
      io.rewind
      expect(reader.read(decoder)).to eq(42)
      expect(io.pos).to eq(1)
      reader.writers_schema = string
      reader.readers_schema = nil
      expect(reader.read(decoder)).to eq("text")
      expect(io.eof?).to be(true)
    end
  end

  it "cross-decodes multi-datum containers while native routing is active" do
    cache = client.instance_variable_get(:@avrocadabra_codecs)
    io = StringIO.new("".b)
    Avrocadabra::AvroTurf.with_codecs(cache) do
      writer = Avro::DataFile::Writer.new(io, Avro::IO::DatumWriter.new(schema), schema)
      [1, 2, 3].each { writer << it }
      writer.close
      reader = Avro::DataFile::Reader.new(StringIO.new(io.string), Avro::IO::DatumReader.new)
      expect(reader.to_a).to eq([1, 2, 3])
    end
    reader = Avro::DataFile::Reader.new(StringIO.new(io.string), Avro::IO::DatumReader.new)
    expect(reader.to_a).to eq([1, 2, 3])
  end
end
