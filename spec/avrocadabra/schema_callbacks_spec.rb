# frozen_string_literal: true

RSpec.describe Avrocadabra::Schema do
  let(:schema) { native_schema({ "type" => "array", "items" => "long" }) }

  it "invalidates captured iteration blocks after encoding" do
    saved = nil
    value = [12]
    value.define_singleton_method(:each) do |&block|
      saved = block
      block.call(12)
    end
    expect(schema.decode(schema.encode(value))).to eq([12])
    GC.verify_compaction_references(double_heap: true, toward: :empty)
    expect { saved.call(34) }.to raise_error(LocalJumpError, "inactive encoding block")
  end

  it "invalidates captured blocks when an iterator raises" do
    saved = nil
    value = [12]
    value.define_singleton_method(:each) do |&block|
      saved = block
      raise IOError, "iteration failed"
    end
    expect { schema.encode(value) }.to raise_error(IOError, "iteration failed")
    expect { saved.call(34) }.to raise_error(LocalJumpError, "inactive encoding block")
  end

  it "rejects asynchronous iteration callbacks on another fiber" do
    value = [12]
    value.define_singleton_method(:each) { |&block| Fiber.new { block.call(12) }.resume }
    expect { schema.encode(value) }.to raise_error(ThreadError, /another thread or fiber/)
  end

  it "frames the values an iterator yields instead of its declared size" do
    fewer = [12, 34]
    fewer.define_singleton_method(:each) { |&block| block.call(12) }
    more = [12]
    more.define_singleton_method(:each) { |&block| 3.times { block.call(12) } }
    silent = [12]
    silent.define_singleton_method(:each) { nil }
    expect(schema.decode(schema.encode(fewer))).to eq([12])
    expect(schema.decode(schema.encode(more))).to eq([12, 12, 12])
    expect(schema.encode(silent)).to eq("\x00".b)
  end

  it "frames elements that callbacks add or remove during array encoding" do
    strings = native_schema({ "type" => "array", "items" => "string" })
    items = []
    shrinking = Class.new(String) { define_method(:encode) { |*args| items.pop && super(*args) } }
    items.push(shrinking.new("a"), "b", "c")
    expect(strings.decode(strings.encode(items))).to eq(%w[a b])
    growing = Class.new(String) { define_method(:encode) { |*args| items.push("z") && super(*args) } }
    items.replace([growing.new("a"), "b"])
    expect(strings.decode(strings.encode(items))).to eq(%w[a b z])
  end

  it "dispatches core collection and text methods redefined after load" do
    output, errors, status = ruby_fixture("redefined_core.rb")
    expect(status.success?).to be(true), errors
    expect(JSON.parse(output)).to eq("id" => 7, "items" => [10], "index" => { "changed" => 20 }, "label" => "changed")
  end

  it "bounds iterators that yield more values than their declared size" do
    value = [12]
    value.define_singleton_method(:each) { |&block| 100.times { block.call(12) } }
    limited = native_schema({ "type" => "array", "items" => "long" }, max_items: 10)
    expect { limited.encode(value) }.to raise_error(Avrocadabra::EncodeError, /maximum item count/)
  end
end
