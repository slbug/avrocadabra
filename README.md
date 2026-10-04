# Avrocadabra

Raw Avro datums for CRuby 4.0. Rust `apache-avro` + Magnus.

```ruby
require "avrocadabra"

schema = Avrocadabra::Schema.new({
  type: "record", name: "Sample",
  fields: [{ name: "id", type: "long" }, { name: "label", type: "string" }]
})
bytes = schema.encode({ id: 42, label: "温度" })
schema.decode(bytes) # => {"id" => 42, "label" => "温度"}
```

## Schemas

Accepts JSON, a Hash, or a union Array. Primitive JSON needs quotes: `Schema.new('"long"')`. Reuse schemas; pass external named types in `references: [json_or_hash, ...]`.

```ruby
writer = Avrocadabra::Schema.new({ type: "record", name: "Sample",
  fields: [{ name: "id", type: "int" }] })
reader = Avrocadabra::Schema.new({ type: "record", name: "Sample",
  fields: [{ name: "id", type: "long" }, { name: "label", type: "string", default: "" }] })
writer.decode(writer.encode({ id: 42 }), reader_schema: reader)
# => {"id" => 42, "label" => ""}
```

The receiver supplies the writer schema. Resolution supports aliases, defaults, enums, fixed, recursive names, unions and Avro promotions. Each writer caches up to eight resolution plans.

- Fields use string keys, then symbols, preserving `false` and `nil`. Hash defaults, default procs and `key?`/`[]` overrides apply. Missing nullable fields become `nil`; other missing fields raise. Extra fields are ignored.
- Schema defaults fill reader fields absent from the writer schema.
- Map keys must be strings. Decoded records/maps have string keys in wire order; encoding preserves Hash order. Inputs stay untouched unless their own callbacks mutate them.
- Decode consumes one datum from a string or `StringIO`, leaving trailing bytes unread. Failed reads preserve the stream position.

## Types

| Avro | Ruby |
| --- | --- |
| null / boolean | `nil` / `true`, `false` |
| int / long | Signed 32-bit / 64-bit `Integer` |
| float / double | `Float`; encoding also accepts `Integer` and `BigDecimal` |
| string | UTF-8 `String`; encoding transcodes text |
| enum | Declared symbol as a `String`; no transcoding |
| bytes / fixed | `String`; decoded as `Encoding::BINARY` |
| array / map / record or error | `Array` / `Hash` / `Hash` |
| decimal, bytes or fixed | `BigDecimal`; encoding also accepts `Integer` and `Float` |
| big-decimal, bytes | `BigDecimal` with per-value scale; encoding also accepts `Integer` and `Float` |
| date | `Date`; encoding accepts `DateTime` or numeric days since 1970-01-01 |
| time-millis / micros | Integer ticks since midnight, within one day |
| timestamp-millis / micros / nanos | UTC `Time`; encoding accepts `Time`, `Date`, `DateTime` or numeric ticks |
| local-timestamp-millis / micros / nanos | Integer ticks since local 1970-01-01 |
| uuid, string or fixed16 | Canonical lowercase, hyphenated UUID string |
| duration, fixed12 | `Avrocadabra::Duration.new(months:, days:, milliseconds:)`; uint32 components |

- Decimals use `to_d`; use `BigDecimal` for exact input. Non-finite values, excess scale and overflow raise. Resolution requires matching precision and scale.
- Big-decimal preserves numeric value; Ruby normalizes insignificant zeros. Exponents outside Ruby BigDecimal's finite range raise.
- Numeric date/timestamp inputs use `to_i`; timestamps discard sub-unit fractions. Dates use the proleptic Gregorian calendar.
- Float/double use IEEE-754 rounding; overflow becomes signed infinity, matching Ruby Avro.

Convert custom objects before encoding. Collection `each` and text `encode` overrides apply; iterators must yield synchronously in the calling thread/fiber.

For registries, use the [AvroTurf integration](docs/avro_turf.md): add the gem, then replace `AvroTurf::Messaging.new` with `Avrocadabra::AvroTurf::Messaging.new`.

Unions choose the first accepting branch in schema order. Override with a branch index, primitive name, or full named type:

```ruby
schema = Avrocadabra::Schema.new(["int", "long"])
bytes = schema.encode(Avrocadabra::Union.new("long", 42))
schema.decode(bytes) # => 42
schema.decode(bytes, tagged_unions: true) # => Avrocadabra::Union.new(1, 42)
```

Decode unwraps unions; `tagged_unions: true` retains branch indices. Unknown logical annotations and invalid decimal annotations use the physical type. UUID-to-string resolution returns canonical text.

Ruby Avro resolution quirks are preserved:

- Defaults retain JSON float precision and UTF-8 strings; extra record keys are ignored.
- Nested defaults use truthiness fallback: `false` without its own field default becomes `:no_default`.
- Integer-to-floating promotions retain `Integer`; string/bytes promotions retain writer encoding.
- An unmatched enum symbol survives if the reader has no default.

## Bounds

`SchemaError`, `EncodeError`, `DecodeError` inherit from `Avrocadabra::Error`; `ResolutionError` inherits from `DecodeError`. Errors include paths/byte offsets where available. Validation is always on.

| Constructor option | Default | Maximum |
| --- | ---: | ---: |
| `max_depth` | 64 | 128 |
| `max_bytes` | 16 MiB | 64 MiB |
| `max_items` | 1,000,000 | 1,000,000 |

Limits must be positive:

- Items count values, containers, nulls, expanded defaults and union-search visits.
- Union wrappers and named references add no datum depth.
- Bytes bound the consumed datum and aggregate content, including copied names. They do not bound RSS.
- Resolution applies the smaller writer/reader limits before copying or decoding, including discarded fields.
- Schema JSON: 1 MiB total, 64 levels, 65,536 nodes. Fixed-scale decimals: 4,096 digits. Big-decimal coefficients use the byte limit; exponents never expand into zero-filled strings.

Ruby GC and `ObjectSpace.memsize_of` account for retained native schemas and resolution plans. Ruby allocation counts exclude native allocations.

## Concurrency

Prepared schemas are frozen and Ractor-shareable. Threads, Ractors and synchronous fibers can reuse them through GC compaction and after `fork`. Load the gem before starting Ractors; keep inputs stable during calls.

```ruby
schema = Avrocadabra::Schema.new('"long"')
Ractor.new(schema) { |codec| codec.decode(codec.encode(42)) }.value # => 42
```

`encode` and `decode` hold the GVL by default. `release_gvl: true` releases it for owned Rust work; Ruby conversions keep it. See [transition costs](benchmark/README.md).

## Build

Binary gems need RubyGems 4.1+ (prereleases supported) and no compiler. Source builds need Rust 1.99+, Cargo, a C toolchain, libclang and Ruby headers. Runtime dependencies: `json`, `bigdecimal`, `date`, `zeitwerk`; source builds add `rb_sys`. The optional integration needs `avro` and `avro_turf`.

```sh
bundle install
COVERAGE=true bundle exec rake
BUNDLE_WITH=mutation bundle install
BUNDLE_WITH=mutation bundle exec rake mutation
bundle exec ruby benchmark/codec.rb
bundle exec rake build package:verify
```

CI gates: Ruby line/branch coverage ≥95%, Mutineer score 100%. Reports: HTML/LCOV coverage and JUnit test results; coverage and test results go to Codecov. Rust has unit, interoperability and fuzz tests.

`rake build` builds source + seven native gems locally on macOS with the running Ruby release. `package:verify` installs and tests source/current-platform gems outside the checkout. Build tools, targets and manual publishing: [releases](docs/releasing.md).

[MIT](LICENSE.txt). [Bundled dependency licenses](docs/third_party.md).
