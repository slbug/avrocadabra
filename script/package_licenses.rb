#!/usr/bin/env ruby
# frozen_string_literal: true

require "digest"
require "json"
require "open3"

class PackageLicenses
  LICENSE_NAME = /\A(?:licen[cs]es?|copying|notice|unlicense|authors|copyright)(?:[._-].*)?\z/i

  def initialize(metadata, rust_version)
    @packages = metadata.fetch("packages").select { it["source"] }.sort_by { [it.fetch("name"), it.fetch("version")] }
    @rust_version = rust_version
    @texts = {}
  end

  def render
    rows = @packages.map { package_row(it) }
    runtime_rows = Dir["docs/licenses/rust-#{@rust_version}/*", "docs/licenses/{mingw-w64,gcc}-*/*"]
                   .select { File.file?(it) }.sort.map do |path|
      "- [#{path.delete_prefix("docs/licenses/")}](#{path.delete_prefix("docs/")})"
    end
    <<~MARKDOWN + @texts.sort.map { text_section(*it) }.join("\n")
      # Third-party notices

      Regenerate after dependency changes: `bundle exec rake package:licenses`. Duplicate texts appear once.

      Cargo.lock SHA-256: `#{Digest::SHA256.file("Cargo.lock").hexdigest}`.

      Includes build and target-specific crates. Ruby gems ship their own licenses.

      `quad-rand` supplies no license file; its supplemental notice includes the declared MIT terms and author metadata.

      | Crate and version | Declared license | Distributed license and notice files |
      | --- | --- | --- |
      #{rows.join("\n")}

      ## Compiler runtime

      Rust #{@rust_version}: standard library, backtrace, SIMD/intrinsics, compiler builtins, libm and libunwind. Refresh notices when upgrading Rust. System libraries are dynamically linked.

      Windows binaries also include MinGW-w64 CRT and GCC runtime code.

      #{runtime_rows.join("\n")}

      Source: [Rust #{@rust_version}](https://github.com/rust-lang/rust/tree/#{@rust_version}).

      ## Crate license texts

    MARKDOWN
  end

  private

  def package_row(package)
    root = File.dirname(package.fetch("manifest_path"))
    files = license_files(root, package["license_file"])
    files += Dir["docs/licenses/crates/#{package.fetch("name")}-#{package.fetch("version")}/*.txt"]
    raise "No license text packaged for #{package.fetch("name")}" if files.empty?

    links = files.map { "[#{it.delete_prefix("#{root}/")}](#license-#{record_text(it)})" }
    name = package.fetch("name")
    version = package.fetch("version")
    license = package["license"] || "See license text"
    "| [`#{name} #{version}`](https://crates.io/crates/#{name}/#{version}) | #{license} | #{links.join(", ")} |"
  end

  def license_files(root, declared)
    paths = Dir.children(root).grep(LICENSE_NAME).flat_map do |name|
      path = File.join(root, name)
      File.directory?(path) ? Dir["#{path}/**/*"] : path
    end
    paths << File.expand_path(declared, root) if declared
    paths.select { File.file?(it) }.uniq.sort
  end

  def record_text(path)
    text = File.read(path, encoding: "UTF-8").gsub("\r\n", "\n").gsub(/[ \t]+$/, "").strip
    raise "Invalid UTF-8 license: #{path}" unless text.valid_encoding?

    digest = Digest::SHA256.hexdigest(text)
    @texts[digest] ||= text
    digest
  end

  def text_section(digest, text)
    <<~MARKDOWN
      ### License #{digest}

      ````text
      #{text}
      ````
    MARKDOWN
  end
end

abort "Usage: ruby script/package_licenses.rb [--check]" unless (ARGV - ["--check"]).empty?

Dir.chdir(File.expand_path("..", __dir__)) do
  output, error, status = Open3.capture3("cargo", "metadata", "--locked", "--format-version", "1")
  abort error unless status.success?

  rust_version = File.read("rust-toolchain.toml")[/channel\s*=\s*"([^"]+)"/, 1]
  document = PackageLicenses.new(JSON.parse(output.force_encoding(Encoding::UTF_8)), rust_version).render
  destination = "docs/third_party.md"
  if ARGV.include?("--check")
    unless File.read(destination, encoding: "UTF-8") == document
      abort "#{destination} is stale; run ruby script/package_licenses.rb"
    end
  else
    File.write(destination, document, encoding: "UTF-8")
  end
end
