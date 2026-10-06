# frozen_string_literal: true

SimpleCov.configure do
  enable_coverage :branch
  cover "lib/**/*.rb"
  skip "lib/avrocadabra/version.rb"
end
