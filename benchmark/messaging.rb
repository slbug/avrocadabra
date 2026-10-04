# frozen_string_literal: true

require_relative "codec"
require_relative "../spec/support/avro_turf_fixture"

module AvrocadabraMessagingBenchmark
  class Runner < AvrocadabraBenchmark::Runner
    def run
      AvroTurfFixture.with_registry { measure_clients(it) }
    end

    private

    def measure_clients(server)
      rows = []
      factories, schema_id = clients_and_schema_id(server)
      reference = factories.fetch("ruby-avroturf").call
      sizes = { "small" => 3, "large" => 500 }
      factories.each do |engine, factory|
        client = engine == "ruby-avroturf" ? reference : factory.call
        sizes.each do |name, size|
          next unless ["all", name].include?(@options[:case])

          datum = AvrocadabraBenchmark.payload(items: size)
          bytes = reference.encode(datum, schema_id: schema_id)
          verify_clients({ "ruby-avroturf" => reference, engine => client }, datum, bytes, schema_id)
          operations(client, datum, bytes, schema_id).each do |operation, callable|
            rows << measure(callable).merge(case: name, engine: engine, operation: operation,
                                            bytes: bytes.bytesize, items: size)
          end
        end
      end
      { environment: environment, options: @options, results: rows,
        methodology: "Full Messaging calls; warm caches; stock measured before integration loads; " \
                     "local HTTP registry outside timed loops; GC on; latency includes clock overhead; " \
                     "batch throughput; Ruby allocations exclude native memory; consumer processing unmeasured." }
    end

    def environment
      super.merge(avro_turf_version: Gem.loaded_specs.fetch("avro_turf").version.to_s)
    end

    def clients_and_schema_id(server)
      schema = AvrocadabraBenchmark::SCHEMA
      evolved = schema.merge("fields" => schema.fetch("fields") +
                                        [{ "name" => "revision", "type" => "long", "default" => 1 }])
      store = AvroTurfFixture.schema_store(evolved)
      upstream = AvroTurf::ConfluentSchemaRegistry.new(AvroTurfFixture.registry_url(server), logger: Logger.new(nil))
      registry = AvroTurf::CachedConfluentSchemaRegistry.new(upstream)
      id = registry.register("batches", Avro::Schema.parse(JSON.generate(schema)))
      options = { registry: registry, schema_store: store, namespace: "benchmark", logger: Logger.new(nil) }
      clients = { "ruby-avroturf" => -> { AvroTurf::Messaging.new(**options) },
                  "native GVL held" => -> { Avrocadabra::AvroTurf::Messaging.new(**options) } }
      [clients, id]
    end

    def verify_clients(clients, datum, bytes, schema_id)
      clients.each_value do |client|
        raise "decode mismatch" unless client.decode(bytes) == datum
        unless client.decode(bytes, schema_name: "Batch") == datum.merge("revision" => 1)
          raise "reader resolution mismatch"
        end
        unless clients.fetch("ruby-avroturf").decode(client.encode(datum, schema_id: schema_id)) == datum
          raise "Ruby AvroTurf decode mismatch"
        end
      end
    end

    def operations(client, datum, bytes, schema_id)
      [["encode", -> { client.encode(datum, schema_id: schema_id) }],
       ["decode", -> { client.decode(bytes) }],
       ["decode resolved", -> { client.decode(bytes, schema_name: "Batch") }]]
    end
  end
end

if $PROGRAM_NAME == __FILE__
  options = AvrocadabraBenchmark.options(ARGV)
  report = AvrocadabraMessagingBenchmark::Runner.new(options).run
  AvrocadabraBenchmark.print_report(report)
  File.write(options[:json], "#{JSON.pretty_generate(report)}\n") if options[:json]
end
