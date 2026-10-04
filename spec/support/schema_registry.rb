# frozen_string_literal: true

require "support/avro_turf_fixture"

RSpec.shared_context "with a schema registry" do
  attr_reader :registry_url

  let(:upstream) { AvroTurf::ConfluentSchemaRegistry.new(registry_url, logger: Logger.new(nil)) }
  let(:registry) { AvroTurf::CachedConfluentSchemaRegistry.new(upstream) }

  around do |example|
    AvroTurfFixture.with_registry do |server|
      @registry_url = AvroTurfFixture.registry_url(server)
      example.run
    end
  end
end
