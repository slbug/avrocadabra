# frozen_string_literal: true

require "json"
require "bigdecimal"
require "bigdecimal/util"
require "date"
require "stringio"
require "zeitwerk"

loader = Zeitwerk::Loader.for_gem
loader.inflector.inflect("avro_turf" => "AvroTurf")
loader.do_not_eager_load("#{__dir__}/avrocadabra/avro_turf.rb", "#{__dir__}/avrocadabra/avro_turf")
loader.setup

module Avrocadabra
  class Error < StandardError; end
  class SchemaError < Error; end
  class EncodeError < Error; end
  class DecodeError < Error; end
  class ResolutionError < DecodeError; end

  Union = Data.define(:branch, :value)
  Duration = Data.define(:months, :days, :milliseconds)
end

begin
  require "avrocadabra/#{RUBY_VERSION.split(".").first(2).join(".")}/avrocadabra"
rescue LoadError => e
  raise unless e.path == "avrocadabra/#{RUBY_VERSION.split(".").first(2).join(".")}/avrocadabra"

  require "avrocadabra/avrocadabra"
end

loader.eager_load
