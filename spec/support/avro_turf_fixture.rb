# frozen_string_literal: true

require "fileutils"
require "tmpdir"
require "json"
require "avro_turf/messaging"
require "avro_turf/test/fake_confluent_schema_registry_server"
require "rackup/handler/webrick"
require "webrick/https"

module AvroTurfFixture
  def self.with_registry(tls: false, app: FakeConfluentSchemaRegistryServer)
    FakeConfluentSchemaRegistryServer.clear
    ready = Queue.new
    server = Rackup::Handler::WEBrick::Server.new(
      Rack::RewindableInput::Middleware.new(app),
      BindAddress: "127.0.0.1", Port: 0, MaxClients: 1,
      Logger: WEBrick::Log.new(File::NULL), AccessLog: [], StartCallback: -> { ready << true },
      SSLEnable: tls, SSLCertName: [%w[CN 127.0.0.1]]
    )
    worker = Thread.new { server.start }
    ready.pop
    yield server
  ensure
    server&.shutdown
    worker&.value
  end

  def self.registry_url(server)
    "#{server[:SSLEnable] ? "https" : "http"}://127.0.0.1:#{server[:Port]}"
  end

  def self.schema_store(*definitions)
    directory = Dir.mktmpdir("avrocadabra-schemas-")
    definitions.each do |definition|
      name = [definition["namespace"], definition.fetch("name")].compact.join(".")
      path = File.join(directory, "#{name.tr(".", "/")}.avsc")
      FileUtils.mkdir_p(File.dirname(path))
      File.write(path, JSON.generate(definition))
    end
    ::AvroTurf::SchemaStore.new(path: directory)
  end
end
