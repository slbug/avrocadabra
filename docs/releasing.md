# Release

Manual publishing only. CI runs specs, lint, coverage, mutation and host package checks.

## Check

Update `lib/avrocadabra/version.rb`, `ext/avrocadabra/Cargo.toml`, `Cargo.lock` and `CHANGELOG.md`.

```sh
bundle install
bundle exec rake package:licenses
COVERAGE=true bundle exec rake
BUNDLE_WITH=mutation bundle install
BUNDLE_WITH=mutation bundle exec rake mutation
bundle exec ruby benchmark/codec.rb
bundle exec ruby benchmark/messaging.rb
```

Gates: 95% Ruby line/branch coverage; 100% Mutineer score. Reports: CI artifacts and Codecov. Refresh compiler notices in `docs/licenses/` when upgrading release compilers.

## Prerequisites

macOS, Xcode Command Line Tools, Ruby 4.0 and Rust 1.99. Install missing tools:

```sh
brew install zig cargo-zigbuild mingw-w64 rustup
rustup target add aarch64-apple-darwin x86_64-apple-darwin \
  x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu \
  x86_64-unknown-linux-musl aarch64-unknown-linux-musl x86_64-pc-windows-gnu
```

Ruby source must match `ruby -v`. RVM's source tree is detected automatically; otherwise set `RUBY_SOURCE` to the extracted [release source](https://www.ruby-lang.org/en/downloads/). Builds generate target headers/configuration in `tmp/native/`; the source tree stays untouched. Tasks check prerequisites; they never install them.

## Build

```sh
bundle exec rake build package:verify
bundle exec rake 'build:native[x86_64-linux]'
bundle exec rake build:source
```

`build`: source gem + seven binaries in `pkg/`, using the running Ruby release and installed compilers. Target builds are cached in `tmp/native/`. No Docker.

| Gem platform | Runtime |
| --- | --- |
| `ruby` | Source build: Rust 1.99+, C toolchain, libclang, Ruby headers |
| `x86_64-linux-gnu`, `aarch64-linux-gnu` | Linux, glibc 2.30+ |
| `x86_64-linux-musl`, `aarch64-linux-musl` | Linux, musl |
| `arm64-darwin`, `x86_64-darwin` | macOS 14+ |
| `x64-mingw-ucrt` | Windows x64 UCRT |

Binary gems need no compiler. `package:verify` installs source/current-platform gems into the active Ruby's gem directory and checks them outside the checkout. Test other binaries on their target OS/CPU before publishing.

## Publish

Requires all eight artifacts for the current version in `pkg/`.

```sh
DRY_RUN=1 bundle exec rake publish
bundle exec rake 'publish[coop]'
bundle exec rake 'publish[rubygems]'
bundle exec rake publish
```

| Target | Registry | Credentials |
| --- | --- | --- |
| `coop` | `https://gem.coop/@slbug` | `GEM_COOP_API_KEY` or `GEM_HOST_API_KEY` |
| `rubygems` | `https://rubygems.org` | `RUBYGEMS_API_KEY` or stored RubyGems credentials |
| `all` / omitted | Both | Credentials above |

RubyGems prompts for MFA when required; `GEM_HOST_OTP_CODE` is forwarded only there. [CLI MFA](https://guides.rubygems.org/using-mfa-in-command-line/).
