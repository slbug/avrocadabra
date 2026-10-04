# frozen_string_literal: true

require "bundler/gem_tasks"
require "rb_sys/extensiontask"
require "rspec/core/rake_task"
require "rubocop/rake_task"
require "rubygems/package"
require "zeitwerk"

build_loader = Zeitwerk::Loader.new
build_loader.push_dir(File.join(__dir__, "script/build"))
build_loader.setup

RSpec::Core::RakeTask.new(:spec)
RuboCop::RakeTask.new

GEMSPEC = Gem::Specification.load("avrocadabra.gemspec")
NATIVE_TARGETS = NativeBuild::TARGETS.keys.freeze

Rake::Task[:build].enhance(["build:native"])

namespace :build do
  desc "Build the source gem"
  task source: "package:licenses_check" do
    Bundler::GemHelper.instance.build_gem
  end

  desc "Cross compile all native gems, or one target: build:native[x86_64-linux]"
  task :native, [:target] => "package:licenses_check" do |_, args|
    targets = args[:target] ? [args[:target]] : NATIVE_TARGETS
    abort "Unknown target: #{args[:target]}" unless (targets - NATIVE_TARGETS).empty?
    ruby "script/package_all.rb", *targets
  end
end

desc "Publish release gems: publish[coop|rubygems|all]; DRY_RUN=1 lists commands"
task :publish, [:target] do |_, args|
  target = args[:target] || "all"
  abort "Unknown publish target: #{target}" unless %w[coop rubygems all].include?(target)
  platforms = NATIVE_TARGETS.map { it.end_with?("-linux") ? "#{it}-gnu" : it }
  ruby "script/package_publish.rb", target, *platforms
end

%w[install install:local build:checksum].each do |name|
  Rake::Task[name].clear_prerequisites.enhance(["build:source"])
end

native_specs = []
RbSys::ExtensionTask.new("avrocadabra", GEMSPEC) do |ext|
  ext.lib_dir = "lib/avrocadabra"
  ext.cross_compiling do |spec|
    spec.required_ruby_version = "~> #{RUBY_VERSION.split(".").first(2).join(".")}.0"
    spec.files.reject! { it.start_with?("ext/") }
    if spec.platform.os == "linux" && spec.platform.version.nil?
      spec.platform = Gem::Platform.new([spec.platform.cpu, "linux", "gnu"])
    end
    spec.original_platform = spec.platform.to_s
    native_specs << spec
  end
end

Rake::Task[:spec].enhance([:compile])

desc "Mutation test all Ruby sources against all specs (BUNDLE_WITH=mutation)"
task mutation: :compile do
  unless Gem.loaded_specs.key?("mutineer")
    abort "Enable the mutation group with BUNDLE_WITH=mutation bundle install, " \
          "then BUNDLE_WITH=mutation bundle exec rake mutation"
  end

  tests = Dir["spec/**/*_spec.rb"].flat_map { ["--test", it] }
  ruby "-Ilib", Gem.bin_path("mutineer", "mutineer"), "run", "lib", *tests,
       "--format", "json", "--output", "tmp/mutation.json"
end

namespace :rust do
  desc "Test Rust code"
  task(:test) { sh "cargo test --workspace --locked" }

  desc "Check Rust formatting"
  task(:fmt) { sh "cargo fmt --all --check" }

  desc "Lint Rust code"
  task(:clippy) { sh "cargo clippy --workspace --all-targets --locked -- -D warnings" }
end

namespace :package do
  desc "Refresh bundled dependency license notices from Cargo.lock"
  task(:licenses) { ruby "script/package_licenses.rb" }

  desc "Check bundled dependency license notices against Cargo.lock"
  task(:licenses_check) { ruby "script/package_licenses.rb", "--check" }

  desc "Build the source gem and the current platform's native gem"
  task build: ["build:source", :native] do
    native_specs.each do |spec|
      directory = "pkg/#{spec.full_name}"
      Rake::Task[directory].invoke
      abi = Gem::ContentAddress.ruby_abi_for(spec.required_ruby_version)
      name = Dir.chdir(directory) { Gem::Package.build(spec, false, false, nil, abi) }
      FileUtils.cp(File.join(directory, name), File.join("pkg", name))
    end
  end
end

desc "Install built gems and verify them outside the checkout"
task "package:verify" do
  source = "pkg/#{GEMSPEC.name}-#{GEMSPEC.version}.gem"
  abort "Missing #{source}; run bundle exec rake build:source" unless File.file?(source)
  abi = RUBY_VERSION.split(".").first(2).join(".")
  candidates = Dir["pkg/#{GEMSPEC.name}-#{GEMSPEC.version}-*.gem"].select do |path|
    package = Gem::Package.new(path)
    spec = package.spec
    Gem::ContentAddress.ruby_abi_for(spec.required_ruby_version) == abi &&
      Gem::Platform.match_spec?(spec) && package.content_address
  end
  native = candidates.max_by { File.mtime(it) }
  abort "Missing current-platform native gem; run bundle exec rake package:build" unless native
  ruby "script/package_verify.rb", source, native
end

task default: %i[spec rubocop rust:test rust:fmt rust:clippy package:licenses_check]
