# Codec benchmarks

```sh
bundle exec rake compile
bundle exec ruby --yjit benchmark/codec.rb --json benchmark/results.json
```

Prepared schemas; cross-decoding checked before timing. Synthetic payloads: 3 items / 359 bytes, 500 items / 47,107 bytes. Unicode, binary, nulls, maps, enums, timestamps and decimals.

Ruby 4.0.7 + YJIT + Prism, `arm64-darwin27`, Rust 1.99.0, Avrocadabra 0.0.1, Ruby avro 1.12.2. Per operation: 0.15 s warmup, five 0.25 s batches, GC enabled. Latency: 500 separate samples, including clock overhead. [Raw results](results.json).

Allocations count Ruby objects, excluding native memory. GVL comparisons measure single-thread transition cost; parallel scaling is unmeasured.

| Payload | Operation | Engine | p50 µs | p95 µs | Ops/s | Ruby allocs/op |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| small | encode | Ruby avro | 11 | 14 | 77,562 | 138 |
| small | decode | Ruby avro | 10 | 13 | 98,061 | 56 |
| small | encode | Native, GVL held | 11 | 51 | 83,817 | 63 |
| small | decode | Native, GVL held | 7 | 9 | 127,669 | 50 |
| small | encode | Native, GVL released | 11 | 14 | 85,939 | 63 |
| small | decode | Native, GVL released | 7 | 9 | 127,467 | 50 |
| large | encode | Ruby avro | 1,129 | 1,794 | 822 | 13,746 |
| large | decode | Ruby avro | 972 | 1,233 | 981 | 5,771 |
| large | encode | Native, GVL held | 1,002 | 1,350 | 971 | 5,529 |
| large | decode | Native, GVL held | 718 | 1,046 | 1,327 | 5,268 |
| large | encode | Native, GVL released | 1,011 | 1,369 | 965 | 5,529 |
| large | decode | Native, GVL released | 724 | 1,036 | 1,316 | 5,268 |

Native throughput: encode 8–18% faster, decode 1.3–1.4× Ruby avro. Releasing the GVL changed throughput by less than 3%.

## Messaging

```sh
bundle exec ruby --yjit benchmark/messaging.rb --json benchmark/messaging_results.json
```

Full Messaging calls against AvroTurf 1.20.2; stock runs before integration hooks load. Same timing settings; warm schema, registry and native caches. AvroTurf's fake registry uses local HTTP outside timed loops. Reader resolution adds a defaulted field. Payloads with headers: 364 and 47,112 bytes.

| Payload | Operation | Engine | p50 µs | p95 µs | Ops/s | Ruby allocs/op |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| small | encode | Ruby AvroTurf | 18 | 22 | 48,308 | 216 |
| small | decode | Ruby AvroTurf | 10 | 12 | 93,608 | 60 |
| small | decode resolved | Ruby AvroTurf | 12 | 14 | 78,092 | 62 |
| large | encode | Ruby AvroTurf | 1,732 | 1,884 | 576 | 19,423 |
| large | decode | Ruby AvroTurf | 1,001 | 1,208 | 962 | 5,775 |
| large | decode resolved | Ruby AvroTurf | 1,163 | 1,390 | 826 | 5,777 |
| small | encode | Native, GVL held | 19 | 23 | 49,212 | 78 |
| small | decode | Native, GVL held | 15 | 19 | 64,477 | 64 |
| small | decode resolved | Native, GVL held | 25 | 29 | 38,304 | 66 |
| large | encode | Native, GVL held | 1,109 | 1,443 | 856 | 6,538 |
| large | decode | Native, GVL held | 849 | 1,094 | 1,133 | 6,276 |
| large | decode resolved | Native, GVL held | 1,239 | 1,513 | 782 | 6,278 |

Large native messages: encode 1.5× faster, decode 1.2× faster, resolved decode 5% slower. Small: encode near parity, decode 1.5× slower, resolved decode 2.0× slower. Calls include schema mutation checks and Ruby callbacks. Decoding allocates more Ruby objects than stock.

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
