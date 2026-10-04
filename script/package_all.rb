#!/usr/bin/env ruby
# frozen_string_literal: true

require "digest"
require "etc"
require "fileutils"
require "json"
require "open3"
require "rbconfig"
require "rubygems/package"
require "shellwords"
require "tmpdir"
require "zeitwerk"

loader = Zeitwerk::Loader.new
loader.push_dir(File.join(__dir__, "build"))
loader.setup

abort "Usage: ruby script/package_all.rb TARGET..." if ARGV.empty?
abort "All-target builds require macOS with Xcode Command Line Tools" unless RUBY_PLATFORM.include?("darwin")
unless (ARGV - NativeBuild::TARGETS.keys).empty?
  abort "Unknown target: #{(ARGV - NativeBuild::TARGETS.keys).join(", ")}"
end

root = File.expand_path("..", __dir__)
tools = BuildToolchains.new(ARGV)
puts "Toolchains: #{tools.versions.map { |name, version| "#{name} #{version}" }.join(", ")}"
FileUtils.mkdir_p(File.join(root, "tmp/native"))
File.open(File.join(root, "tmp/native/build.lock"), "a") do |lock|
  abort "A native build is already running" unless lock.flock(File::LOCK_EX | File::LOCK_NB)

  ARGV.each { NativeBuild.new(root, it, tools).build }
end
