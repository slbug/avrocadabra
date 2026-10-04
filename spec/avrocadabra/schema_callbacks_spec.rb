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

  it "bounds iterators that yield more values than their declared size" do
    value = [12]
    value.define_singleton_method(:each) { |&block| 100.times { block.call(12) } }
    limited = native_schema({ "type" => "array", "items" => "long" }, max_items: 10)
    expect { limited.encode(value) }.to raise_error(Avrocadabra::EncodeError, /maximum item count/)
  end
end
