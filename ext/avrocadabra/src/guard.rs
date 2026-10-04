use apache_avro::{
    Schema,
    schema::{InnerDecimalSchema, NamesRef, UuidSchema},
};
use num_bigint::BigInt;
use num_traits::Signed;
use std::fmt::Display;

#[derive(Clone, Copy)]
pub struct Limits {
    pub max_depth: usize,
    pub max_bytes: usize,
    pub max_items: usize,
}

impl Limits {
    pub fn intersect(self, other: Self) -> Self {
        Self {
            max_depth: self.max_depth.min(other.max_depth),
            max_bytes: self.max_bytes.min(other.max_bytes),
            max_items: self.max_items.min(other.max_items),
        }
    }
}

#[cfg(test)]
pub fn prefix(
    schema: &Schema,
    names: &NamesRef<'_>,
    bytes: &[u8],
    limits: Limits,
) -> Result<usize, String> {
    inspect(schema, names, bytes, limits).map(|frame| frame.consumed)
}

pub struct Frame {
    pub consumed: usize,
    pub unions: Vec<u32>,
}

pub fn inspect(
    schema: &Schema,
    names: &NamesRef<'_>,
    bytes: &[u8],
    mut limits: Limits,
) -> Result<Frame, String> {
    // A caller-selected limit must not turn recursive schemas into a native stack overflow.
    limits.max_depth = limits.max_depth.min(256);
    let mut scanner = Scanner {
        bytes,
        position: 0,
        end: bytes.len().min(limits.max_bytes),
        remaining: limits.max_items,
        remaining_bytes: limits.max_bytes,
        limits,
        names,
        path: "$".into(),
        unions: Vec::new(),
    };
    scanner.value(schema, 0)?;
    Ok(Frame {
        consumed: scanner.position,
        unions: scanner.unions,
    })
}

struct Scanner<'data, 'names, 'schema> {
    bytes: &'data [u8],
    position: usize,
    end: usize,
    remaining: usize,
    remaining_bytes: usize,
    limits: Limits,
    names: &'names NamesRef<'schema>,
    path: String,
    unions: Vec<u32>,
}

impl<'data, 'schema> Scanner<'data, '_, 'schema> {
    fn error(&self, message: impl Display) -> String {
        format!("{} at byte {}: {message}", self.path, self.position)
    }

    fn allocation(&mut self, bytes: usize) -> Result<(), String> {
        self.remaining_bytes = self
            .remaining_bytes
            .checked_sub(bytes)
            .ok_or_else(|| self.error("decoded values exceed maximum byte count"))?;
        Ok(())
    }

    fn take(&mut self, length: usize) -> Result<&'data [u8], String> {
        if length > self.limits.max_bytes.saturating_sub(self.position) {
            return Err(self.error("datum exceeds max_bytes"));
        }
        let end = self
            .position
            .checked_add(length)
            .filter(|&end| end <= self.end)
            .ok_or_else(|| self.error("truncated datum or collection block"))?;
        let bytes = &self.bytes[self.position..end];
        self.position = end;
        Ok(bytes)
    }

    fn long(&mut self) -> Result<i64, String> {
        let mut value = 0_u64;
        for shift in (0..70).step_by(7) {
            let byte = self.take(1)?[0];
            if shift == 63 && byte > 1 {
                return Err(self.error("Avro integer varint overflows 64 bits"));
            }
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(((value >> 1) as i64) ^ -((value & 1) as i64));
            }
        }
        Err(self.error("unterminated Avro integer varint"))
    }

    fn int(&mut self) -> Result<i32, String> {
        i32::try_from(self.long()?)
            .map_err(|_| self.error("Avro int is outside signed 32-bit range"))
    }

    fn length(&mut self) -> Result<usize, String> {
        let length = self.long()?;
        usize::try_from(length).map_err(|_| self.error("negative or overflowing byte length"))
    }

    fn bytes(&mut self) -> Result<&'data [u8], String> {
        let length = self.length()?;
        self.allocation(length)?;
        self.take(length)
    }

    fn string(&mut self) -> Result<&'data str, String> {
        std::str::from_utf8(self.bytes()?).map_err(|_| self.error("Avro string is not valid UTF-8"))
    }

    fn value(&mut self, mut schema: &'schema Schema, depth: usize) -> Result<(), String> {
        if depth > self.limits.max_depth {
            return Err(self.error("datum exceeds maximum depth"));
        }
        if self.remaining == 0 {
            return Err(self.error("datum exceeds maximum item count"));
        }
        self.remaining -= 1;
        let mut hops = 0;
        while let Schema::Ref { name } = schema {
            if hops > self.names.len() {
                return Err(self.error(format_args!("cyclic unresolved schema reference {name}")));
            }
            schema =
                self.names.get(name).copied().ok_or_else(|| {
                    self.error(format_args!("unresolved schema reference {name}"))
                })?;
            hops += 1;
        }
        match schema {
            Schema::Null => {}
            Schema::Boolean => {
                if self.take(1)?[0] > 1 {
                    return Err(self.error("Avro boolean must be 0 or 1"));
                }
            }
            Schema::Int | Schema::Date => {
                self.int()?;
            }
            Schema::Long
            | Schema::TimestampMillis
            | Schema::TimestampMicros
            | Schema::TimestampNanos
            | Schema::LocalTimestampMillis
            | Schema::LocalTimestampMicros
            | Schema::LocalTimestampNanos => {
                self.long()?;
            }
            Schema::TimeMillis => {
                if !(0..86_400_000).contains(&self.int()?) {
                    return Err(self.error("time-millis is outside one day"));
                }
            }
            Schema::TimeMicros => {
                if !(0..86_400_000_000).contains(&self.long()?) {
                    return Err(self.error("time-micros is outside one day"));
                }
            }
            Schema::Float => {
                self.take(4)?;
            }
            Schema::Double => {
                self.take(8)?;
            }
            Schema::Bytes => {
                self.bytes()?;
            }
            Schema::String => {
                self.string()?;
            }
            Schema::Fixed(fixed) => {
                self.allocation(fixed.size)?;
                self.take(fixed.size)?;
            }
            Schema::Duration(fixed) => {
                if fixed.size != 12 {
                    return Err(self.error("duration requires fixed size 12"));
                }
                self.take(12)?;
            }
            Schema::Uuid(UuidSchema::String) => {
                uuid::Uuid::parse_str(self.string()?)
                    .map_err(|error| self.error(format_args!("invalid UUID: {error}")))?;
            }
            Schema::Uuid(UuidSchema::Bytes) => {
                uuid::Uuid::from_slice(self.bytes()?)
                    .map_err(|error| self.error(format_args!("invalid UUID: {error}")))?;
            }
            Schema::Uuid(UuidSchema::Fixed(fixed)) => {
                if fixed.size != 16 {
                    return Err(self.error("UUID requires fixed size 16"));
                }
                self.allocation(16)?;
                self.take(16)?;
            }
            Schema::Decimal(decimal) => {
                let bytes = match &decimal.inner {
                    InnerDecimalSchema::Bytes => self.bytes()?,
                    InnerDecimalSchema::Fixed(fixed) => {
                        self.allocation(fixed.size)?;
                        self.take(fixed.size)?
                    }
                };
                decimal_precision(bytes, decimal.precision).map_err(|error| self.error(error))?;
            }
            Schema::BigDecimal => {
                crate::big_decimal::validate(self.bytes()?).map_err(|error| self.error(error))?;
            }
            Schema::Enum(enumeration) => {
                let index = self.int()?;
                if index < 0 || index as usize >= enumeration.symbols.len() {
                    return Err(self.error("invalid enum symbol index"));
                }
                self.allocation(enumeration.symbols[index as usize].len())?;
            }
            Schema::Union(union) => {
                let index = self.long()?;
                let branch = usize::try_from(index)
                    .ok()
                    .and_then(|index| union.variants().get(index))
                    .ok_or_else(|| self.error("invalid union branch index"))?;
                self.unions.push(index as u32);
                self.value(branch, depth)?;
            }
            Schema::Record(record) => {
                if record.fields.len() > self.remaining {
                    return Err(self.error("record exceeds maximum item count"));
                }
                for field in &record.fields {
                    self.allocation(field.name.len())?;
                    let path_length = self.path.len();
                    self.path.push('.');
                    self.path.push_str(&field.name);
                    self.value(&field.schema, depth + 1)?;
                    self.path.truncate(path_length);
                }
            }
            Schema::Array(array) => self.collection(&array.items, false, depth)?,
            Schema::Map(map) => self.collection(&map.types, true, depth)?,
            Schema::Ref { .. } => return Err(self.error("unresolved schema reference")),
        }
        Ok(())
    }

    fn collection(
        &mut self,
        item_schema: &'schema Schema,
        is_map: bool,
        depth: usize,
    ) -> Result<(), String> {
        let mut index = 0_usize;
        loop {
            let signed_count = self.long()?;
            if signed_count == 0 {
                return Ok(());
            }
            let count = usize::try_from(signed_count.unsigned_abs())
                .ok()
                .filter(|&count| count <= self.remaining)
                .ok_or_else(|| self.error("collection exceeds maximum item count"))?;
            let enclosing_end = self.end;
            let block_end = if signed_count < 0 {
                let size = self.length()?;
                let end = self
                    .position
                    .checked_add(size)
                    .filter(|&end| end <= enclosing_end)
                    .ok_or_else(|| self.error("truncated collection block"))?;
                self.end = end;
                Some(end)
            } else {
                None
            };
            for _ in 0..count {
                let path_length = self.path.len();
                crate::push_index(&mut self.path, index);
                if is_map {
                    self.path.push_str(".key");
                    self.string()?;
                    self.path.truncate(self.path.len() - 4);
                }
                self.value(item_schema, depth + 1)?;
                self.path.truncate(path_length);
                index += 1;
            }
            if block_end.is_some_and(|end| self.position != end) {
                return Err(self.error("collection block byte size does not match its contents"));
            }
            self.end = enclosing_end;
        }
    }
}

pub(crate) fn decimal_precision(bytes: &[u8], precision: usize) -> Result<(), &'static str> {
    if bytes.is_empty() {
        return Err("decimal requires at least one signed byte");
    }
    if precision == 0 {
        return Err("decimal requires positive precision");
    }
    if bytes.len().saturating_mul(3) <= precision {
        return Ok(());
    }
    let value = BigInt::from_signed_bytes_be(bytes);
    if value.bits() <= precision.saturating_mul(3) as u64 {
        return Ok(());
    }
    if value.bits() > precision.saturating_mul(4) as u64 {
        return Err("decimal exceeds schema precision");
    }
    let precision = u32::try_from(precision).map_err(|_| "decimal precision is too large")?;
    if value.abs() >= BigInt::from(10_u8).pow(precision) {
        return Err("decimal exceeds schema precision");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use apache_avro::schema::ResolvedSchema;

    fn limits() -> Limits {
        crate::validation::initialize().unwrap();
        Limits {
            max_depth: 64,
            max_items: 1_000,
            max_bytes: 1_000_000,
        }
    }

    fn valid(schema: &Schema, bytes: &[u8]) -> Result<usize, String> {
        crate::validation::initialize().unwrap();
        let resolved = ResolvedSchema::new(schema).unwrap();
        prefix(schema, resolved.get_names(), bytes, limits())
    }

    fn schema(json: &str) -> Schema {
        crate::validation::initialize().unwrap();
        Schema::parse_str(json).unwrap()
    }

    fn long(value: i64) -> Vec<u8> {
        let mut value = ((value as u64) << 1) ^ ((value >> 63) as u64);
        let mut bytes = Vec::new();
        while value >= 0x80 {
            bytes.push((value as u8) | 0x80);
            value >>= 7;
        }
        bytes.push(value as u8);
        bytes
    }

    fn bytes(value: &[u8]) -> Vec<u8> {
        let mut encoded = long(value.len() as i64);
        encoded.extend_from_slice(value);
        encoded
    }

    #[test]
    fn exactly_one_datum_including_zero_byte_values() {
        assert_eq!(valid(&Schema::Null, &[]).unwrap(), 0);
        assert_eq!(valid(&Schema::Null, &[0]).unwrap(), 0);
        assert_eq!(valid(&Schema::Int, &[2]).unwrap(), 1);
        assert_eq!(valid(&Schema::Int, &[2, 0]).unwrap(), 1);
        assert!(valid(&schema(r#"{"type":"record","name":"R","fields":[]}"#), &[]).is_ok());
    }

    #[test]
    fn booleans_strings_and_unions_cannot_decode_truncated_as_null() {
        for candidate in [Schema::Boolean, Schema::String, schema(r#"["null","int"]"#)] {
            assert!(valid(&candidate, &[]).unwrap_err().contains("truncated"));
        }
        assert!(valid(&Schema::Boolean, &[0]).is_ok());
        assert!(valid(&Schema::Boolean, &[1]).is_ok());
        assert!(valid(&Schema::Boolean, &[2]).is_err());
        assert!(valid(&Schema::String, &[0]).is_ok());
        assert!(valid(&Schema::String, &[2]).is_err());
    }

    #[test]
    fn integer_boundaries_and_overflowing_varints() {
        for value in [i64::MIN, -1, 0, 1, i64::MAX] {
            assert!(valid(&Schema::Long, &long(value)).is_ok());
        }
        for value in [i64::from(i32::MIN), 0, i64::from(i32::MAX)] {
            assert!(valid(&Schema::Int, &long(value)).is_ok());
        }
        for value in [i64::from(i32::MIN) - 1, i64::from(i32::MAX) + 1] {
            assert!(valid(&Schema::Int, &long(value)).is_err());
            assert!(valid(&Schema::Date, &long(value)).is_err());
        }
        assert!(valid(&Schema::Long, &[0x80]).is_err());
        assert!(valid(&Schema::Long, &[0x80; 10]).is_err());
        let mut overflowing = vec![0xff; 9];
        overflowing.push(2);
        assert!(
            valid(&Schema::Long, &overflowing)
                .unwrap_err()
                .contains("overflows")
        );
        overflowing.push(0);
        assert!(valid(&Schema::Long, &overflowing).is_err());
    }

    #[test]
    fn byte_lengths_and_utf8_are_validated_before_allocation() {
        assert!(valid(&Schema::Bytes, &long(-1)).is_err());
        assert!(valid(&Schema::Bytes, &long(i64::MAX)).is_err());
        assert!(valid(&Schema::String, &bytes("Zażółć".as_bytes())).is_ok());
        assert!(
            valid(&Schema::String, &bytes(&[0xff]))
                .unwrap_err()
                .contains("UTF-8")
        );
        assert!(valid(&Schema::Bytes, &bytes(&[0xff])).is_ok());
        let mut limits = limits();
        limits.max_bytes = 2;
        assert!(prefix(&Schema::Bytes, &NamesRef::new(), &bytes(&[1, 2]), limits).is_err());
    }

    #[test]
    fn enum_union_and_fixed_indices_and_sizes() {
        let enumeration = schema(r#"{"type":"enum","name":"E","symbols":["A","B"]}"#);
        assert!(valid(&enumeration, &[2]).is_ok());
        assert!(valid(&enumeration, &[4]).is_err());
        assert!(valid(&enumeration, &[1]).is_err());
        let union = schema(r#"["null","int"]"#);
        assert!(valid(&union, &[0]).is_ok());
        assert!(valid(&union, &[2, 0]).is_ok());
        for bytes in [vec![4], vec![1], vec![2]] {
            assert!(valid(&union, &bytes).is_err());
        }
        let fixed = schema(r#"{"type":"fixed","name":"F","size":2}"#);
        assert!(valid(&fixed, &[0xff, 0]).is_ok());
        assert!(valid(&fixed, &[0xff]).is_err());
        assert_eq!(valid(&fixed, &[0xff, 0, 0]).unwrap(), 2);
    }

    #[test]
    fn collection_blocks_require_exact_declared_sizes() {
        let array = schema(r#"{"type":"array","items":"int"}"#);
        assert!(valid(&array, &[3, 4, 2, 4, 0]).is_ok());
        assert!(valid(&array, &[3, 2, 2, 4, 0]).is_err());
        assert!(valid(&array, &[3, 6, 2, 4, 0]).is_err());
        assert!(valid(&array, &[3, 1, 2, 4, 0]).is_err());
        assert!(valid(&array, &[3, 100, 2, 4, 0]).is_err());
        assert!(valid(&array, &[3, 4, 2, 4]).is_err());
        assert!(valid(&array, &[2, 2, 2, 4, 0]).is_ok());
    }

    #[test]
    fn nested_blocks_cannot_read_past_parent_block() {
        let array = schema(r#"{"type":"array","items":{"type":"array","items":"int"}}"#);
        assert!(valid(&array, &[1, 6, 2, 2, 0, 0]).is_ok());
        assert!(valid(&array, &[1, 4, 2, 2, 0, 0]).is_err());
        assert!(valid(&array, &[1, 6, 1, 4, 2, 0]).is_err());
    }

    #[test]
    fn null_items_and_empty_records_cannot_bypass_allocation_budget() {
        for array in [
            schema(r#"{"type":"array","items":"null"}"#),
            schema(r#"{"type":"array","items":{"type":"record","name":"R","fields":[]}}"#),
        ] {
            let resolved = ResolvedSchema::new(&array).unwrap();
            let mut limits = limits();
            limits.max_items = 4;
            assert!(prefix(&array, resolved.get_names(), &[6, 0], limits).is_ok());
            assert!(prefix(&array, resolved.get_names(), &[8, 0], limits).is_err());
            assert!(prefix(&array, resolved.get_names(), &[4, 4, 0], limits).is_err());
            assert!(prefix(&array, resolved.get_names(), &[3, 0, 2, 0], limits).is_ok());
            assert!(prefix(&array, resolved.get_names(), &[3, 0, 4, 0], limits).is_err());
            let mut bomb = long(i64::MIN);
            bomb.extend_from_slice(&[0, 0]);
            assert!(valid(&array, &bomb).is_err());
        }
    }

    #[test]
    fn repeated_field_names_and_enum_symbols_cannot_amplify_tiny_input() {
        let records = schema(
            r#"{"type":"array","items":{"type":"record","name":"R","fields":[
            {"name":"large_field_name","type":"null"}]}}"#,
        );
        let enums = schema(
            r#"{"type":"array","items":{"type":"enum","name":"E","symbols":["LARGE_SYMBOL"]}}"#,
        );
        let mut limits = limits();
        limits.max_bytes = 20;
        for (schema, bytes) in [(records, vec![4, 0]), (enums, vec![4, 0, 0, 0])] {
            let resolved = ResolvedSchema::new(&schema).unwrap();
            assert!(
                prefix(&schema, resolved.get_names(), &bytes, limits)
                    .unwrap_err()
                    .contains("byte count")
            );
        }
    }

    #[test]
    fn map_keys_are_utf8_and_zero_length_keys_are_bounded_by_entry_count() {
        let map = schema(r#"{"type":"map","values":"null"}"#);
        assert!(valid(&map, &[2, 0, 0]).is_ok());
        assert!(valid(&map, &[2, 2, 0xff, 0]).unwrap_err().contains(".key"));
        assert!(valid(&map, &[1, 2, 0, 0]).is_ok());
        assert!(valid(&map, &[1, 0, 0, 0]).is_err());
        let mut limits = limits();
        limits.max_items = 2;
        assert!(prefix(&map, &NamesRef::new(), &[4, 0, 0, 0], limits).is_err());
    }

    #[test]
    fn recursion_and_schema_references_have_a_depth_bound() {
        let recursive = schema(
            r#"{"type":"record","name":"n.R","fields":[
            {"name":"next","type":["null","n.R"]}]}"#,
        );
        let resolved = ResolvedSchema::new(&recursive).unwrap();
        let mut limits = limits();
        limits.max_depth = 4;
        assert!(prefix(&recursive, resolved.get_names(), &[2, 0], limits).is_ok());
        assert!(prefix(&recursive, resolved.get_names(), &[2, 2, 2, 0], limits).is_ok());
        let error = prefix(&recursive, resolved.get_names(), &[2, 2, 2, 2, 0], limits).unwrap_err();
        assert!(error.contains("$.next.next.next.next.next"));
        assert!(error.contains("depth"));
        let impossible =
            schema(r#"{"type":"record","name":"R","fields":[{"name":"r","type":"R"}]}"#);
        assert!(valid(&impossible, &[]).unwrap_err().contains("depth"));
    }

    #[test]
    fn named_dependencies_resolve_and_ref_hops_do_not_add_depth() {
        let dependency = schema(r#"{"type":"fixed","name":"example.F","size":1}"#);
        let reference = Schema::Ref {
            name: apache_avro::schema::Name::new("example.F").unwrap(),
        };
        let names = ResolvedSchema::new(&dependency).unwrap();
        let mut limits = limits();
        limits.max_depth = 0;
        assert!(prefix(&reference, names.get_names(), &[1], limits).is_ok());
        assert!(prefix(&reference, &NamesRef::new(), &[1], limits).is_err());
    }

    #[test]
    fn temporal_logical_values_obey_their_physical_limits() {
        assert!(valid(&Schema::Date, &long(i64::from(i32::MIN))).is_ok());
        for (schema, end) in [
            (Schema::TimeMillis, 86_400_000_i64),
            (Schema::TimeMicros, 86_400_000_000),
        ] {
            assert!(valid(&schema, &long(0)).is_ok());
            assert!(valid(&schema, &long(end - 1)).is_ok());
            assert!(valid(&schema, &long(-1)).is_err());
            assert!(valid(&schema, &long(end)).is_err());
        }
        for schema in [
            Schema::TimestampMillis,
            Schema::TimestampMicros,
            Schema::TimestampNanos,
            Schema::LocalTimestampMillis,
            Schema::LocalTimestampMicros,
            Schema::LocalTimestampNanos,
        ] {
            assert!(valid(&schema, &long(i64::MIN)).is_ok());
            assert!(valid(&schema, &long(i64::MAX)).is_ok());
        }
    }

    #[test]
    fn decimal_precision_is_checked_for_bytes_and_fixed() {
        let decimal = schema(r#"{"type":"bytes","logicalType":"decimal","precision":2,"scale":1}"#);
        for value in [-99_i64, 0, 99] {
            assert!(valid(&decimal, &bytes(&BigInt::from(value).to_signed_bytes_be())).is_ok());
        }
        for value in [-100_i64, 100] {
            assert!(valid(&decimal, &bytes(&BigInt::from(value).to_signed_bytes_be())).is_err());
        }
        assert!(valid(&decimal, &[0]).is_err());
        assert!(valid(&decimal, &bytes(&[0, 0, 99])).is_ok());
        let fixed = schema(
            r#"{"type":"fixed","name":"D","size":2,"logicalType":"decimal","precision":3,"scale":2}"#,
        );
        assert!(valid(&fixed, &[3, 231]).is_ok());
        assert!(valid(&fixed, &[3, 232]).is_err());
        assert!(valid(&Schema::BigDecimal, &[]).is_err());
        assert!(valid(&Schema::BigDecimal, &bytes(&[2, 123, 4])).is_ok());
        assert!(valid(&Schema::BigDecimal, &bytes(&[2, 123, 4, 0])).is_err());
    }

    #[test]
    fn uuid_and_duration_underlying_representations_are_validated() {
        let uuid = schema(r#"{"type":"string","logicalType":"uuid"}"#);
        assert!(valid(&uuid, &bytes(b"550e8400-e29b-41d4-a716-446655440000")).is_ok());
        assert!(valid(&uuid, &bytes(b"not a UUID")).is_err());
        assert!(valid(&Schema::Uuid(UuidSchema::Bytes), &bytes(&[0; 16])).is_ok());
        assert!(valid(&Schema::Uuid(UuidSchema::Bytes), &bytes(&[0; 15])).is_err());
        let uuid = schema(r#"{"type":"fixed","name":"U","size":16,"logicalType":"uuid"}"#);
        assert!(valid(&uuid, &[0; 16]).is_ok());
        assert!(valid(&uuid, &[0; 15]).is_err());
        let duration = schema(r#"{"type":"fixed","name":"D","size":12,"logicalType":"duration"}"#);
        assert!(valid(&duration, &[255; 12]).is_ok());
        assert!(valid(&duration, &[255; 11]).is_err());
    }

    #[test]
    fn seeded_malformed_input_never_passes_a_datum_that_apache_cannot_read() {
        use apache_avro::reader::datum::GenericDatumReader;

        let schemas = [
            Schema::Null,
            Schema::Boolean,
            Schema::Int,
            Schema::Long,
            Schema::Float,
            Schema::Double,
            Schema::String,
            Schema::Bytes,
            schema(r#"["null","boolean","long","string"]"#),
            schema(r#"{"type":"array","items":["null","int"]}"#),
            schema(r#"{"type":"map","values":"null"}"#),
            schema(
                r#"{"type":"record","name":"R","fields":[{"name":"v","type":"int"},{"name":"next","type":["null","R"]}]}"#,
            ),
        ];
        let mut random = 0x7f9b_0a4c_823d_65e1_u64;
        for schema in schemas {
            let resolved = ResolvedSchema::new(&schema).unwrap();
            let reader = GenericDatumReader::builder(&schema).build().unwrap();
            for sample in 0..4_096 {
                let mut input = [0_u8; 32];
                for byte in &mut input {
                    random ^= random << 13;
                    random ^= random >> 7;
                    random ^= random << 17;
                    *byte = random as u8;
                }
                let input = &input[..sample % 33];
                if let Ok(consumed) = prefix(&schema, resolved.get_names(), input, limits()) {
                    let mut remaining = &input[..consumed];
                    assert!(
                        reader.read_value(&mut remaining).is_ok(),
                        "{schema}: {input:?}"
                    );
                    assert!(remaining.is_empty(), "{schema}: {input:?}");
                }
            }
        }
    }
}
