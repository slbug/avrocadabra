## 0.0.3

- Encoding writes Avro bytes during Ruby conversion: no intermediate value tree, no serde pass. `max_bytes` stops encoding as soon as written bytes exceed it, discarded union attempts included. `release_gvl` no longer changes `encode`.
- Collections and strings skip Ruby dispatch while their class resolves `each`, `size`, `keys`, `key?`, `[]`, `default`, `default_proc`, `encode` and `to_s` to core definitions; these checks reset after every Ruby callback.
- AvroTurf unions resolve natively while a datum invokes no Ruby callbacks. Default procs, overridden lookups, custom or redefined adapters, date or timestamp values other than Integer, Float or `Time`, and other value classes keep Ruby Avro's validation and call counts. Every examined branch charges the work budget.
- Built-in `BytesDecimal` encodes natively, matching bigdecimal's truncated `Float#to_d` digits and dtoa tie rounding, while its Ruby Avro and bigdecimal methods keep stock definitions and `BigDecimal.limit` is 0.
- Rejected `null` branches skip Ruby Avro validation and its `inspect`-built message; each still costs one budget item.
- Record lookups use the Ruby schema's field-name objects, so `compare_by_identity` hashes match Ruby Avro; names that are not exact `String`s dispatch through protected Ruby calls.
- Schema mutation checks call readers through generated Ruby instead of C dispatch. Native schemas follow the `fields` reader, so replaced readers recompile.
- Ruby objects allocated while Rust owns memory are created under `rb_protect`, typed data is allocated empty before its Rust value is attached, and errors become Ruby exceptions before they leave Rust, so Ruby exceptions never unwind through Rust frames.
- Messaging encode, 500-entry nested unions: 18.6 ms → 0.56 ms (Ruby AvroTurf 14.2 ms). 500-item flat: 1.11 ms → 0.41 ms. Small: 19 µs → 9 µs.
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
