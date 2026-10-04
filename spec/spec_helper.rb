# frozen_string_literal: true

if ENV["COVERAGE"] == "true"
  require "simplecov"
  require "simplecov-lcov"

  SimpleCov::Formatter::LcovFormatter.config do |config|
    config.report_with_single_file = true
    config.single_report_path = "coverage/coverage.lcov"
  end
  SimpleCov.formatters = [SimpleCov::Formatter::HTMLFormatter, SimpleCov::Formatter::LcovFormatter]
  SimpleCov.command_name "RSpec (Ruby API only)"
  SimpleCov.start do
    enable_coverage :branch
    cover "lib/**/*.rb"
    minimum_coverage line: 95, branch: 95
  end
end

require "avrocadabra"
require "avro"
require "bigdecimal"
require "date"
require "json"
require "stringio"
require "open3"
require "rbconfig"

require "support/avro_helpers"

RSpec.configure do |config|
  config.example_status_persistence_file_path = ".rspec_status"
  config.disable_monkey_patching!
  config.order = :random
  config.include AvroHelpers

  config.expect_with :rspec do |c|
    c.syntax = :expect
  end
end
