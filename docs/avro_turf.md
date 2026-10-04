# AvroTurf

Two changes:

```ruby
gem "avrocadabra"
```

```ruby
messaging = Avrocadabra::AvroTurf::Messaging.new(**existing_options)
```

Keep `avro_turf` and existing encode/decode calls. Bundler loads Avrocadabra; Zeitwerk loads the integration on constant access. Either require order works. No initializer or flags.

AvroTurf keeps framing, registry access, authentication, registration, subjects, IDs, versions and schema caches. The integration prepends datum routing, bounded validation and Ractor support. Stock `AvroTurf::Messaging` keeps Ruby codecs; standalone Avrocadabra needs no AvroTurf.

```ruby
payload = messaging.encode({ "id" => 42, "reading" => BigDecimal("12.34") }, schema_name: "Sample")
message = messaging.decode(payload)
evolved = messaging.decode_message(payload, schema_name: "CurrentSample")
evolved.message
evolved.schema_id
evolved.writer_schema
evolved.reader_schema
```

The schema ID selects the writer; decode's `schema_name:` selects the reader. Metadata retains Ruby Avro schema objects.

## Contract

- Same native reader/writer as `Avrocadabra::Schema`.
- Field lookup: string keys, then symbols. Hash defaults, default procs and `key?`/`[]` overrides apply. Missing nullable fields become `nil`; missing required fields raise, even with writer defaults.
- Records include `error` schemas and recursion. Exact field names beat aliases; later writer matches overwrite earlier ones.
- Unions select the first valid branch. Ruby Avro adapters preserve logical mappings and nested exceptions. Decimal Floats work; excess scale raises `RangeError`.
- `each` and text `encode` overrides apply; enums use the original value for symbol lookup. Iterators must yield synchronously in the calling thread/fiber; retained encoding blocks expire after return.
- Reader additions require explicit defaults. Ruby Avro materializes them, including its float, UTF-8 and nested-default quirks. Extra default keys are ignored.
- Writer-union resolution preserves repeated adapter calls. Earlier adapters run before later resolution errors.
- UUID case stays unchanged. Unsupported Ruby Avro annotations retain physical values: fixed decimals, big-decimal and duration decode as strings. Standalone schemas have [additional mappings](../README.md#types).
- Integer-to-floating promotions retain `Integer`; string/bytes promotions retain writer encoding. Maps retain insertion order.
- Decode consumes one datum, leaving trailing bytes unread. Failed native reads preserve the cursor, including adapter failures.
- Bounds: 16 MiB, 64 levels, 1,000,000 value/search nodes. Rejected union branches consume the work budget. `validate: true` retains Ruby Avro's rules and extra-field checks, with bounded recursive validation.

Registry/framing errors stay in AvroTurf. Codec errors use Ruby Avro classes or `EOFError`; messages, malformed-input rejection and failure cursor behavior may differ. Bounds reject some inputs stock Avro accepts. Native failures never retry through Ruby.

## Reuse

Each client caches 128 prepared schemas; each writer caches eight reader plans. Schema mutation invalidates cached codecs, including edits to fields, symbols and defaults. Do not mutate schemas during a call.

Cache access uses a mutex; codec work runs outside it. Routing is fiber-local and restored after nested calls/errors. Threads, interleaved fibers, GC compaction and fork reuse are tested. Registry clients retain their own thread/fork constraints.

Ruby conversion and Messaging retain the calling Ractor's GVL. Create one client per Ractor; standalone prepared schemas are shareable.

```ruby
Ractor.new(existing_options, payload) do |options, bytes|
  Avrocadabra::AvroTurf::Messaging.new(**options).decode(bytes)
end.value
```

Load dependencies before workers. Create registry clients/loggers inside their Ractor. Worker Avro/Excon tables use load-time snapshots; later main-Ractor adapter registrations stay local. Configure Excon inside the worker or pass constructor options. Workers parse schemas with Ruby `JSON`; main-Ractor MultiJson settings stay unchanged.

Tested: AvroTurf 1.20.2, Ruby Avro 1.12.2. Rollback: `AvroTurf::Messaging.new`. [Benchmarks](../benchmark/README.md#messaging).
