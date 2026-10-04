# frozen_string_literal: true

RSpec.describe Avrocadabra do
  it "loads core code through Zeitwerk without loading AvroTurf" do
    output, errors, status = ruby_subprocess(<<~RUBY)
      Zeitwerk::Loader.eager_load_all
      puts JSON.generate(version: Avrocadabra::VERSION,
                         schema: Avrocadabra.const_source_location(:Schema).first.end_with?("avrocadabra/schema.rb"),
                         logical: Avrocadabra.autoload?(:Logical).nil?,
                         avro_turf: !!defined?(::AvroTurf))
    RUBY
    expect(status.success?).to be(true), errors
    expect(JSON.parse(output)).to eq("version" => Avrocadabra::VERSION, "schema" => true,
                                     "logical" => true, "avro_turf" => false)
  end

  it "supports the optional integration as the first require" do
    output, errors, status = Open3.capture3(RbConfig.ruby, "-I", avrocadabra_library,
                                            "-ravrocadabra/avro_turf", "-e", <<~RUBY)
                                              puts Avrocadabra::AvroTurf::Messaging.superclass.name
                                            RUBY
    expect(status.success?).to be(true), errors
    expect(output.strip).to eq("AvroTurf::Messaging")
  end

  [%w[avrocadabra avro_turf/messaging], %w[avro_turf/messaging avrocadabra]].each do |order|
    it "exposes Messaging through the normal require in order #{order.inspect}" do
      output, errors, status = Open3.capture3(RbConfig.ruby, "-I", avrocadabra_library,
                                              "-e", <<~RUBY)
                                                #{order.map { "require #{it.inspect}" }.join("\n")}
                                                class Registry
                                                  def fetch(*) = '"long"'
                                                end
                                                worker = Ractor.new do
                                                  client = Avrocadabra::AvroTurf::Messaging.new(registry: Registry.new)
                                                  client.decode(client.encode(42, schema_id: 1))
                                                end
                                                puts worker.value
                                                puts Avrocadabra::AvroTurf::Messaging.superclass.name
                                              RUBY
      expect(status.success?).to be(true), errors
      expect(output.lines.map(&:strip)).to eq(["42", "AvroTurf::Messaging"])
    end
  end
end
