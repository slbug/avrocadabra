# frozen_string_literal: true

class NativeBuild
  TARGETS = {
    "x86_64-linux" => "x86_64-unknown-linux-gnu", "aarch64-linux" => "aarch64-unknown-linux-gnu",
    "x86_64-linux-musl" => "x86_64-unknown-linux-musl", "aarch64-linux-musl" => "aarch64-unknown-linux-musl",
    "arm64-darwin" => "aarch64-apple-darwin", "x86_64-darwin" => "x86_64-apple-darwin",
    "x64-mingw-ucrt" => "x86_64-pc-windows-gnu"
  }.freeze

  def initialize(root, target, tools)
    @root = root
    @target = target
    @tools = tools
    @rust = TARGETS.fetch(target)
    @cache = File.join(root, "tmp/native")
    keys = %w[ruby make clang rustc]
    keys += linux? ? %w[zig cargo-zigbuild] : %w[x86_64-w64-mingw32-gcc]
    fingerprint = Digest::SHA256.hexdigest(JSON.generate([tools.ruby, tools.versions.slice(*keys)])).slice(0, 12)
    @build = File.join(root, "tmp/native", RUBY_VERSION, target, fingerprint)
    @cargo = File.join(root, "tmp/native/cargo")
    @env = { "RUBYOPT" => nil, "RUSTUP_AUTO_INSTALL" => "0", "CFLAGS" => "-O2", "LDFLAGS" => "", "LIBS" => "" }
    FileUtils.mkdir_p(@build)
    configure_compiler
  end

  def build
    puts "Building #{@target} for Ruby #{RUBY_VERSION} (#{@rust})"
    prepare_ruby
    compile
    package
  end

  private

  def windows? = @target == "x64-mingw-ucrt"
  def linux? = @target.include?("linux")

  def configure_compiler
    if windows?
      @cc = [@tools.mingw]
      @host = "x86_64-w64-mingw32"
      @env["rb_cv_msvcrt"] = "ucrt"
      @env["CFLAGS"] += " -DDTRACE_PROBES_DISABLED=1"
    elsif linux?
      target = @rust.sub("-unknown", "")
      target += ".2.30" if target.end_with?("gnu")
      @cc = [@tools.zig, "cc", "-target", target]
      @host = @rust.sub("-unknown", "")
      @env.merge!("AR" => [@tools.zig, "ar"].shelljoin, "RANLIB" => [@tools.zig, "ranlib"].shelljoin,
                  "ZIG_GLOBAL_CACHE_DIR" => File.join(@cache, "zig-cache"), "CFLAGS" => "-O2 -fPIC")
    else
      @cc = ["clang", "-arch", @target.split("-").first]
      @host = @target.sub("-darwin", "-apple-darwin")
      @env["MACOSX_DEPLOYMENT_TARGET"] = "14.0"
    end
    @env["CC"] = @cc.shelljoin
  end

  def run(*command, **)
    system(@env, *command, **, exception: true)
  end

  def prepare_ruby
    unless File.file?(File.join(@build, ".prepared"))
      File.open(File.join(@build, "configure.log"), "w") do |log|
        run(File.join(@tools.ruby, "configure"), "--build=#{RbConfig::CONFIG.fetch("host")}", "--host=#{@host}",
            "--enable-shared", "--disable-install-doc", "--disable-yjit", "--with-ext=", "--without-gmp",
            "--with-baseruby=#{RbConfig.ruby}", "--prefix=#{@build}/ruby", chdir: @build, out: log, err: log)
      end
      run("make", "rbconfig.rb", chdir: @build)
    end
    read_config
    artifacts = [".ext/include/#{@config.fetch("arch")}/ruby/config.h"]
    if windows?
      artifacts.push("PRISM_BUILD_DIR=#{@build}/prism", "TIMESTAMPDIR=#{@build}/.ext/.timestamp",
                     "--assume-new=#{@tools.ruby}/dmyext.c", "--assume-new=#{@tools.ruby}/dmyenc.c",
                     @config.fetch("LIBRUBY_SO"))
    end
    run("make", "-j#{Etc.nprocessors}", *artifacts, chdir: @build)
    FileUtils.touch(File.join(@build, ".prepared"))
    @config.merge!("rubyhdrdir" => File.join(@tools.ruby, "include"),
                   "rubyarchhdrdir" => File.join(@build, ".ext/include", @config.fetch("arch")),
                   "libdir" => @build, "CROSS_COMPILING" => "yes")
    @env.merge!(@config.transform_keys { "RBCONFIG_#{it}" })
  end

  def read_config
    output, status = Open3.capture2({ "RUBYOPT" => nil }, RbConfig.ruby, "-rjson", "-e",
                                    "Object.send(:remove_const, :RbConfig); load ARGV.fetch(0); print JSON.generate(RbConfig::CONFIG)",
                                    File.join(@build, "rbconfig.rb"))
    abort "Cannot load target Ruby configuration" unless status.success?
    @config = JSON.parse(output)
    return if @config["RUBY_PROGRAM_VERSION"] == RUBY_VERSION

    abort "Unexpected Ruby version: #{@config["RUBY_PROGRAM_VERSION"]}"
  end

  def bindgen_headers
    _, output, status = Open3.capture3(@env, *@cc, "-E", "-v", "-xc", File::NULL)
    abort "Cannot find target headers: #{output}" unless status.success?
    includes = output.split("#include <...> search starts here:").last.split("End of search list.").first
    includes = includes.lines.map(&:strip).reject { it.empty? || it.end_with?("(framework directory)") }
    resource, status = Open3.capture2("xcrun", "clang", "-print-resource-dir")
    abort "Xcode Command Line Tools are required" unless status.success?
    includes[0] = File.join(resource.strip, "include")
    flags = ["-nostdinc", *includes.flat_map { ["-isystem", File.expand_path(it)] }]
    flags << "--target=x86_64-w64-windows-gnu" if windows?
    @env["BINDGEN_EXTRA_CLANG_ARGS"] = flags.shelljoin
  end

  def compile
    bindgen_headers
    compiler_paths
    @env["CARGO_TARGET_DIR"] = @cargo
    if linux?
      @env["CARGO_ZIGBUILD_ZIG_PATH"] = @tools.zig
      @env["CARGO_ZIGBUILD_CACHE_DIR"] = File.join(@cache, "zigbuild-cache")
      target = @rust.end_with?("gnu") ? "#{@rust}.2.30" : @rust
      run(@tools.zigbuild, "zigbuild", "--release", "--locked", "--package", "avrocadabra", "--target", target,
          chdir: @root)
    else
      @env["CARGO_TARGET_#{@rust.upcase.tr("-", "_")}_LINKER"] = @cc.first
      @env["CC_#{@rust.tr("-", "_")}"] = @cc.shelljoin
      run("cargo", "build", "--release", "--locked", "--package", "avrocadabra", "--target", @rust, chdir: @root)
    end
  end

  def compiler_paths
    paths = { Dir.home => "/build", ENV.fetch("CARGO_HOME", File.join(Dir.home, ".cargo")) => "/cargo",
              @tools.ruby => "/ruby", @root => "/avrocadabra" }
    flags = paths.map { |source, target| "--remap-path-prefix=#{source}=#{target}" }
    flags += ["-C", windows? ? "link-arg=-Wl,--strip-all" : "link-arg=-Wl,-S"]
    flags += ["-C", "target-feature=-crt-static"] if @target.end_with?("musl")
    flags += ["-C", "link-arg=-Wl,-install_name,@rpath/avrocadabra.bundle"] unless linux? || windows?
    @env["CARGO_ENCODED_RUSTFLAGS"] = flags.join("\x1f")
    @env["CFLAGS"] += " #{paths.map { |source, target| "-ffile-prefix-map=#{source}=#{target}" }.shelljoin}"
  end

  def native_spec
    spec = Gem::Specification.load(File.join(@root, "avrocadabra.gemspec")).dup
    spec.platform = Gem::Platform.new(@target.end_with?("-linux") ? "#{@target}-gnu" : @target)
    spec.extensions = []
    spec.dependencies.reject! { it.name == "rb_sys" }
    spec.files.reject! { it.start_with?("ext/") || it.match?(/\ACargo\.(toml|lock)\z/) }
    spec.required_ruby_version = "~> #{RUBY_VERSION.split(".").first(2).join(".")}.0"
    spec.required_rubygems_version = ">= 3.3.22"
    spec.metadata["build_ruby_version"] = RUBY_VERSION
    spec.metadata["build_toolchains"] = JSON.generate(@tools.versions)
    spec
  end

  def package
    spec = native_spec
    directory = Dir.mktmpdir("gem-", @build)
    spec.files.each do |file|
      destination = File.join(directory, file)
      FileUtils.mkdir_p(File.dirname(destination))
      FileUtils.cp(File.join(@root, file), destination, preserve: true)
    end
    spec.files << copy_extension(directory)
    FileUtils.mkdir_p(File.join(@root, "pkg"))
    Dir.chdir(directory) { Gem::Package.build(spec, false, false, File.join(@root, "pkg", spec.file_name)) }
  end

  def copy_extension(directory)
    extension = "lib/avrocadabra/#{RUBY_VERSION.split(".").first(2).join(".")}/avrocadabra.#{@config.fetch("DLEXT")}"
    FileUtils.mkdir_p(File.dirname(File.join(directory, extension)))
    suffix = linux? ? "so" : "dylib"
    binary = windows? ? "avrocadabra.dll" : "libavrocadabra.#{suffix}"
    source = File.join(@cargo, @rust, "release", binary)
    content = File.binread(source)
    paths = [Dir.home, @root].flat_map { [it.b, it.b.gsub(/[^A-Za-z0-9]/, "_")] }
    abort "Native binary contains a build path: #{@target}" if paths.any? { content.include?(it) }

    FileUtils.cp(source, File.join(directory, extension))
    extension
  end
end
