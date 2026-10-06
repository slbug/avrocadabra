# frozen_string_literal: true

require "support/schema_registry"

RSpec.describe Avrocadabra::AvroTurf::Messaging do
  include_context "with a schema registry"

  let(:store) { AvroTurfFixture.schema_store }
  let(:options) { { registry: registry, schema_store: store, logger: Logger.new(nil) } }
  let(:native) { described_class.new(**options) }
  let(:reference) { AvroTurf::Messaging.new(**options) }

  def error_schema(name, fields, **properties)
    record_schema(name, fields, **properties).merge("type" => "error")
  end

  def expect_error_contract(definition, value)
    id = registry.register("errors", reference_schema(definition))
    bytes = reference.encode(value, schema_id: id)
    expect(native.encode(value, schema_id: id)).to eq(bytes)
    expected = reference.decode(bytes)
    expect(value_contract(native.decode(bytes))).to eq(value_contract(expected))
    schema = native_schema(definition)
    expect(schema.encode(value)).to eq(bytes.byteslice(5..).b)
    expect(value_contract(schema.decode(bytes.byteslice(5..)))).to eq(value_contract(expected))
    [id, bytes]
  end

  it "cross-decodes recursive error records with logical fields and metadata" do
    date = { "type" => "int", "logicalType" => "date" }
    definition = error_schema("Failure", [field("day", date), field("cause", %w[null Failure])])
    value = { "day" => Date.new(2000, 1, 1), "cause" => { "day" => Date.new(1999, 1, 1) } }
    id, bytes = expect_error_contract(definition, value)
    message = native.decode_message(bytes)
    expect(message.schema_id).to eq(id)
    expect(message.writer_schema.type_sym).to eq(:error)
    expect(native.encode(value, schema_id: id, validate: true)).to eq(bytes)
  end

  %i[array map union].each do |container|
    it "cross-decodes error records within #{container}" do
      failure = error_schema("Failure", [field("id", "long")])
      definition, value = case container
                          when :array then [{ "type" => "array", "items" => failure }, [{ "id" => 1 }]]
                          when :map then [{ "type" => "map", "values" => failure }, { "entry" => { "id" => 1 } }]
                          when :union then [["null", failure], { "id" => 1 }]
                          end
      expect_error_contract(definition, value)
    end
  end

  it "resolves error names, field aliases, and reader defaults" do
    writer = error_schema("OldFailure", [field("old_id", "int")], namespace: "before")
    reader = error_schema("NewFailure", [field("id", "long", aliases: ["old_id"]),
                                         field("message", "string", default: "unknown")],
                          namespace: "after", aliases: ["before.OldFailure"])
    allow(store).to receive(:find).with("Reader", nil).and_return(reference_schema(reader))
    _id, bytes = expect_error_contract(writer, { "old_id" => 42 })
    expected = reference.decode(bytes, schema_name: "Reader")
    expect(native.decode(bytes, schema_name: "Reader")).to eq(expected)
    expect(native_schema(writer).decode(bytes.byteslice(5..), reader_schema: native_schema(reader))).to eq(expected)
  end

  it "invalidates prepared codecs when an error's field schema changes" do
    choice = { "type" => "enum", "name" => "Choice", "symbols" => ["A"] }
    definition = error_schema("Failure", [field("choice", choice)])
    id = registry.register("errors", reference_schema(definition))
    results = [reference, native].map do |client|
      client.encode({ "choice" => "A" }, schema_id: id)
      client.fetch_schema_by_id(id).first.fields.first.type.symbols << "B"
      client.encode({ "choice" => "B" }, schema_id: id)
    end
    expect(results.last).to eq(results.first)
  end

  it "materializes error records supplied by reader defaults" do
    writer = record_schema("Value", [])
    failure = error_schema("Failure", [field("id", "long")])
    reader = record_schema("Value", [field("failure", failure, default: { "id" => 7 })])
    allow(store).to receive(:find).with("Reader", nil).and_return(reference_schema(reader))
    id = registry.register("values", reference_schema(writer))
    bytes = reference.encode({}, schema_id: id)
    expected = reference.decode(bytes, schema_name: "Reader")
    expect(native.decode(bytes, schema_name: "Reader")).to eq(expected)
    expect(native_schema(writer).decode(bytes.byteslice(5..), reader_schema: native_schema(reader))).to eq(expected)
  end

  it "resolves explicitly supplied error dependencies" do
    failure = error_schema("Failure", [field("id", "long")], namespace: "example")
    schema = Avrocadabra::Schema.new("\"example.Failure\"", references: [failure])
    inline = native_schema(failure)
    value = { "id" => 42 }.freeze
    bytes = reference_encode(failure, value)
    expect(schema.encode(value)).to eq(bytes)
    expect(schema.decode(bytes)).to eq(inline.decode(bytes))
    expect(schema.decode(bytes, reader_schema: inline)).to eq(value)
  end

  [%w[record error], %w[error record]].each do |writer_kind, reader_kind|
    it "rejects #{writer_kind}-to-#{reader_kind} evolution even when names match" do
      writer = record_schema("Failure", [field("id", "long")]).merge("type" => writer_kind)
      reader = writer.merge("type" => reader_kind)
      allow(store).to receive(:find).with("Reader", nil).and_return(reference_schema(reader))
      id = registry.register("errors", reference_schema(writer))
      bytes = reference.encode({ "id" => 1 }, schema_id: id)
      expect { reference.decode(bytes, schema_name: "Reader") }.to raise_error(Avro::IO::SchemaMatchException)
      expect { native.decode(bytes, schema_name: "Reader") }.to raise_error(Avro::IO::SchemaMatchException)
      expect { native_schema(writer).decode(bytes.byteslice(5..), reader_schema: native_schema(reader)) }
        .to raise_error(Avrocadabra::ResolutionError)
    end
  end

  it "raises a buffered write failure before a later encoding failure, as Ruby Avro does" do
    day = { "type" => "int", "logicalType" => "date" }
    schema = reference_schema(record_schema("Closed", [field("count", "long"), field("day", day)]))
    cache = Avrocadabra::AvroTurf::Cache.new
    write = lambda do |datum, io|
      Avrocadabra::AvroTurf.with_codecs(cache) do
        Avro::IO::DatumWriter.new(schema).write(datum, Avro::IO::BinaryEncoder.new(io))
      end
    end
    write.call({ "count" => 1, "day" => 3 }, StringIO.new(+"".b))
    io = StringIO.new(+"".b)
    closing = Hash.new { io.close_write || 1 }.merge("day" => Float::NAN)
    expect { write.call(closing, io) }.to raise_error(IOError, "not opened for writing")
  end
end
