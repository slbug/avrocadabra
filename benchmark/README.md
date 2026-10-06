# Codec benchmarks

```sh
bundle exec rake compile
bundle exec ruby --yjit benchmark/codec.rb --json benchmark/results.json
```

Prepared schemas; cross-decoding checked before timing. Synthetic payloads: 3 items / 359 bytes, 500 items / 47,107 bytes. Unicode, binary, nulls, maps, enums, timestamps and decimals. Nested: nullable 500-entry map of records, each with a nullable `[string, record]` array of nullable scalars and a decimal; 30,683 bytes.

Ruby 4.0.7 + YJIT + Prism, `arm64-darwin27`, Rust 1.99.0, Avrocadabra 0.0.3, Ruby avro 1.12.2. Per operation: 0.15 s warmup, five 0.25 s batches, GC enabled. Latency: 500 separate samples, including clock overhead. [Raw results](results.json).

Allocations count Ruby objects, excluding native memory. GVL comparisons measure single-thread transition cost; parallel scaling is unmeasured.

| Payload | Operation | Engine | p50 µs | p95 µs | Ops/s | Ruby allocs/op |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| small | encode | Ruby avro | 12 | 15 | 76,748 | 138 |
| small | decode | Ruby avro | 10 | 13 | 93,457 | 56 |
| small | encode | Native, GVL held | 4 | 5 | 255,686 | 20 |
| small | decode | Native, GVL held | 7 | 9 | 126,062 | 50 |
| small | decode | Native, GVL released | 8 | 10 | 122,195 | 50 |
| large | encode | Ruby avro | 1,292 | 1,505 | 785 | 13,746 |
| large | decode | Ruby avro | 1,114 | 1,390 | 862 | 5,771 |
| large | encode | Native, GVL held | 334 | 465 | 2,837 | 2,008 |
| large | decode | Native, GVL held | 771 | 987 | 1,243 | 5,268 |
| large | decode | Native, GVL released | 781 | 969 | 1,252 | 5,268 |
| nested | encode | Ruby avro | 13,485 | 13,969 | 75 | 105,802 |
| nested | decode | Ruby avro | 2,349 | 2,620 | 421 | 7,504 |
| nested | encode | Native, GVL held | 441 | 603 | 2,169 | 2,001 |
| nested | decode | Native, GVL held | 1,155 | 1,432 | 830 | 6,002 |
| nested | decode | Native, GVL released | 1,166 | 1,438 | 833 | 6,002 |

Native throughput: flat encode 3.3–3.6×, nested encode 29×; decode 1.35–2.0× Ruby avro. Encode writes while converting Ruby values, so it has no GVL-released variant; releasing the GVL for decode changed throughput by less than 4%.

## Messaging

```sh
bundle exec ruby --yjit benchmark/messaging.rb --json benchmark/messaging_results.json
```

Full Messaging calls against AvroTurf 1.20.2; stock runs before integration hooks load. Same timing settings; warm schema, registry and native caches. AvroTurf's fake registry uses local HTTP outside timed loops. Reader resolution adds a defaulted field. Payloads with headers: 364, 47,112 and 30,688 bytes.

| Payload | Operation | Engine | p50 µs | p95 µs | Ops/s | Ruby allocs/op |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| small | encode | Ruby AvroTurf | 19 | 26 | 45,581 | 216 |
| small | decode | Ruby AvroTurf | 10 | 13 | 90,440 | 60 |
| small | decode resolved | Ruby AvroTurf | 12 | 15 | 77,379 | 62 |
| large | encode | Ruby AvroTurf | 1,856 | 2,057 | 540 | 19,423 |
| large | decode | Ruby AvroTurf | 1,039 | 1,246 | 931 | 5,775 |
| large | decode resolved | Ruby AvroTurf | 1,186 | 1,418 | 809 | 5,777 |
| nested | encode | Ruby AvroTurf | 14,708 | 15,390 | 67 | 118,889 |
| nested | decode | Ruby AvroTurf | 2,167 | 2,513 | 457 | 7,508 |
| nested | decode resolved | Ruby AvroTurf | 2,521 | 2,880 | 393 | 7,510 |
| small | encode | Native, GVL held | 8 | 10 | 126,747 | 26 |
| small | decode | Native, GVL held | 11 | 14 | 82,156 | 66 |
| small | decode resolved | Native, GVL held | 19 | 25 | 50,081 | 70 |
| large | encode | Native, GVL held | 280 | 373 | 3,397 | 1,517 |
| large | decode | Native, GVL held | 832 | 1,127 | 1,132 | 6,278 |
| large | decode resolved | Native, GVL held | 1,233 | 1,542 | 794 | 6,282 |
| nested | encode | Native, GVL held | 512 | 631 | 1,907 | 1,514 |
| nested | decode | Native, GVL held | 1,320 | 1,580 | 745 | 7,011 |
| nested | decode resolved | Native, GVL held | 1,991 | 2,257 | 495 | 7,015 |

Nested native messages: encode 28× faster, decode 1.6× faster, resolved decode 1.3× faster. Large: encode 6.3× faster, decode 1.2× faster, resolved decode 2% slower. Small: encode 2.8× faster, decode 1.1× slower, resolved decode 1.5× slower. Encoded bytes, Ruby-visible calls and errors match Ruby AvroTurf. Calls include schema mutation checks and Ruby callbacks. Flat decoding allocates more Ruby objects than stock.

[Raw results](messaging_results.json). Broker I/O, application processing and consumer throughput are unmeasured.

Quick run:

```sh
bundle exec ruby benchmark/codec.rb --rounds 2 --seconds 0.05 --warmup 0.05 --samples 100
```

Seeded conversion and malformed-input fuzzing:

```sh
AVRO_FUZZ_CASES=10000 AVRO_FUZZ_SEED=194857 bundle exec rspec spec/avrocadabra/schema_property_spec.rb
```

Regenerate big-decimal fixtures with Apache Avro Java 1.12.2 and `slf4j-api` jars in `tmp/avro-java/`:

```sh
java --class-path 'tmp/avro-java/*' spec/fixtures/java/BigDecimalVectors.java > spec/fixtures/big_decimal_vectors.json
```
