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
| small | encode | Ruby avro | 12 | 15 | 75,313 | 138 |
| small | decode | Ruby avro | 11 | 13 | 91,387 | 56 |
| small | encode | Native, GVL held | 4 | 5 | 224,312 | 20 |
| small | decode | Native, GVL held | 8 | 9 | 123,389 | 50 |
| small | decode | Native, GVL released | 8 | 21 | 119,242 | 50 |
| large | encode | Ruby avro | 1,188 | 1,455 | 810 | 13,746 |
| large | decode | Ruby avro | 1,102 | 1,395 | 879 | 5,771 |
| large | encode | Native, GVL held | 315 | 468 | 2,985 | 2,008 |
| large | decode | Native, GVL held | 754 | 1,081 | 1,208 | 5,268 |
| large | decode | Native, GVL released | 763 | 1,089 | 1,249 | 5,268 |
| nested | encode | Ruby avro | 13,472 | 14,482 | 72 | 105,802 |
| nested | decode | Ruby avro | 2,313 | 2,598 | 432 | 7,504 |
| nested | encode | Native, GVL held | 458 | 602 | 2,090 | 2,001 |
| nested | decode | Native, GVL held | 1,129 | 1,360 | 861 | 6,002 |
| nested | decode | Native, GVL released | 1,176 | 1,465 | 830 | 6,002 |

Native throughput: flat encode 3.0–3.7×, nested encode 29×; decode 1.35–2.0× Ruby avro. Encode writes while converting Ruby values, so it has no GVL-released variant; releasing the GVL for decode changed throughput by less than 4%.

## Messaging

```sh
bundle exec ruby --yjit benchmark/messaging.rb --json benchmark/messaging_results.json
```

Full Messaging calls against AvroTurf 1.20.2; stock runs before integration hooks load. Same timing settings; warm schema, registry and native caches. AvroTurf's fake registry uses local HTTP outside timed loops. Reader resolution adds a defaulted field. Payloads with headers: 364, 47,112 and 30,688 bytes.

| Payload | Operation | Engine | p50 µs | p95 µs | Ops/s | Ruby allocs/op |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| small | encode | Ruby AvroTurf | 18 | 23 | 47,385 | 216 |
| small | decode | Ruby AvroTurf | 10 | 13 | 93,060 | 60 |
| small | decode resolved | Ruby AvroTurf | 12 | 14 | 79,459 | 62 |
| large | encode | Ruby AvroTurf | 1,758 | 2,062 | 567 | 19,423 |
| large | decode | Ruby AvroTurf | 996 | 1,198 | 974 | 5,775 |
| large | decode resolved | Ruby AvroTurf | 1,146 | 1,437 | 827 | 5,777 |
| nested | encode | Ruby AvroTurf | 14,733 | 15,876 | 70 | 118,889 |
| nested | decode | Ruby AvroTurf | 2,175 | 2,525 | 449 | 7,508 |
| nested | decode resolved | Ruby AvroTurf | 2,463 | 2,835 | 407 | 7,510 |
| small | encode | Native, GVL held | 10 | 12 | 90,208 | 53 |
| small | decode | Native, GVL held | 11 | 14 | 83,223 | 64 |
| small | decode resolved | Native, GVL held | 17 | 21 | 51,903 | 66 |
| large | encode | Native, GVL held | 421 | 541 | 2,263 | 1,544 |
| large | decode | Native, GVL held | 843 | 1,044 | 1,131 | 6,276 |
| large | decode resolved | Native, GVL held | 1,223 | 1,416 | 796 | 6,278 |
| nested | encode | Native, GVL held | 578 | 735 | 1,651 | 1,531 |
| nested | decode | Native, GVL held | 1,288 | 1,508 | 756 | 7,009 |
| nested | decode resolved | Native, GVL held | 2,034 | 2,339 | 494 | 7,011 |

Nested native messages: encode 24× faster, decode 1.7× faster, resolved decode 1.2× faster. Large: encode 4.0× faster, decode 1.2× faster, resolved decode 4% slower. Small: encode 1.9× faster, decode 1.1× slower, resolved decode 1.5× slower. Calls include schema mutation checks and Ruby callbacks. Flat decoding allocates more Ruby objects than stock.

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
