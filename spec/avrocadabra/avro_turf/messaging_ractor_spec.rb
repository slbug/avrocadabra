# frozen_string_literal: true

require "support/avro_turf_fixture"

RSpec.describe Avrocadabra::AvroTurf::Messaging do
  attr_reader :server, :ca_file

  let(:tls) { false }
  let(:requests) { Queue.new }
  let(:schemas_path) { AvroTurfFixture.schema_store(writer, reader).instance_variable_get(:@path) }
  let(:datum) { { "count" => 42, "text" => "日本語", "amount" => BigDecimal("12.34"), "day" => Date.new(2024, 2, 29) } }
  let(:options) do
    { registry_url: AvroTurfFixture.registry_url(server), schemas_path: schemas_path, ssl_ca_file: ca_file,
      user: "reader", password: "secret" }
  end

  around do |example|
    app = lambda do |env|
      requests << env
      FakeConfluentSchemaRegistryServer.call(env)
    end
    AvroTurfFixture.with_registry(tls: tls, app: app) do |server|
      @server = server
      if tls
        @ca_file = File.join(Dir.mktmpdir("avrocadabra-tls-"), "registry.pem")
        File.write(ca_file, server[:SSLCertificate].to_pem)
      end
      example.run
    end
  end

  def writer
    record_schema("Sample", [field("count", "long"), field("text", "string"),
                             field("amount", { "type" => "bytes", "logicalType" => "decimal", "precision" => 8,
                                               "scale" => 2 }),
                             field("day", { "type" => "int", "logicalType" => "date" })])
  end

  def reader
    record_schema("CurrentSample", [field("total", "double", aliases: ["count"]),
                                    field("added", "string", default: "new")], aliases: ["Sample"])
  end

  def round_trip(options, datum)
    Ractor.new(described_class, options, datum) do |client_class, settings, value|
      client = client_class.new(**settings, logger: Logger.new(nil))
      bytes = client.encode(value, schema_name: "Sample", subject: "samples", validate: true)
      message = client.decode_message(bytes, schema_name: "CurrentSample")
      [bytes, client.decode(bytes), message.message,
       [message.schema_id, message.writer_schema.fullname, message.reader_schema.fullname]]
    end
  end

  it "cross-decodes concurrent worker messages with aliases, defaults, logical types and metadata" do
    workers = Array.new(4) { round_trip(options, datum) }
    reference = AvroTurf::Messaging.new(**options, logger: Logger.new(nil))
    stock = reference.encode(datum, schema_name: "Sample", subject: "samples")
    workers.map(&:value).each do |bytes, decoded, evolved, metadata|
      expect(reference.decode(bytes)).to eq(datum)
      expect(decoded).to eq(datum)
      expect(evolved).to eq("total" => 42.0, "added" => "new")
      expect(metadata).to eq([stock.byteslice(1, 4).unpack1("N"), "Sample", "CurrentSample"])
      expect(bytes.byteslice(0, 5)).to eq(stock.byteslice(0, 5))
    end
  end

  it "decodes Ruby messages by historical IDs and reuses prepared schemas" do
    reference = AvroTurf::Messaging.new(**options, logger: Logger.new(nil))
    bytes = reference.encode(datum, schema_name: "Sample", subject: "samples")
    result = Ractor.new(described_class, options, bytes) do |client_class, settings, payload|
      client = client_class.new(**settings, logger: Logger.new(nil))
      decoded = client.decode(payload)
      entries = client.instance_variable_get(:@avrocadabra_codecs).instance_variable_get(:@entries)
      first = entries.values.first
      20.times { client.decode(client.encode(decoded, subject: "samples", version: 1, register_schemas: false)) }
      [decoded, entries.size, first.equal?(entries.values.first)]
    end.value
    expect(result).to eq([datum, 1, true])
  end

  it "keeps validation, registry, framing and native exceptions inside the calling Ractor" do
    results = Ractor.new(described_class, options, datum) do |client_class, settings, value|
      client = client_class.new(**settings, logger: Logger.new(nil))
      bytes = client.encode(value, schema_name: "Sample")
      [-> { client.encode(value.merge("extra" => true), schema_name: "Sample", validate: true) },
       -> { client.encode(value, schema_id: 999) }, -> { client.decode("invalid") },
       -> { client.decode(bytes.byteslice(0, bytes.bytesize - 1)) },
       -> { client.encode(value.merge("count" => "wrong"), schema_name: "Sample") }].map do |operation|
        operation.call
      rescue StandardError => e
        [e.class.name, Thread.current[:avrocadabra_codecs]]
      end
    end.value
    expect(results).to eq(%w[Avro::SchemaValidator::ValidationError AvroTurf::SchemaNotFoundError RuntimeError EOFError
                             Avro::IO::AvroTypeError].map { [it, nil] })
  end

  it "isolates Excon defaults and connections between worker Ractors" do
    workers = %w[one two].map do |tag|
      Ractor.new(described_class, options, datum, tag) do |client_class, settings, value, name|
        Excon.defaults = Excon.defaults.merge(connect_timeout: 2)
        Excon.defaults[:headers]["X-Worker"] = name
        client = client_class.new(**settings, logger: Logger.new(nil))
        client.decode(client.encode(value, schema_name: "Sample", subject: name))
      end
    end
    expect(workers.map(&:value)).to all(eq(datum))
    headers = Array.new(requests.size) { requests.pop }
    expect(headers.map { it["HTTP_X_WORKER"] }.uniq.sort).to eq(%w[one two])
    expect(headers.map { it["HTTP_AUTHORIZATION"] }.uniq).to eq(["Basic #{["reader:secret"].pack("m0")}"])
    expect(Excon.defaults[:headers]).not_to have_key("X-Worker")
  end

  context "with HTTPS" do
    let(:tls) { true }

    it "verifies the registry certificate and round-trips in a worker" do
      bytes, decoded, evolved = round_trip(options, datum).value
      expect(decoded).to eq(datum)
      expect(evolved).to eq("total" => 42.0, "added" => "new")
      expect(bytes.byteslice(0, 5)).to eq("\x00\x00\x00\x00\x00".b)
    end
  end
end
