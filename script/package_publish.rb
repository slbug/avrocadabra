#!/usr/bin/env ruby
# frozen_string_literal: true

require "bundler"
require "rubygems/package"
require_relative "../lib/avrocadabra/version"

target = ARGV.shift
abort "Unknown publish target: #{target}" unless %w[coop rubygems all].include?(target)
abort "Usage: ruby script/package_publish.rb TARGET PLATFORM..." if ARGV.empty?

root = File.expand_path("..", __dir__)
dry_run = ENV["DRY_RUN"] == "1"
platforms = ["ruby", *ARGV]
packages = platforms.map do |platform|
  suffix = platform == "ruby" ? "" : "-#{platform}"
  path = File.join(root, "pkg", "avrocadabra-#{Avrocadabra::VERSION}#{suffix}.gem")
  abort "Missing #{path}; run bundle exec rake build" unless File.file?(path)
  spec = Gem::Package.new(path).spec
  unless spec.name == "avrocadabra" && spec.version.to_s == Avrocadabra::VERSION && spec.platform.to_s == platform
    abort "Unexpected package metadata: #{path}"
  end
  path
end

coop_key = ENV["GEM_COOP_API_KEY"] || ENV.fetch("GEM_HOST_API_KEY", nil)
abort "GEM_COOP_API_KEY or GEM_HOST_API_KEY is required" if target != "rubygems" && !dry_run && !coop_key

hosts = {}
hosts["https://gem.coop/@slbug"] = coop_key if %w[coop all].include?(target)
hosts["https://rubygems.org"] = ENV.fetch("RUBYGEMS_API_KEY", nil) if %w[rubygems all].include?(target)
hosts.each do |host, key|
  packages.each do |package|
    command = [Gem.ruby, "-S", "gem", "push", package, "--host", host]
    if dry_run
      puts command.join(" ")
    else
      Bundler.with_unbundled_env do
        environment = { "GEM_HOST_API_KEY" => key }
        environment["GEM_HOST_OTP_CODE"] = nil if host == "https://gem.coop/@slbug"
        system(environment, *command, exception: true)
      end
    end
  end
end
