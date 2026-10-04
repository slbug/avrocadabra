# frozen_string_literal: true

require_relative "lib/avrocadabra/version"

Gem::Specification.new do |spec|
  spec.name = "avrocadabra"
  spec.version = Avrocadabra::VERSION
  spec.authors = ["Alexander Grebennik"]
  spec.email = ["slbug@users.noreply.github.com", "sl.bug.sl@gmail.com"]
  spec.summary = "Avro binary encoding and decoding with a Rust native extension"
  spec.description = "Reusable Avro schemas, raw datum encoding and decoding, and schema resolution through Apache Avro"
  spec.homepage = "https://github.com/slbug/avrocadabra"
  spec.license = "MIT"
  spec.required_ruby_version = ">= 4.0"
  spec.metadata["homepage_uri"] = spec.homepage
  spec.metadata["source_code_uri"] = "#{spec.homepage}/tree/main"
  spec.metadata["changelog_uri"] = "#{spec.homepage}/blob/main/CHANGELOG.md"
  spec.metadata["rubygems_mfa_required"] = "true"
  spec.metadata["cargo_crate_name"] = "avrocadabra"

  spec.files = Dir[
    "lib/**/*.rb", "ext/avrocadabra/**/*.{rs,rb,toml}",
    "Cargo.{toml,lock}", "README.md", "CHANGELOG.md", "LICENSE.txt", "docs/**/*"
  ].select { File.file?(it) }
  spec.require_paths = ["lib"]
  spec.extensions = ["ext/avrocadabra/extconf.rb"]

  spec.add_dependency "bigdecimal", ">= 3.1"
  spec.add_dependency "date", ">= 3.4"
  spec.add_dependency "json", ">= 2.9"
  spec.add_dependency "rb_sys", ">= 0.9"
  spec.add_dependency "zeitwerk", ">= 2.8"
end
