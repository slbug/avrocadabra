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

Rake::Task[:build].clear

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

desc "Build the source gem and all seven native gems locally on macOS"
task build: %w[build:source build:native]

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

RbSys::ExtensionTask.new("avrocadabra", GEMSPEC) do |ext|
  ext.lib_dir = "lib/avrocadabra"
  ext.cross_compiling do |spec|
    spec.required_ruby_version = "~> 4.0.0"
    spec.required_rubygems_version = ">= 3.3.22"
    spec.files.reject! { it.start_with?("ext/") }
    if spec.platform.os == "linux" && spec.platform.version.nil?
      spec.platform = Gem::Platform.new([spec.platform.cpu, "linux", "gnu"])
    end
    spec.original_platform = spec.platform.to_s
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
  task :build do
    Rake::Task["package:licenses_check"].invoke
    Rake::Task["build:source"].invoke
    Rake::Task["native"].invoke
    Rake::Task["gem"].invoke
  end

  desc "Install built gems and verify them outside the checkout"
  task :verify do
    source = "pkg/#{GEMSPEC.name}-#{GEMSPEC.version}.gem"
    abort "Missing #{source}; run bundle exec rake build:source" unless File.file?(source)
    candidates = Dir["pkg/#{GEMSPEC.name}-*.gem"].select do |package|
      spec = Gem::Package.new(package).spec
      spec.name == GEMSPEC.name && spec.version == GEMSPEC.version &&
        spec.platform != Gem::Platform::RUBY && Gem::Platform.match_spec?(spec)
    end
    native = candidates.max_by { File.mtime(it) }
    ruby "script/package_verify.rb", *[source, native].compact
  end
end

task default: %i[spec rubocop rust:test rust:fmt rust:clippy package:licenses_check]
