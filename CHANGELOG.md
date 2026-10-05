## 0.0.3

- Encoding writes Avro bytes during Ruby conversion: no intermediate value tree, no serde pass. `max_bytes` stops encoding as soon as written bytes exceed it, discarded union attempts included. `release_gvl` no longer changes `encode`.
- Collections and strings skip Ruby dispatch while their class resolves `each`, `size`, `keys`, `key?`, `[]`, `default`, `default_proc`, `encode` and `to_s` to core definitions; these checks reset after every Ruby callback.
- AvroTurf encode is Ruby Avro's `DatumWriter#write`, emulated call site by call site. Calls into load-time Ruby Avro, bigdecimal or core methods that Ruby cannot observe are skipped. Every other call keeps Ruby Avro's receiver, arguments, visibility, order and lexical constants, after buffered bytes reach the stream. Bytes, return values, callback counts and exceptions match, including under mid-call redefinition, schema mutation and a `raise` that returns.
- Verification is cached per definition epoch. Frozen native hooks on `Module`, ahead of each hooked ancestry's own singleton methods, bump it on definition, removal, visibility, mixin and constant changes. Class-variable and constant-cache counters cover the rest.
- Union branches are tried natively. A `validate_recursive`-equivalent walk confirms each rejection. Anything observable restarts selection through `Schema.validate`. Every examined branch charges the work budget.
- Built-in `BytesDecimal` encodes natively, matching bigdecimal's truncated `Float#to_d` digits and dtoa tie rounding while `BigDecimal.limit` is 0. Precision errors replay the adapter in Ruby.
- The adapter-mapped standalone encoder AvroTurf used is gone; mappings serve decode only.
- Schema mutation checks call readers through generated Ruby instead of C dispatch and compare results by raw identity, so an overridden `equal?` hides nothing. Native schemas follow the `fields` reader, so replaced readers recompile.
- Ruby objects allocated while Rust owns memory are created under `rb_protect`, typed data is allocated empty before its Rust value is attached, and errors become Ruby exceptions before they leave Rust, so Ruby exceptions never unwind through Rust frames.
- Messaging encode, 500-entry nested unions: 18.6 ms → 0.51 ms (Ruby AvroTurf 14.7 ms). 500-item flat: 1.11 ms → 0.28 ms. Small: 19 µs → 8 µs.
- Nested-union benchmark payload.

## 0.0.2

- Content-addressable native gems for RubyGems 4.1+, with SHA-based filenames.
- Build, verify and publish tasks select native artifacts by platform and Ruby ABI.
- Codecov test results from RSpec JUnit reports, alongside coverage.

## 0.0.1

- Rust Avro codec: reusable schemas, logical types, unions and reader resolution.
- Optional AvroTurf Messaging integration.
- Bounded decoding; thread, fork and Ractor support.
- Source and prebuilt gems. Manual publishing.
