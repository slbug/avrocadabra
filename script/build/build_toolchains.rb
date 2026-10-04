# frozen_string_literal: true

class BuildToolchains
  attr_reader :ruby, :zig, :zigbuild, :mingw, :versions

  def initialize(targets)
    @errors = []
    @versions = { "ruby" => RUBY_VERSION }
    tool("make", "xcode-select --install")
    tool("clang", "xcode-select --install")
    tool("rustc", "rustup toolchain install #{File.read("rust-toolchain.toml")[/channel\s*=\s*"([^"]+)"/, 1]}")
    if targets.any? { it.include?("linux") }
      @zig = tool("zig", "brew install zig", "version")
      @zigbuild = tool("cargo-zigbuild", "brew install cargo-zigbuild")
    end
    @mingw = tool("x86_64-w64-mingw32-gcc", "brew install mingw-w64") if targets.include?("x64-mingw-ucrt")
    check_ucrt if @mingw
    check_rust_targets(targets)
    check_ruby_source
    abort "Missing build prerequisites:\n#{@errors.join("\n")}" unless @errors.empty?
  end

  private

  def tool(name, install, *arguments)
    path = ENV.fetch("PATH").split(File::PATH_SEPARATOR).map { File.join(it, name) }.find { File.executable?(it) }
    unless path
      @errors << "#{name}: #{install}"
      return
    end
    output, status = Open3.capture2e({ "RUSTUP_AUTO_INSTALL" => "0" }, path,
                                     *(arguments.empty? ? ["--version"] : arguments))
    unless status.success?
      @errors << "#{name}: #{output.strip}; #{install}"
      return
    end
    @versions[name] = output.lines.first.strip
    path
  end

  def check_rust_targets(targets)
    rustup = tool("rustup", "brew install rustup")
    return unless rustup

    output, status = Open3.capture2e({ "RUSTUP_AUTO_INSTALL" => "0" }, rustup, "target", "list", "--installed")
    unless status.success?
      @errors << output.strip
      return
    end
    missing = targets.map { NativeBuild::TARGETS.fetch(it) } - output.lines.map(&:strip)
    @errors << "Rust targets: rustup target add #{missing.join(" ")}" unless missing.empty?
  end

  def check_ruby_source
    source = ENV.fetch("RUBY_SOURCE", nil)
    source ||= File.join(ENV["rvm_path"], "src", "ruby-#{RUBY_VERSION}") if ENV["rvm_path"]
    @ruby = File.expand_path(source) if source
    return if @ruby && File.file?(File.join(@ruby, "configure")) && File.file?(File.join(@ruby, "version.h"))

    @errors << "Ruby source: set RUBY_SOURCE to an existing Ruby #{RUBY_VERSION} source tree"
  end

  def check_ucrt
    output, status = Open3.capture2(@mingw, "-dM", "-E", "-include", "_mingw.h", "-x", "c", File::NULL)
    return if status.success? && output.match?(/^#define _UCRT\b/)

    @errors << "MinGW must target UCRT: brew upgrade mingw-w64"
  end
end
