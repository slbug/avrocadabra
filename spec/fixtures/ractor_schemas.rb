# frozen_string_literal: true

module RactorSchemaData
  class << self
    def packet
      { "type" => "record", "name" => "Packet", "fields" => [
        { "name" => "id", "type" => "long" }, { "name" => "text", "type" => "string" },
        { "name" => "raw", "type" => "bytes" }, { "name" => "enabled", "type" => "boolean" },
        { "name" => "numbers", "type" => { "type" => "array", "items" => "int" } },
        { "name" => "labels", "type" => { "type" => "map", "values" => "string" } },
        { "name" => "optional", "type" => %w[null string] }
      ] }
    end

    def datum(index)
      { "id" => (2**40) + index, "text" => "日本語 #{index}", "raw" => "\x00\xff".b, "enabled" => false,
        "numbers" => [-1, 0, index], "labels" => { "key" => "value" }, "optional" => nil }
    end

    def logical
      { "type" => "record", "name" => "Logical", "fields" => [
        { "name" => "decimal", "type" => { "type" => "bytes", "logicalType" => "decimal",
                                           "precision" => 40, "scale" => 10 } },
        { "name" => "variable", "type" => { "type" => "bytes", "logicalType" => "big-decimal" } },
        { "name" => "date", "type" => { "type" => "int", "logicalType" => "date" } },
        { "name" => "timestamp", "type" => { "type" => "long", "logicalType" => "timestamp-nanos" } },
        { "name" => "local", "type" => { "type" => "long", "logicalType" => "local-timestamp-nanos" } },
        { "name" => "time", "type" => { "type" => "long", "logicalType" => "time-micros" } },
        { "name" => "uuid", "type" => { "type" => "string", "logicalType" => "uuid" } },
        { "name" => "duration", "type" => { "type" => "fixed", "name" => "Elapsed", "size" => 12,
                                            "logicalType" => "duration" } }
      ] }
    end

    def logical_datum
      { "decimal" => BigDecimal("12345678901234567890.0123456789"), "variable" => BigDecimal("-1.23456789e1000000"),
        "date" => Date.new(1000, 1, 1, Date::GREGORIAN), "timestamp" => Time.at(Rational(-123_456_789, 10**9)).utc,
        "local" => -123_456_789, "time" => 123_456_789, "uuid" => "123e4567-e89b-12d3-a456-426614174000",
        "duration" => Avrocadabra::Duration.new(months: 1, days: 2, milliseconds: (2**32) - 1) }
    end

    def resolution_pair
      writer = { "type" => "record", "name" => "Entry", "namespace" => "original",
                 "fields" => [{ "name" => "value", "type" => "int" }] }
      reader = { "type" => "record", "name" => "Entry", "namespace" => "evolved", "aliases" => ["original.Entry"],
                 "fields" => [{ "name" => "renamed", "type" => "long", "aliases" => ["value"] },
                              { "name" => "tags", "type" => { "type" => "array", "items" => "string" },
                                "default" => ["initial"] }] }
      [Avrocadabra::Schema.new('"original.Entry"', references: [writer]),
       Avrocadabra::Schema.new('"evolved.Entry"', references: [reader])]
    end

    def equal(expected, actual)
      raise "unexpected result: #{actual.inspect}" unless actual == expected
    end

    def round_trip(schema, datum, release_gvl)
      bytes = schema.encode(datum, release_gvl: release_gvl)
      equal(Encoding::BINARY, bytes.encoding)
      equal(datum, schema.decode(bytes, release_gvl: release_gvl))
    end
  end
end

module RactorSchemaChecks
  class << self
    def local(release_gvl)
      workers = Array.new(4) do |index|
        Ractor.new(index, release_gvl) do |number, release|
          definition = RactorSchemaData.packet
          definition = JSON.generate(definition) if number.even?
          schema = Avrocadabra::Schema.new(definition)
          25.times { RactorSchemaData.round_trip(schema, RactorSchemaData.datum(number), release) }
          raise "locally constructed schema is not shareable" unless Ractor.shareable?(schema)

          schema.object_id
        end
      end
      RactorSchemaData.equal(4, workers.map(&:value).uniq.length)
    end

    def shared(release_gvl)
      schema = Avrocadabra::Schema.new(RactorSchemaData.packet)
      raise "prepared schema is not shareable" unless Ractor.shareable?(schema)

      original_id = schema.object_id
      expected = Ractor.make_shareable(RactorSchemaData.datum(42))
      workers = Array.new(4) do
        Ractor.new(schema, expected, original_id, release_gvl) do |prepared, datum, identity, release|
          RactorSchemaData.equal(identity, prepared.object_id)
          40.times { RactorSchemaData.round_trip(prepared, datum, release) }
          true
        end
      end
      RactorSchemaData.equal([true] * 4, workers.map(&:value))
      RactorSchemaData.round_trip(schema, expected, release_gvl)
    end

    def logical(release_gvl)
      schema = Avrocadabra::Schema.new(RactorSchemaData.logical)
      workers = Array.new(4) do
        Ractor.new(schema, release_gvl) do |prepared, release|
          datum = RactorSchemaData.logical_datum
          BigDecimal.save_limit do
            BigDecimal.limit(3)
            20.times { RactorSchemaData.round_trip(prepared, datum, release) }
          end
          true
        end
      end
      RactorSchemaData.equal([true] * 4, workers.map(&:value))
    end

    def resolution(release_gvl)
      writer, reader = RactorSchemaData.resolution_pair
      workers = Array.new(4) do |index|
        Ractor.new(writer, reader, index, release_gvl) do |prepared, evolved, number, release|
          30.times do
            bytes = prepared.encode({ value: number }, release_gvl: release)
            options = { reader_schema: evolved, release_gvl: release }
            decoded = prepared.decode(bytes, **options)
            RactorSchemaData.equal({ "renamed" => number, "tags" => ["initial"] }, decoded)
            decoded.fetch("tags") << "local mutation"
            RactorSchemaData.equal(["initial"], prepared.decode(bytes, **options).fetch("tags"))
          end
          true
        end
      end
      RactorSchemaData.equal([true] * 4, workers.map(&:value))
    end

    def errors(release_gvl)
      schema = Avrocadabra::Schema.new('"int"')
      workers = Array.new(4) do
        Ractor.new(schema, release_gvl) do |prepared, release|
          failures = [-> { prepared.encode("invalid", release_gvl: release) },
                      -> { prepared.decode("\x80".b, release_gvl: release) },
                      -> { Avrocadabra::Schema.new("{") },
                      -> { prepared.decode("\x00".b, reader_schema: "invalid", release_gvl: release) }]
          failures.map do |operation|
            operation.call
            raise "invalid datum succeeded"
          rescue Avrocadabra::Error => e
            e.class.name
          end
        end
      end
      expected = %w[Avrocadabra::EncodeError Avrocadabra::DecodeError Avrocadabra::SchemaError Avrocadabra::DecodeError]
      workers.each { RactorSchemaData.equal(expected, it.value) }
    end
  end
end

module RactorSchemaLifecycle
  class << self
    def compaction(release_gvl)
      schema = Avrocadabra::Schema.new(RactorSchemaData.packet)
      ready = Ractor::Port.new
      workers = Array.new(4) do
        Ractor.new(schema, ready, release_gvl) do |prepared, ready_port, release|
          start = Ractor::Port.new
          ready_port.send(start)
          start.receive
          100.times { RactorSchemaData.round_trip(prepared, RactorSchemaData.datum(7), release) }
          true
        end
      end
      starts = Array.new(4) { ready.receive }
      GC.start
      GC.compact
      starts.each { it.send(:start) }
      3.times do
        GC.start
        GC.compact
      end
      RactorSchemaData.equal([true] * 4, workers.map(&:value))
      if GC.respond_to?(:verify_compaction_references)
        GC.verify_compaction_references(double_heap: true,
                                        toward: :empty)
      end
      RactorSchemaData.round_trip(schema, RactorSchemaData.datum(7), release_gvl)
    end

    def teardown(release_gvl)
      8.times do
        prepared = Ractor.new(release_gvl) do |release|
          10.times do
            temporary = Avrocadabra::Schema.new(RactorSchemaData.packet)
            RactorSchemaData.round_trip(temporary, RactorSchemaData.datum(9), release)
          end
          Avrocadabra::Schema.new(RactorSchemaData.logical)
        end.value
        GC.start
        GC.compact
        consumer = Ractor.new(prepared, release_gvl) do |schema, release|
          RactorSchemaData.round_trip(schema, RactorSchemaData.logical_datum, release)
          true
        end
        RactorSchemaData.equal(true, consumer.value)
      end
    end
  end
end

check = ARGV.fetch(0)
release_gvl = ARGV.fetch(1) == "true"
runner = %w[compaction teardown].include?(check) ? RactorSchemaLifecycle : RactorSchemaChecks
runner.public_send(check, release_gvl)
puts "ok"
