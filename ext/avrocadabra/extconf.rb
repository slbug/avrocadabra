# frozen_string_literal: true

require "mkmf"
require "rb_sys/mkmf"

create_rust_makefile("avrocadabra/avrocadabra") do |config|
  config.auto_install_rust_toolchain = false
  config.extra_cargo_args = ["--locked"]
end
