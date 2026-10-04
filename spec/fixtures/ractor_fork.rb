# frozen_string_literal: true

module RactorForkCheck
  class << self
    def schemas
      writer = Avrocadabra::Schema.new(
        { "type" => "record", "name" => "Record", "fields" => [{ "name" => "value", "type" => "int" }] }
      )
      readers = Array.new(24) do |index|
        Avrocadabra::Schema.new(
          { "type" => "record", "name" => "Record", "fields" => [
            { "name" => "value", "type" => "long" }, { "name" => "tag", "type" => "int", "default" => index }
          ] }
        )
      end
      Ractor.make_shareable([writer, readers])
    end

    def resolve(writer, readers, iteration, release_gvl)
      index = iteration % readers.length
      bytes = writer.encode({ value: iteration }, release_gvl: release_gvl)
      actual = writer.decode(bytes, reader_schema: readers.fetch(index), release_gvl: release_gvl)
      expected = { "value" => iteration, "tag" => index }
      raise "corrupt resolved datum" unless actual == expected
    end

    def construction(iteration, release_gvl)
      definition = { "type" => "record", "name" => "Envelope#{iteration}",
                     "namespace" => "generated.ns#{iteration % 7}",
                     "fields" => [
                       { "name" => "state", "type" => {
                         "type" => "enum", "name" => "State", "symbols" => %w[READY WAIT]
                       } },
                       { "name" => "token", "type" => { "type" => "fixed", "name" => "Token", "size" => 3 } },
                       { "name" => "items", "type" => { "type" => "array", "items" => {
                         "type" => "record", "name" => "Item", "fields" => [{ "name" => "text", "type" => "string" }]
                       } } },
                       { "name" => "index", "type" => { "type" => "map", "values" => "Item" } },
                       { "name" => "optional", "type" => %w[null Token] }
                     ] }
      schema = Avrocadabra::Schema.new(definition)
      datum = { "state" => "READY", "token" => "\x00\xff\x80".b, "items" => [{ "text" => "日本語" }],
                "index" => { "entry" => { "text" => "value" } }, "optional" => nil }
      bytes = schema.encode(datum, release_gvl: release_gvl)
      raise "corrupt newly constructed schema" unless schema.decode(bytes, release_gvl: release_gvl) == datum
    end

    def fork_check(writer, readers, release_gvl)
      pid = Process.fork do
        24.times do |index|
          construction(index, release_gvl)
          resolve(writer, readers, index, release_gvl)
        end
        exit! 0
      rescue StandardError => e
        warn e.full_message
        exit! 1
      end
      watchdog = Thread.new do
        sleep 10
        Process.kill("KILL", pid)
      rescue Errno::ESRCH
        nil
      end
      _pid, status = Process.wait2(pid)
      raise "child process failed or inherited a locked native mutex: #{status}" unless status.success?
    ensure
      watchdog&.kill&.join
    end

    def run(release_gvl)
      writer, readers = schemas
      ready = Ractor::Port.new
      workers = Array.new(4) do
        Ractor.new(writer, readers, ready, release_gvl) do |prepared, evolved, parent, release|
          parent.send(:ready)
          150_000.times do |iteration|
            RactorForkCheck.construction(iteration, release) if (iteration % 200).zero?
            RactorForkCheck.resolve(prepared, evolved, iteration, release)
          end
          true
        end
      end
      4.times { ready.receive }
      8.times { fork_check(writer, readers, release_gvl) }
      raise "background worker failed" unless workers.map(&:value).all?
    end
  end
end

RactorForkCheck.run(ARGV.fetch(0) == "true")
puts "ok"
