# frozen_string_literal: true

RSpec.describe Avrocadabra::AvroTurf::Messaging do
  def expect_ruby_avro_output(fixture, *)
    outputs = %w[stock native].map do |engine|
      output, errors, status = ruby_fixture(fixture, engine, *)
      expect(status.success?).to be(true), errors
      output
    end
    expect(outputs.last).to eq(outputs.first)
  end

  %w[redefined_during_encode.rb redefined_adapter.rb redefined_conversion.rb].each do |fixture|
    it "matches Ruby Avro with methods redefined by #{fixture}" do
      expect_ruby_avro_output(fixture)
    end
  end

  %w[date date_time].each do |kind|
    it "matches Ruby Avro when #{kind} conversion calls a redefined Time constructor" do
      expect_ruby_avro_output("redefined_time_constructor.rb", kind)
    end
  end

  %w[integer float boolean time decimal inspect digits limited aliased undefined].each do |kind|
    it "matches Ruby Avro when a stock method behind #{kind} values is redefined" do
      expect_ruby_avro_output("redefined_stock_method.rb", kind)
    end
  end

  ["write-time validation", "method removed before load", "singleton visibility", "ancestor module method",
   "late mixin method", "returning raise", "constant swapped mid-encode", "fields reordered mid-encode",
   "union branches reordered mid-encode", "hash impostor", "plan replaced mid-encode", "field renamed inside key?",
   "decimal factor changed"].each do |kind|
    it "matches Ruby Avro after #{kind}" do
      expect_ruby_avro_output("changed_stock_world.rb", kind)
    end
  end
end
