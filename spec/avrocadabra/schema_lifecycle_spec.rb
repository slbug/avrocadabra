# frozen_string_literal: true

RSpec.describe Avrocadabra::Schema do
  let(:definition) do
    record_schema("Message",
                  [field("id", "long"), field("text", "string"),
                   field("numbers", { "type" => "array", "items" => "int" })])
  end
  let(:datum) { { "id" => 2**40, "text" => "日本語", "numbers" => [-1, 0, 1] } }

  ractor_checks = %w[local shared logical resolution errors compaction teardown]

  it "resolves its native wrapper class before workers construct their first schema" do
    output, errors, status = ruby_subprocess(<<~RUBY)
      module WorkerClassLookup
        def const_get(name, *)
          raise "class lookup from a worker" if name == "Avrocadabra::NativeSchema" && !Ractor.main?

          super
        end
      end
      Object.singleton_class.prepend(WorkerClassLookup)
      puts Ractor.new { Avrocadabra::Schema.new('"long"').encode(42).unpack1("H*") }.value
    RUBY
    expect(status.success?).to be(true), errors
    expect(output.strip).to eq("54")
  end

  [false, true].each do |release_gvl|
    it "supports concurrent calls on one prepared schema with release_gvl=#{release_gvl}" do
      schema = native_schema(definition)
      expected = datum
      threads = Array.new(6) do
        Thread.new do
          50.times do
            bytes = schema.encode(expected, release_gvl: release_gvl)
            decoded = schema.decode(bytes, release_gvl: release_gvl)
            raise "shared schema produced corrupt data" unless decoded == expected
          end
          true
        end
      end
      expect(threads.map(&:value)).to all(be(true))
    end

    it "keeps conversion errors inside Ruby with release_gvl=#{release_gvl}" do
      schema = native_schema(definition)
      expect { schema.encode({ "id" => "bad", "text" => "ok", "numbers" => [] }, release_gvl: release_gvl) }
        .to raise_error(Avrocadabra::EncodeError)
      expect { schema.decode("\x80".b, release_gvl: release_gvl) }.to raise_error(Avrocadabra::DecodeError)
    end

    it "supports interleaved fibers sharing a prepared schema with release_gvl=#{release_gvl}" do
      schema = native_schema(definition)
      fibers = Array.new(3) do
        Fiber.new do
          10.times do
            bytes = schema.encode(datum, release_gvl: release_gvl)
            Fiber.yield(bytes)
            Fiber.yield(schema.decode(bytes, release_gvl: release_gvl))
          end
        end
      end
      10.times do
        encoded = fibers.map(&:resume)
        expect(encoded.map { schema.decode(it) }).to all(eq(datum))
        expect(fibers.map(&:resume)).to all(eq(datum))
      end
    end

    ractor_checks.each do |check|
      it "supports Ractor #{check} with release_gvl=#{release_gvl}" do
        output, errors, status = ruby_fixture("ractor_schemas.rb", check, release_gvl.to_s)
        expect(status.success?).to be(true), errors
        expect(output.strip).to eq("ok")
      end
    end

    it "can fork while Ractors resolve shared schemas with release_gvl=#{release_gvl}" do
      skip "fork is unavailable on this Ruby platform" unless Process.respond_to?(:fork)

      output, errors, status = ruby_fixture("ractor_fork.rb", release_gvl.to_s)
      expect(status.success?).to be(true), errors
      expect(output.strip).to eq("ok")
    end
  end

  it "survives thread interruption during native work" do
    output, errors, status = ruby_fixture("thread_interruption.rb")
    expect(status.success?).to be(true), errors
    expect(output.strip).to eq("ok")
  end

  it "can fork while another Ruby thread is using the prepared schema" do
    skip "fork is unavailable on this Ruby platform" unless Process.respond_to?(:fork)

    output, errors, status = ruby_fixture("concurrent_fork.rb")
    expect(status.success?).to be(true), errors
    expect(output.strip).to eq("ok")
  end

  it "survives Ruby GC and compaction with prepared writer and reader schemas" do
    schema = native_schema(definition)
    reader = native_schema(record_schema("Message", [field("text", "string"), field("id", "long")]))
    bytes = schema.encode(datum)
    GC.start
    GC.compact
    GC.verify_compaction_references(double_heap: true, toward: :empty) if GC.respond_to?(:verify_compaction_references)
    expect(schema.decode(bytes)).to eq(datum)
    expect(schema.decode(bytes, reader_schema: reader)).to eq("text" => "日本語", "id" => 2**40)
    expect(schema.encode(datum)).to eq(bytes)
  end

  it "survives repeated construction and collection" do
    150.times do |index|
      schema = native_schema(definition)
      expect(schema.decode(schema.encode(datum))).to eq(datum)
      GC.start if (index % 25).zero?
    end
  end

  it "can use a schema prepared before fork in both parent and child" do
    skip "fork is unavailable on this Ruby platform" unless Process.respond_to?(:fork)

    schema = native_schema(definition)
    bytes = schema.encode(datum)
    child_value, status = fork_result do
      schema.decode(schema.encode(datum, release_gvl: true), release_gvl: true)
    end
    expect(status).to be_success
    expect(child_value).to eq(datum)
    expect(schema.decode(bytes)).to eq(datum)
  end
end
