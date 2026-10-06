# frozen_string_literal: true

module AvroHelpers
  def native_schema(definition, **)
    Avrocadabra::Schema.new(JSON.generate(definition), **)
  end

  def reference_schema(definition)
    Avro::Schema.parse(JSON.generate(definition))
  end

  def reference_encode(definition, datum)
    output = StringIO.new("".b)
    Avro::IO::DatumWriter.new(reference_schema(definition)).write(datum, Avro::IO::BinaryEncoder.new(output))
    output.string
  end

  def reference_decode(definition, bytes, reader: definition)
    decoder = Avro::IO::BinaryDecoder.new(StringIO.new(bytes))
    Avro::IO::DatumReader.new(reference_schema(definition), reference_schema(reader)).read(decoder)
  end

  def value_contract(value)
    details = case value
              when String then [value.encoding, value.bytes]
              when Hash then value.map { |key, child| [value_contract(key), value_contract(child)] }
              when Array then value.map { value_contract(it) }
              else value
              end
    [value.class, details]
  end

  def expect_interoperable(definition, datum, expected: datum, reference_datum: datum)
    schema = native_schema(definition)
    encoded = schema.encode(datum)
    expect(encoded.encoding).to eq(Encoding::BINARY)
    expect(reference_decode(definition, encoded)).to eq(expected)
    expect(schema.decode(reference_encode(definition, reference_datum))).to eq(expected)
    expect(schema.decode(encoded)).to eq(expected)
    encoded
  end

  def record_schema(name, fields, **properties)
    { "type" => "record", "name" => name, "fields" => fields }.merge(properties.transform_keys(&:to_s))
  end

  def field(name, type, **properties)
    { "name" => name, "type" => type }.merge(properties.transform_keys(&:to_s))
  end

  def avro_long(value)
    output = StringIO.new("".b)
    Avro::IO::BinaryEncoder.new(output).write_long(value)
    output.string
  end

  def big_decimal_bytes(coefficient, scale)
    length = (coefficient.bit_length + 8) / 8
    unsigned = coefficient & ((1 << (length * 8)) - 1)
    signed = [unsigned.to_s(16).rjust(length * 2, "0")].pack("H*")
    reference_encode("bytes", reference_encode("bytes", signed) + avro_long(scale))
  end

  def fork_result
    read_pipe, write_pipe = IO.pipe
    pid = Process.fork do
      read_pipe.close
      write_pipe.write(JSON.generate(yield))
      write_pipe.close
      exit! 0
    rescue StandardError => e
      write_pipe.write(JSON.generate([e.class.name, e.message]))
      write_pipe.close
      exit! 1
    end
    write_pipe.close
    data = JSON.parse(read_pipe.read)
    _pid, status = Process.wait2(pid)
    [data, status]
  ensure
    read_pipe&.close unless read_pipe&.closed?
    write_pipe&.close unless write_pipe&.closed?
  end

  def avrocadabra_library
    File.dirname($LOADED_FEATURES.find { it.end_with?("/avrocadabra.rb") })
  end

  def ruby_subprocess(code, *, coverage: true, preload: true)
    flags = if coverage && ENV["COVERAGE"] == "true"
              ["-I", File.expand_path("..", __dir__),
               "-rsupport/subprocess_coverage"]
            else
              []
            end
    flags += ["-I", avrocadabra_library]
    flags << "-ravrocadabra" if preload
    Open3.capture3(RbConfig.ruby, *flags, "-e", code, *)
  end

  def ruby_fixture(name, *, coverage: true)
    ruby_subprocess(File.read(File.expand_path("../fixtures/#{name}", __dir__)), *, coverage:)
  end
end
