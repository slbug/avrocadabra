# frozen_string_literal: true

require "support/schema_registry"

RSpec.describe Avrocadabra::AvroTurf::Messaging do
  include_context "with a schema registry"

  def definition
    record_schema("Event", [field("id", "long"), field("label", "string"), field("enabled", "boolean"),
                            field("note", %w[null string]), field("tags", { "type" => "map", "values" => "string" })])
  end

  def datum
    { "id" => 7, "label" => "日本語", "enabled" => false, "note" => nil, "tags" => { "b" => "2", "a" => "1" } }
  end
  let(:store) { AvroTurfFixture.schema_store(definition) }
  let(:native) { messaging }
  let(:reference) { AvroTurf::Messaging.new(registry: registry, schema_store: store, logger: Logger.new(nil)) }

  def schema_id
    registry.register("events", store.find("Event"))
  end

  def messaging(**)
    described_class.new(registry: registry, schema_store: store, logger: Logger.new(nil), **)
  end

  it "cross-decodes through the published Messaging API with identical Confluent headers" do
    ruby_bytes = reference.encode(datum, schema_id: schema_id)
    native_bytes = native.encode(datum, schema_id: schema_id)
    expect(native_bytes.byteslice(0, 5)).to eq("\x00".b + [schema_id].pack("N"))
    expect(native_bytes.byteslice(0, 5)).to eq(ruby_bytes.byteslice(0, 5))
    expect(native_bytes.encoding).to eq(ruby_bytes.encoding)
    expect(reference.decode(native_bytes)).to eq(datum)
    expect(native.decode(ruby_bytes)).to eq(datum)
  end

  it "registers schema_name under the requested subject" do
    allow(upstream).to receive(:register).and_call_original
    bytes = native.encode(datum, schema_name: "Event", subject: "events")
    expect(registry.subject_version("events").fetch("id")).to eq(bytes.byteslice(1, 4).unpack1("N"))
    expect(native.decode(bytes)).to eq(datum)
    expect(upstream).to have_received(:register).with("events", store.find("Event")).once
  end

  it "uses the schema full name as the default subject" do
    bytes = native.encode(datum, schema_name: "Event")
    expect(registry.subject_version("Event").fetch("id")).to eq(bytes.byteslice(1, 4).unpack1("N"))
  end

  it "uses an explicit namespace for schema lookup" do
    namespaced = definition.merge("namespace" => "example")
    client = messaging(schema_store: AvroTurfFixture.schema_store(namespaced), namespace: "example")
    bytes = client.encode(datum, schema_name: "Event")
    expect(client.decode_message(bytes, schema_name: "Event").reader_schema.fullname).to eq("example.Event")
    expect(registry.subject_version("example.Event").fetch("id")).to eq(bytes.byteslice(1, 4).unpack1("N"))
  end

  it "fetches a subject version without registering" do
    id = schema_id
    allow(upstream).to receive(:register).and_call_original
    bytes = native.encode(datum, subject: "events", version: 1)
    expect(bytes.byteslice(1, 4).unpack1("N")).to eq(id)
    expect(reference.decode(bytes)).to eq(datum)
    expect(upstream).not_to have_received(:register)
  end

  it "looks up schema bodies with register_schemas disabled" do
    id = schema_id
    allow(upstream).to receive(:register).and_call_original
    allow(upstream).to receive(:check).and_call_original
    2.times { native.encode(datum, schema_name: "Event", subject: "events", register_schemas: false) }
    expect(upstream).to have_received(:check).with("events", store.find("Event")).once
    expect(upstream).not_to have_received(:register)
    expect(native.decode(reference.encode(datum, schema_id: id))).to eq(datum)
  end

  it "keeps schema_id precedence over subject, version and schema_name" do
    bytes = native.encode(datum, schema_id: schema_id, schema_name: "Missing", subject: "missing", version: 999)
    expect(reference.decode(bytes)).to eq(datum)
  end

  it "keeps subject/version precedence over schema_name" do
    schema_id
    bytes = native.encode(datum, subject: "events", version: 1, schema_name: "Missing")
    expect(reference.decode(bytes)).to eq(datum)
  end

  it "preserves AvroTurf validation and rejects extra fields" do
    expect(reference.decode(native.encode(datum, schema_name: "Event", validate: true))).to eq(datum)
    [datum.merge("extra" => 1), datum.merge("id" => "wrong")].each do |invalid|
      expect { reference.encode(invalid, schema_name: "Event", validate: true) }
        .to raise_error(Avro::SchemaValidator::ValidationError) do |error|
          expect do
            native.encode(invalid, schema_name: "Event", validate: true)
          end.to raise_error(error.class, error.message)
        end
    end
  end

  it "preserves DecodedMessage metadata without a reader schema" do
    result = native.decode_message(reference.encode(datum, schema_id: schema_id))
    expect(result).to be_a(AvroTurf::Messaging::DecodedMessage)
    expect(result.schema_id).to eq(schema_id)
    expect(result.writer_schema).to be_a(Avro::Schema::RecordSchema)
    expect(result.writer_schema.fullname).to eq("Event")
    expect(result.reader_schema).to be_nil
    expect(result.message).to eq(datum)
  end

  it "resolves historical writer IDs using reader aliases, defaults and logical types" do
    decimal = { "type" => "bytes", "logicalType" => "decimal", "precision" => 8, "scale" => 2 }
    date = { "type" => "int", "logicalType" => "date" }
    old = record_schema("Earlier", [field("id", "int"), field("measure", decimal), field("day", date)])
    fields = [field("sequence", "long", aliases: ["id"]), field("measure", decimal),
              field("day", date), field("label", "string", default: "new")]
    evolved = record_schema("Current", fields, aliases: ["Earlier"])
    id = registry.register("events", reference_schema(old))
    registry.register("events", reference_schema(evolved))
    value = { "id" => 42, "measure" => BigDecimal("12.34"), "day" => Date.new(2000, 2, 29) }
    client = messaging(schema_store: AvroTurfFixture.schema_store(evolved))
    result = client.decode_message(reference.encode(value, schema_id: id), schema_name: "Current")
    expect(result.schema_id).to eq(id)
    expect(result.writer_schema.fullname).to eq("Earlier")
    expect(result.reader_schema.fullname).to eq("Current")
    expect(result.message).to eq("sequence" => 42, "measure" => value.fetch("measure"),
                                 "day" => value.fetch("day"), "label" => "new")
  end

  it "matches omitted nullable fields and decimal Floats without changing caller values" do
    missing = datum.except("note").freeze
    expect(native.decode(native.encode(missing, schema_name: "Event"))).to eq(datum)
    expect(missing).not_to have_key("note")
    decimal = { "type" => "bytes", "logicalType" => "decimal", "precision" => 8, "scale" => 2 }
    id = registry.register("measurement", reference_schema(decimal))
    expect(native.decode(native.encode(1.25,
                                       schema_id: id))).to eq(reference.decode(reference.encode(1.25, schema_id: id)))
  end

  it "preserves frozen inputs and native string/symbol key precedence" do
    input = Ractor.make_shareable(datum.transform_keys(&:to_sym).merge("enabled" => false, enabled: true))
    snapshot = Marshal.dump(input)
    bytes = native.encode(input, schema_name: "Event").freeze
    encoded_snapshot = bytes.dup
    expect(native.decode(bytes)).to eq(datum)
    expect(Marshal.dump(input)).to eq(snapshot)
    expect(bytes).to eq(encoded_snapshot)
  end

  it "leaves trailing bytes unread like stock AvroTurf" do
    bytes = "#{reference.encode(datum, schema_id: schema_id)}trailing"
    expect(native.decode(bytes)).to eq(reference.decode(bytes))
  end

  it "decodes a zero-byte null payload" do
    id = registry.register("empty", reference_schema("null"))
    bytes = native.encode(nil, schema_id: id)
    expect(bytes.bytesize).to eq(5)
    expect(native.decode(bytes)).to be_nil
    expect(reference.decode(bytes)).to be_nil
  end

  it "preserves aliases inside arrays, maps and unions without modifying Avro schemas" do
    child = record_schema("Child", [field("old", "int")])
    old = record_schema("Nested", [field("array", { "type" => "array", "items" => child }),
                                   field("map", { "type" => "map", "values" => "Child" }),
                                   field("union", %w[null Child])])
    current_child = record_schema("Child", [field("new", "long", aliases: ["old"])])
    current = record_schema("Nested", [field("array", { "type" => "array", "items" => current_child }),
                                       field("map", { "type" => "map", "values" => "Child" }),
                                       field("union", %w[null Child])])
    id = registry.register("nested", reference_schema(old))
    store = AvroTurfFixture.schema_store(current)
    schema = store.find("Nested")
    before = schema.to_s
    value = { "array" => [{ "old" => 1 }], "map" => { "key" => { "old" => 2 } }, "union" => { "old" => 3 } }
    decoded = messaging(schema_store: store).decode(reference.encode(value, schema_id: id), schema_name: "Nested")
    expect(decoded).to eq("array" => [{ "new" => 1 }], "map" => { "key" => { "new" => 2 } }, "union" => { "new" => 3 })
    expect(schema.to_s).to eq(before)
  end

  it "passes registry authentication and context options to AvroTurf" do
    allow(AvroTurf::ConfluentSchemaRegistry).to receive(:new).and_call_original
    messaging(registry: nil, registry_url: "https://registry.example.invalid", user: "reader", password: "secret",
              schema_context: "events", connect_timeout: 3)
    expect(AvroTurf::ConfluentSchemaRegistry).to have_received(:new)
      .with("https://registry.example.invalid", hash_including(user: "reader", password: "secret",
                                                               schema_context: "events", connect_timeout: 3))
  end

  [{}, { subject: "missing" }, { schema_name: "Missing" }, { schema_id: 999 },
   { subject: "missing", version: 1 }, { schema_name: "Event", register_schemas: false }].each do |options|
    it "preserves AvroTurf's encoding error for #{options.inspect}" do
      expect { reference.encode(datum, **options) }.to raise_error(StandardError) do |error|
        expect { native.encode(datum, **options) }.to raise_error(error.class, error.message)
      end
    end
  end

  ["", "\x01", "\x00", "\x00\x01", "\x00\x00\x00\x00"].each do |header|
    it "preserves AvroTurf's header error for #{header.inspect}" do
      expect { reference.decode(header) }.to raise_error(StandardError) do |error|
        expect { native.decode(header) }.to raise_error(error.class, error.message)
      end
    end
  end

  it "preserves missing writer and reader schema errors" do
    bytes = "\x00".b + [999].pack("N")
    expect { native.decode(bytes) }.to raise_error(AvroTurf::SchemaNotFoundError, /id: 999/)
    expect { native.decode(bytes, schema_name: "Missing") }
      .to raise_error(AvroTurf::SchemaNotFoundError, /could not find Avro schema/)
  end

  it "preserves incompatible registry schema types" do
    allow(upstream).to receive(:subject_version).with("events", 1).and_return("id" => 1, "schemaType" => "JSON")
    expect { native.encode(datum, subject: "events", version: 1) }
      .to raise_error(AvroTurf::IncompatibleSchemaError, "The JSON schema for events is incompatible.")
  end

  it "passes through registry transport failures without retrying codecs" do
    failure = Excon::Error::Socket.new(IOError.new("unavailable"))
    allow(upstream).to receive(:fetch).with(999).and_raise(failure)
    expect { native.encode(datum, schema_id: 999) }.to raise_error(failure)
    expect { native.decode("\x00".b + [999].pack("N")) }.to raise_error(failure)
  end

  it "reuses prepared schemas across concurrent calls" do
    id = schema_id
    native.fetch_schema_by_id(id)
    allow(Avrocadabra::AvroTurf::Codec).to receive(:new).and_call_original
    results = Array.new(8) do
      Thread.new { Array.new(30) { native.decode(native.encode(datum, schema_id: id)) } }
    end.flat_map(&:value)
    expect(results).to all(eq(datum))
    expect(Avrocadabra::AvroTurf::Codec).to have_received(:new).once
  end

  it "keeps native caches scoped to each messaging instance" do
    allow(Avrocadabra::AvroTurf::Codec).to receive(:new).and_call_original
    2.times { messaging.encode(datum, schema_name: "Event") }
    expect(Avrocadabra::AvroTurf::Codec).to have_received(:new).twice
  end

  it "leaves ordinary Messaging clients on the Ruby codecs" do
    allow(Avrocadabra::AvroTurf::Codec).to receive(:new).and_call_original
    expect(reference.decode(reference.encode(datum, schema_name: "Event"))).to eq(datum)
    expect(Avrocadabra::AvroTurf::Codec).not_to have_received(:new)
    path = AvroTurf::Messaging.instance_method(:encode).super_method.source_location.first
    expect(path).to start_with(Gem.loaded_specs.fetch("avro_turf").full_gem_path)
  end

  it "leaves multi-datum Ruby Avro container readers intact" do
    io = StringIO.new("".b)
    schema = reference_schema(definition)
    writer = Avro::DataFile::Writer.new(io, Avro::IO::DatumWriter.new(schema), schema)
    3.times { writer << datum }
    writer.close
    reader = Avro::DataFile::Reader.new(StringIO.new(io.string), Avro::IO::DatumReader.new)
    expect(reader.to_a).to eq([datum, datum, datum])
  end
end
