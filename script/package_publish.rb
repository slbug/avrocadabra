#!/usr/bin/env ruby
# frozen_string_literal: true

require "bundler"
require "rubygems/package"

target = ARGV.shift
abort "Unknown publish target: #{target}" unless %w[coop rubygems all].include?(target)
abort "Usage: ruby script/package_publish.rb TARGET PLATFORM..." if ARGV.empty?

root = File.expand_path("..", __dir__)
release = Gem::Specification.load(File.join(root, "avrocadabra.gemspec"))
abi = RUBY_VERSION.split(".").first(2).join(".")
native = Dir[File.join(root, "pkg", "#{release.name}-#{release.version}-*.gem")]
         .sort_by { File.mtime(it) }.filter_map do |path|
  package = Gem::Package.new(path)
  spec = package.spec
  next unless Gem::ContentAddress.ruby_abi_for(spec.required_ruby_version) == abi && package.content_address

  [spec.platform.to_s, path]
end.to_h
dry_run = ENV["DRY_RUN"] == "1"
platforms = ["ruby", *ARGV]
packages = platforms.map do |platform|
  path = if platform == "ruby"
           File.join(root, "pkg", release.file_name)
         else
           native.fetch(platform) { abort "Missing #{platform}; run bundle exec rake build" }
         end
  abort "Missing #{path}; run bundle exec rake build" unless File.file?(path)
  package = Gem::Package.new(path)
  spec = package.spec
  unless spec.name == release.name && spec.version == release.version && spec.platform.to_s == platform
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
