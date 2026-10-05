# frozen_string_literal: true

require "simplecov"

SimpleCov.command_name "fixture #{Process.pid}"
SimpleCov.formatter = Class.new { def format(_result) = nil }
SimpleCov.start
