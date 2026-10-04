#!/usr/bin/env ruby
# frozen_string_literal: true

require "bundler"
require "rubygems/package"
require "tmpdir"

abort "Usage: ruby script/package_verify.rb pkg/avrocadabra-*.gem" if ARGV.empty?

smoke_test = <<~'RUBY'
  require "rubygems"
  expected_version, expected_platform = ARGV
  spec = Gem::Specification.find_all_by_name("avrocadabra", "= #{expected_version}").find do |candidate|
    candidate.platform.to_s == expected_platform
  end
  abort "Installed package not found" unless spec
  spec.activate
  require "avrocadabra"
  schema = Avrocadabra::Schema.new({
    type: "record", name: "Smoke", fields: [
      { name: "id", type: "long" },
      { name: "active", type: "boolean" },
      { name: "label", type: "string" },
      { name: "payload", type: "bytes" },
      { name: "decimal", type: { type: "bytes", logicalType: "big-decimal" } },
      { name: "single", type: "float" },
      { name: "double", type: "double" }
    ]
  })
  abort "Prepared schema is not shareable" unless Ractor.shareable?(schema)
  value = Ractor.make_shareable({
    "id" => 42, "active" => false, "label" => "hello ☃", "payload" => "\x00\xff".b,
    "decimal" => BigDecimal("-12345678901234567890.01234567890123456789"),
    "single" => BigDecimal("1.25"), "double" => BigDecimal("-0.125")
  })
  bytes = schema.encode(value)
  abort "Encoding is not binary" unless bytes.encoding == Encoding::BINARY
  abort "Installed package round trip failed" unless schema.decode(bytes) == value
  [false, true].each do |release_gvl|
    ready = Ractor::Port.new
    workers = Array.new(2) do
      Ractor.new(schema, value, ready, release_gvl) do |prepared, datum, ready_port, release|
        start = Ractor::Port.new
        ready_port.send(start)
        start.receive
        5.times do
          encoded = prepared.encode(datum, release_gvl: release)
          decoded = prepared.decode(encoded, release_gvl: release)
          raise "Ractor round trip failed" unless decoded == datum
          raise "Decimal type changed" unless decoded.fetch("decimal").instance_of?(BigDecimal)
          raise "Float type changed" unless %w[single double].all? { decoded.fetch(it).instance_of?(Float) }
        end
        prepared.object_id
      end
    end
    starts = Array.new(2) { ready.receive }
    starts.each { it.send(:start) }
    abort "Prepared schema was copied" unless workers.map(&:value) == [schema.object_id] * 2
  end
  stream = StringIO.new(bytes * 2)
  2.times { abort "Stream decoding failed" unless schema.decode(stream) == value }
  abort "Stream position is incorrect" unless stream.eof?
  abort "Prefix decoding failed" unless schema.decode(bytes + "\x00") == value
  loaded = $LOADED_FEATURES.find { it.end_with?("/avrocadabra.rb") }
  abort "Loaded checkout instead of installed gem" unless loaded&.start_with?(spec.full_gem_path + "/")
  extension = $LOADED_FEATURES.find { it.match?(%r{/avrocadabra\.(?:bundle|so|dll)\z}) }
  directories = [spec.full_gem_path, spec.extension_dir]
  unless extension && directories.any? { extension.start_with?(it + "/") }
    abort "Loaded a native extension outside the selected installed gem"
  end
  abort "Ruby reference gem leaked into runtime" if Gem.loaded_specs.key?("avro")
  puts "Verified #{spec.full_name} from #{extension} (Ruby #{RUBY_VERSION})"
RUBY

Bundler.with_unbundled_env do
  ARGV.each do |argument|
    package = File.expand_path(argument)
    spec = Gem::Package.new(package).spec
    raise "Unexpected package: #{spec.name}" unless spec.name == "avrocadabra"

    unless spec.platform == Gem::Platform::RUBY
      raise "Native package declares a build step" unless spec.extensions.empty?
      raise "Native package depends on rb_sys" if spec.dependencies.any? { it.name == "rb_sys" }
      raise "Native package contains Rust sources" if spec.files.any? { it.end_with?(".rs") }
    end

    Dir.mktmpdir("avrocadabra-package-") do |directory|
      platform = spec.platform == Gem::Platform::RUBY ? ["--platform", "ruby"] : []
      system(Gem.ruby, "-S", "gem", "install", package, "--no-document", *platform, chdir: directory, exception: true)
      system(Gem.ruby, "-e", smoke_test, spec.version.to_s, spec.platform.to_s, chdir: directory, exception: true)
    end
  end
end
