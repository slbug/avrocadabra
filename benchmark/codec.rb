# frozen_string_literal: true

require "avrocadabra"
require "avro"
require "bigdecimal"
require "json"
require "optparse"
require "stringio"

module AvrocadabraBenchmark
  CLOCK = Process::CLOCK_MONOTONIC

  SCHEMA = {
    "type" => "record", "name" => "Batch", "namespace" => "benchmark",
    "fields" => [
      { "name" => "id", "type" => "long" },
      { "name" => "created_at", "type" => { "type" => "long", "logicalType" => "timestamp-micros" } },
      { "name" => "source", "type" => "string" },
      { "name" => "baseline",
        "type" => { "type" => "bytes", "logicalType" => "decimal", "precision" => 18, "scale" => 4 } },
      { "name" => "active", "type" => "boolean" },
      { "name" => "note", "type" => %w[null string] },
      { "name" => "metadata", "type" => { "type" => "map", "values" => "string" } },
      { "name" => "blob", "type" => "bytes" },
      { "name" => "items", "type" => {
        "type" => "array", "items" => {
          "type" => "record", "name" => "Sample", "fields" => [
            { "name" => "id", "type" => "long" },
            { "name" => "label", "type" => "string" },
            { "name" => "category", "type" => "string" },
            { "name" => "description", "type" => "string" },
            { "name" => "reading",
              "type" => { "type" => "bytes", "logicalType" => "decimal", "precision" => 12, "scale" => 4 } },
            { "name" => "state",
              "type" => { "type" => "enum", "name" => "State", "symbols" => %w[NEW READY DISABLED] } },
            { "name" => "annotation", "type" => %w[null string] }
          ]
        }
      } }
    ]
  }.freeze

  def self.payload(items:)
    random = Random.new(20_261_003)
    { "id" => 4_294_967_296, "created_at" => Time.at(1_791_000_000, 123_456, :microsecond).utc,
      "source" => "node-001", "baseline" => BigDecimal("23.4567"),
      "active" => false, "note" => "測定データ 🌡",
      "metadata" => { "channel" => "stream-a", "format" => "raw" },
      "blob" => random.bytes(items * 32),
      "items" => Array.new(items) do |index|
        { "id" => index + 1, "label" => "Sensor α #{index}", "category" => "measurement",
          "description" => "Generic numeric sample", "reading" => BigDecimal("23.4567"),
          "state" => "READY", "annotation" => index.even? ? "sample-#{index + 1}" : nil }
      end }
  end

  class ReferenceCodec
    def initialize(schema)
      parsed = Avro::Schema.parse(JSON.generate(schema))
      @writer = Avro::IO::DatumWriter.new(parsed)
      @reader = Avro::IO::DatumReader.new(parsed, parsed)
    end

    def encode(datum)
      output = StringIO.new("".b)
      @writer.write(datum, Avro::IO::BinaryEncoder.new(output))
      output.string
    end

    def decode(bytes)
      @reader.read(Avro::IO::BinaryDecoder.new(StringIO.new(bytes)))
    end
  end

  class Runner
    def initialize(options)
      @options = options
    end

    def run
      native = Avrocadabra::Schema.new(SCHEMA)
      reference = ReferenceCodec.new(SCHEMA)
      rows = []
      { "small" => 3, "large" => 500 }.each do |name, size|
        next unless ["all", name].include?(@options[:case])

        datum = AvrocadabraBenchmark.payload(items: size)
        bytes = reference.encode(datum)
        verify(native, reference, datum, bytes)
        operations(native, reference, datum, bytes).each do |engine, operation, callable|
          rows << measure(callable).merge(case: name, engine: engine, operation: operation, bytes: bytes.bytesize,
                                          items: size)
        end
      end
      { environment: environment, options: @options, results: rows,
        methodology: "Prepared schemas; GC on; latency includes clock overhead; batch throughput; " \
                     "Ruby allocations exclude native memory." }
    end

    private

    def environment
      { ruby: RUBY_DESCRIPTION, platform: RUBY_PLATFORM, native_version: Avrocadabra::VERSION,
        avro_version: Gem.loaded_specs.fetch("avro").version.to_s }
    end

    def verify(native, reference, datum, bytes)
      raise "native decode mismatch" unless native.decode(bytes) == datum
      raise "Ruby avro decode mismatch" unless reference.decode(native.encode(datum)) == datum
    end

    def operations(native, reference, datum, bytes)
      [["ruby-avro", "encode", -> { reference.encode(datum) }],
       ["ruby-avro", "decode", -> { reference.decode(bytes) }],
       ["native GVL held", "encode", -> { native.encode(datum, release_gvl: false) }],
       ["native GVL held", "decode", -> { native.decode(bytes, release_gvl: false) }],
       ["native GVL released", "encode", -> { native.encode(datum, release_gvl: true) }],
       ["native GVL released", "decode", -> { native.decode(bytes, release_gvl: true) }]]
    end

    def clock
      Process.clock_gettime(CLOCK)
    end

    def warmup(callable)
      deadline = clock + @options[:warmup]
      callable.call while clock < deadline
    end

    def iterations_for(callable)
      count = 1
      loop do
        started = clock
        count.times { callable.call }
        duration = clock - started
        return (count * @options[:seconds] / duration).ceil.clamp(1, 100_000) if duration >= 0.02 || count >= 100_000

        count = [count * 4, 100_000].min
      end
    end

    def measure(callable)
      warmup(callable)
      iterations = iterations_for(callable)
      rounds = Array.new(@options[:rounds]) { measure_round(callable, iterations) }
      latencies = Array.new(@options[:samples]) do
        started = clock
        callable.call
        (clock - started) * 1_000_000
      end.sort
      total_operations = iterations * rounds.length
      { iterations_per_round: iterations, rounds: rounds.length,
        latency_p50_us: percentile(latencies, 0.50), latency_p95_us: percentile(latencies, 0.95),
        throughput_per_second: total_operations / rounds.sum { it.fetch(:seconds) },
        ruby_allocations_per_operation: rounds.sum { it.fetch(:allocations) }.fdiv(total_operations) }
    end

    def measure_round(callable, iterations)
      GC.start
      before = GC.stat(:total_allocated_objects)
      started = clock
      iterations.times { callable.call }
      duration = clock - started
      { seconds: duration, allocations: GC.stat(:total_allocated_objects) - before }
    end

    def percentile(values, fraction)
      values.fetch(((values.length - 1) * fraction).ceil)
    end
  end

  def self.options(argv)
    result = { rounds: 5, seconds: 0.25, warmup: 0.15, samples: 500, case: "all", json: nil }
    OptionParser.new do |parser|
      parser.banner = "Usage: bundle exec ruby #{$PROGRAM_NAME} [options]"
      parser.on("--rounds N", Integer, "Rounds per operation (5)") { result[:rounds] = it }
      parser.on("--seconds N", Float, "Seconds per round (0.25)") { result[:seconds] = it }
      parser.on("--warmup N", Float, "Warmup seconds per operation (0.15)") { result[:warmup] = it }
      parser.on("--samples N", Integer, "Latency samples per operation (500)") { result[:samples] = it }
      parser.on("--case NAME", %w[all small large], "Payload: all, small, large") { result[:case] = it }
      parser.on("--json PATH", "Write JSON results") { result[:json] = it }
    end.parse!(argv)
    unless result.values_at(:rounds, :seconds, :warmup, :samples).all?(&:positive?)
      raise ArgumentError, "rounds, seconds, warmup and samples must be positive"
    end

    result
  end

  def self.print_report(report)
    environment = report.fetch(:environment)
    puts environment.fetch(:ruby)
    puts "Avrocadabra #{environment.fetch(:native_version)}; Ruby avro #{environment.fetch(:avro_version)}"
    puts "AvroTurf #{environment.fetch(:avro_turf_version)}" if environment.key?(:avro_turf_version)
    puts report.fetch(:methodology)
    puts
    puts "| Payload | Operation | Engine | Bytes | p50 µs | p95 µs | Ops/s | Ruby allocs/op |"
    puts "| --- | --- | --- | ---: | ---: | ---: | ---: | ---: |"
    report.fetch(:results).each do |row|
      puts format("| %<case>s | %<operation>s | %<engine>s | %<bytes>d | %<latency_p50_us>.2f | " \
                  "%<latency_p95_us>.2f | %<throughput_per_second>.0f | %<ruby_allocations_per_operation>.2f |", row)
    end
  end
end

if $PROGRAM_NAME == __FILE__
  options = AvrocadabraBenchmark.options(ARGV)
  report = AvrocadabraBenchmark::Runner.new(options).run
  AvrocadabraBenchmark.print_report(report)
  File.write(options[:json], "#{JSON.pretty_generate(report)}\n") if options[:json]
end
