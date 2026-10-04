use crate::{
    Core, error,
    guard::Limits,
    mapping::{self, Mapping},
    namespace,
};
use apache_avro::{
    Decimal, Duration, Schema,
    schema::{InnerDecimalSchema, NamesRef, NamespaceRef},
    types::Value as AvroValue,
};
use magnus::{
    Error, RArray, RClass, RHash, RModule, RString, Ruby, TryConvert, Value, prelude::*, value::Id,
};
use num_bigint::BigInt;
use std::collections::HashMap;

pub fn encode(
    ruby: &Ruby,
    core: &Core,
    value: Value,
    keys: RArray,
    mapping: Option<Mapping>,
) -> Result<AvroValue, Error> {
    let module = namespace(ruby)?;
    let codec = core.prepared.borrow_codec();
    let mut ctx = Encoder {
        ruby,
        names: codec.resolved.get_names(),
        keys,
        fields: &core.fields,
        limits: core.limits,
        path: "$".into(),
        items: 0,
        work: 0,
        bytes: 0,
        logical: module.const_get("Logical")?,
        union: module.const_get("Union")?,
        duration: module.const_get("Duration")?,
        decimal: ruby.class_object().const_get("BigDecimal")?,
        date: ruby.class_object().const_get("Date")?,
        error_class: module.const_get("EncodeError")?,
        hash_key: ruby.intern("key?"),
        hash_read: ruby.intern("[]"),
    };
    ctx.value(codec.schema, None, value, 0, mapping)
}

struct Encoder<'a, 's> {
    ruby: &'a Ruby,
    names: &'a NamesRef<'s>,
    keys: RArray,
    fields: &'a HashMap<String, usize>,
    limits: Limits,
    path: String,
    items: usize,
    work: usize,
    bytes: usize,
    logical: RModule,
    union: RClass,
    duration: RClass,
    decimal: RClass,
    date: RClass,
    error_class: magnus::ExceptionClass,
    hash_key: Id,
    hash_read: Id,
}

impl Encoder<'_, '_> {
    fn field_value(&self, data: RHash, key: Value, symbol: Value) -> Result<Value, Error> {
        let present: bool = data.funcall_public(self.hash_key, (key,))?;
        data.funcall_public(self.hash_read, (if present { key } else { symbol },))
    }

    fn fail(&self, message: impl AsRef<str>) -> Error {
        error(
            self.ruby,
            "EncodeError",
            format!("{}: {}", self.path, message.as_ref()),
        )
    }

    fn ruby_result<T>(&self, result: Result<T, Error>) -> Result<T, Error> {
        result.map_err(|e| self.fail(e.to_string()))
    }

    fn count_bytes(&mut self, n: usize) -> Result<(), Error> {
        self.bytes = self
            .bytes
            .checked_add(n)
            .filter(|&n| n <= self.limits.max_bytes)
            .ok_or_else(|| self.fail("value exceeds max_bytes"))?;
        Ok(())
    }

    fn integer(&self, value: Value) -> Result<i64, Error> {
        if !value.is_kind_of(self.ruby.class_integer()) {
            return Err(self.fail("expected Integer"));
        }
        self.ruby_result(i64::try_convert(value))
    }

    fn int32(&self, value: Value) -> Result<i32, Error> {
        i32::try_from(self.integer(value)?).map_err(|_| self.fail("Avro int out of range"))
    }

    fn float(&self, value: Value) -> Result<f64, Error> {
        let integer = value.is_kind_of(self.ruby.class_integer());
        let decimal = value.is_kind_of(self.decimal);
        if !value.is_kind_of(self.ruby.class_float()) && !integer && !decimal {
            return Err(self.fail("expected Float, Integer or BigDecimal"));
        }
        self.ruby_result(f64::try_convert(value))
    }

    fn string(&mut self, value: Value) -> Result<String, Error> {
        let value = RString::from_value(value).ok_or_else(|| self.fail("expected UTF-8 String"))?;
        let value: RString = value.funcall_public("encode", ("UTF-8",))?;
        self.count_bytes(value.len())?;
        self.ruby_result(unsafe { value.as_str() }.map(str::to_owned))
    }

    fn bytes(&mut self, value: Value) -> Result<Vec<u8>, Error> {
        let value =
            RString::from_value(value).ok_or_else(|| self.fail("expected binary String"))?;
        self.count_bytes(value.len())?;
        // Copy before any Ruby allocation or callback can move the source string.
        Ok(unsafe { value.as_slice() }.to_vec())
    }

    fn resolved<'a>(&self, schema: &'a Schema, ns: NamespaceRef<'_>) -> Result<&'a Schema, Error>
    where
        Self: 'a,
    {
        if let Schema::Ref { name } = schema {
            self.names
                .get(&name.fully_qualified_name(ns))
                .copied()
                .ok_or_else(|| self.fail(format!("unresolved name {}", name.fullname(ns))))
        } else {
            Ok(schema)
        }
    }

    fn label(&self, schema: &Schema, ns: NamespaceRef<'_>) -> String {
        if let Some(name) = schema.name() {
            return name.fullname(ns);
        }
        match schema {
            Schema::Null => "null",
            Schema::Boolean => "boolean",
            Schema::Int => "int",
            Schema::Long => "long",
            Schema::Float => "float",
            Schema::Double => "double",
            Schema::Bytes | Schema::Decimal(_) | Schema::BigDecimal => "bytes",
            Schema::String | Schema::Uuid(_) => "string",
            Schema::Array(_) => "array",
            Schema::Map(_) => "map",
            Schema::Date | Schema::TimeMillis => "int",
            _ => "long",
        }
        .into()
    }

    fn accepts(&self, schema: &Schema, ns: NamespaceRef<'_>, value: Value) -> Result<bool, Error> {
        let schema = self.resolved(schema, ns)?;
        Ok(match schema {
            Schema::Null => value.is_nil(),
            Schema::Boolean => {
                value.is_kind_of(self.ruby.class_true_class())
                    || value.is_kind_of(self.ruby.class_false_class())
            }
            Schema::Int
            | Schema::Long
            | Schema::TimeMillis
            | Schema::TimeMicros
            | Schema::LocalTimestampMillis
            | Schema::LocalTimestampMicros
            | Schema::LocalTimestampNanos => value.is_kind_of(self.ruby.class_integer()),
            Schema::Float | Schema::Double => {
                value.is_kind_of(self.ruby.class_float())
                    || value.is_kind_of(self.ruby.class_integer())
                    || value.is_kind_of(self.decimal)
            }
            Schema::String
            | Schema::Bytes
            | Schema::Enum(_)
            | Schema::Fixed(_)
            | Schema::Uuid(_) => RString::from_value(value).is_some(),
            Schema::Array(_) => RArray::from_value(value).is_some(),
            Schema::Map(_) | Schema::Record(_) => RHash::from_value(value).is_some(),
            Schema::Decimal(_) | Schema::BigDecimal => {
                value.is_kind_of(self.decimal)
                    || value.is_kind_of(self.ruby.class_integer())
                    || value.is_kind_of(self.ruby.class_float())
            }
            Schema::Date => {
                value.is_kind_of(self.date) || value.is_kind_of(self.ruby.class_numeric())
            }
            Schema::TimestampMillis | Schema::TimestampMicros | Schema::TimestampNanos => {
                value.is_kind_of(self.ruby.class_time())
                    || value.is_kind_of(self.date)
                    || value.is_kind_of(self.ruby.class_numeric())
            }
            Schema::Duration(_) => value.is_kind_of(self.duration),
            _ => false,
        })
    }

    fn union_index(
        &mut self,
        variants: &[Schema],
        value: Value,
        mapping: Mapping,
        depth: usize,
    ) -> Result<usize, Error> {
        for (index, schema) in variants.iter().enumerate() {
            if !matches!(
                schema,
                Schema::Null
                    | Schema::Boolean
                    | Schema::Int
                    | Schema::Long
                    | Schema::Float
                    | Schema::Double
                    | Schema::String
                    | Schema::Bytes
            ) || !mapping.child(index)?.identity()?
            {
                let budget = self.ruby.ary_from_vec(vec![
                    self.limits.max_items.saturating_sub(self.work),
                    self.limits.max_depth.saturating_sub(depth) + 1,
                ]);
                let index = mapping.union_index(value, budget)?;
                self.work = self.limits.max_items - budget.entry::<usize>(0)?;
                return Ok(index);
            }
        }
        for (index, schema) in variants.iter().enumerate() {
            if self.accepts(schema, None, value)? {
                let valid = match schema {
                    Schema::Int => self
                        .integer(value)
                        .is_ok_and(|value| i32::try_from(value).is_ok()),
                    Schema::Long => self.integer(value).is_ok(),
                    _ => true,
                };
                if valid {
                    return Ok(index);
                }
            }
        }
        Err(mapping.encoding_error(value)?)
    }

    fn value(
        &mut self,
        schema: &Schema,
        ns: NamespaceRef<'_>,
        value: Value,
        depth: usize,
        mapping: Option<Mapping>,
    ) -> Result<AvroValue, Error> {
        if let Schema::Ref { name } = schema {
            let full = name.fully_qualified_name(ns);
            let schema = self
                .names
                .get(&full)
                .copied()
                .ok_or_else(|| self.fail("unresolved named schema"))?;
            return self.value(schema, full.namespace(), value, depth, mapping);
        }
        let value = match mapping {
            Some(mapping) => mapping.convert("encode", value)?,
            None => value,
        };
        match self.physical(schema, ns, value, depth, mapping) {
            Err(error) if error.is_kind_of(self.error_class) && mapping.is_some() => {
                Err(mapping.unwrap().encoding_error(value)?)
            }
            result => result,
        }
    }

    fn physical(
        &mut self,
        schema: &Schema,
        ns: NamespaceRef<'_>,
        value: Value,
        depth: usize,
        mapping: Option<Mapping>,
    ) -> Result<AvroValue, Error> {
        if depth > self.limits.max_depth {
            return Err(self.fail("value exceeds maximum depth"));
        }
        self.items += 1;
        if self.items > self.limits.max_items {
            return Err(self.fail("value exceeds maximum item count"));
        }
        self.work += 1;
        if self.work > self.limits.max_items {
            return Err(self.fail("union search exceeds maximum item count"));
        }
        match schema {
            Schema::Null if value.is_nil() => Ok(AvroValue::Null),
            Schema::Boolean
                if value.is_kind_of(self.ruby.class_true_class())
                    || value.is_kind_of(self.ruby.class_false_class()) =>
            {
                Ok(AvroValue::Boolean(bool::try_convert(value)?))
            }
            Schema::Int => Ok(AvroValue::Int(self.int32(value)?)),
            Schema::Long => Ok(AvroValue::Long(self.integer(value)?)),
            Schema::Float => Ok(AvroValue::Float(self.float(value)? as f32)),
            Schema::Double => Ok(AvroValue::Double(self.float(value)?)),
            Schema::String => Ok(AvroValue::String(self.string(value)?)),
            Schema::Bytes => Ok(AvroValue::Bytes(self.bytes(value)?)),
            Schema::Fixed(fixed) => {
                let bytes = self.bytes(value)?;
                if bytes.len() != fixed.size {
                    return Err(self.fail(format!("fixed requires {} bytes", fixed.size)));
                }
                Ok(AvroValue::Fixed(fixed.size, bytes))
            }
            Schema::Enum(enumeration) => {
                let index = match mapping {
                    Some(mapping) => mapping.enum_index(value)?,
                    None => {
                        let symbol = RString::from_value(value)
                            .ok_or_else(|| self.fail("expected enum String"))?;
                        if symbol.enc_coderange_scan() != magnus::encoding::Coderange::SevenBit {
                            return Err(self.fail("unknown enum symbol"));
                        }
                        enumeration.symbols.iter().position(|candidate| {
                            (unsafe { symbol.as_slice() }) == candidate.as_bytes()
                        })
                    }
                }
                .ok_or_else(|| self.fail("unknown enum symbol"))?;
                let symbol = enumeration
                    .symbols
                    .get(index)
                    .ok_or_else(|| self.fail("enum index out of range"))?;
                self.count_bytes(symbol.len())?;
                Ok(AvroValue::Enum(index as u32, symbol.clone()))
            }
            Schema::Array(array) => {
                let values =
                    RArray::from_value(value).ok_or_else(|| self.fail("expected Array"))?;
                let length: usize = values.funcall_public("size", ())?;
                if length > self.limits.max_items - self.items {
                    return Err(self.fail("array exceeds maximum item count"));
                }
                let mut output = Vec::with_capacity(length);
                let child = mapping::child(mapping, 0)?;
                if length == 0 {
                    return Ok(AvroValue::Array(output));
                }
                crate::callback::each(self.ruby, value, |args| {
                    let n = self.path.len();
                    crate::push_index(&mut self.path, output.len());
                    output.push(self.value(
                        &array.items,
                        ns,
                        args.first().copied().unwrap_or(self.ruby.qnil().as_value()),
                        depth + 1,
                        child,
                    )?);
                    self.path.truncate(n);
                    Ok(())
                })?;
                Ok(AvroValue::Array(output))
            }
            Schema::Map(map) => {
                let values = RHash::from_value(value).ok_or_else(|| self.fail("expected Hash"))?;
                let length: usize = values.funcall_public("size", ())?;
                if length > self.limits.max_items - self.items {
                    return Err(self.fail("map exceeds maximum item count"));
                }
                let mut output = Vec::with_capacity(length);
                let child = mapping::child(mapping, 0)?;
                if length == 0 {
                    return Ok(AvroValue::Record(output));
                }
                crate::callback::each(self.ruby, value, |args| {
                    let pair;
                    let args = if args.len() == 1 {
                        pair = RArray::try_convert(args[0])?;
                        &[pair.entry(0)?, pair.entry(1)?]
                    } else {
                        args
                    };
                    let key = args.first().copied().unwrap_or(self.ruby.qnil().as_value());
                    let value = args.get(1).copied().unwrap_or(self.ruby.qnil().as_value());
                    let key = self.string(key)?;
                    let n = self.path.len();
                    self.path.push_str(&format!("[{key:?}]"));
                    output.push((key, self.value(&map.types, ns, value, depth + 1, child)?));
                    self.path.truncate(n);
                    Ok(())
                })?;
                Ok(AvroValue::Record(output))
            }
            Schema::Record(record) => {
                let data =
                    RHash::from_value(value).ok_or_else(|| self.fail("expected record Hash"))?;
                let full = record.name.fully_qualified_name(ns);
                if record.fields.len() > self.limits.max_items - self.items {
                    return Err(self.fail("record exceeds maximum item count"));
                }
                let mut output = Vec::with_capacity(record.fields.len());
                for (field_index, field) in record.fields.iter().enumerate() {
                    self.count_bytes(field.name.len())?;
                    let n = self.path.len();
                    self.path.push('.');
                    self.path.push_str(&field.name);
                    let index = self
                        .fields
                        .get(&field.name)
                        .ok_or_else(|| self.fail("missing prepared field"))?
                        * 2;
                    let key: Value = self.keys.entry(index as isize)?;
                    let symbol: Value = self.keys.entry(index as isize + 1)?;
                    let value = self.field_value(data, key, symbol)?;
                    let value = self.value(
                        &field.schema,
                        full.namespace(),
                        value,
                        depth + 1,
                        mapping::child(mapping, field_index)?,
                    )?;
                    output.push((field.name.clone(), value));
                    self.path.truncate(n);
                }
                Ok(AvroValue::Record(output))
            }
            Schema::Union(union) => {
                let datum;
                let index = if value.is_kind_of(self.union) {
                    let branch: Value = value.funcall("branch", ())?;
                    datum = value.funcall("value", ())?;
                    if branch.is_kind_of(self.ruby.class_integer()) {
                        self.ruby_result(usize::try_convert(branch))?
                    } else {
                        let branch: String = self.ruby_result(branch.funcall("to_s", ()))?;
                        union
                            .variants()
                            .iter()
                            .position(|s| self.label(s, ns) == branch)
                            .ok_or_else(|| self.fail(format!("unknown union branch {branch}")))?
                    }
                } else if let Some(mapping) = mapping {
                    datum = value;
                    self.union_index(union.variants(), value, mapping, depth)?
                } else {
                    let (items, bytes, path_len) = (self.items, self.bytes, self.path.len());
                    let mut failure = None;
                    for (i, schema) in union.variants().iter().enumerate() {
                        if self.accepts(schema, ns, value)? {
                            match self.value(schema, ns, value, depth, None) {
                                Ok(value) => {
                                    return Ok(AvroValue::Union(i as u32, Box::new(value)));
                                }
                                Err(error) => {
                                    if self.work > self.limits.max_items {
                                        return Err(error);
                                    }
                                    failure = Some(error.to_string());
                                }
                            }
                            self.items = items;
                            self.bytes = bytes;
                            self.path.truncate(path_len);
                        }
                    }
                    return Err(self.fail(
                        failure
                            .as_deref()
                            .unwrap_or("no union branch matches this Ruby value"),
                    ));
                };
                let schema = union
                    .variants()
                    .get(index)
                    .ok_or_else(|| self.fail("union branch index out of range"))?;
                Ok(AvroValue::Union(
                    index as u32,
                    Box::new(self.value(
                        schema,
                        ns,
                        datum,
                        depth,
                        mapping::child(mapping, index)?,
                    )?),
                ))
            }
            Schema::Decimal(decimal) => {
                let unscaled: String = self.ruby_result(self.logical.funcall(
                    "decimal_unscaled",
                    (value, decimal.precision, decimal.scale),
                ))?;
                let integer = BigInt::parse_bytes(unscaled.as_bytes(), 10)
                    .ok_or_else(|| self.fail("invalid decimal"))?;
                let mut bytes = integer.to_signed_bytes_be();
                if let InnerDecimalSchema::Fixed(fixed) = &decimal.inner {
                    if bytes.len() > fixed.size {
                        return Err(self.fail("decimal overflows fixed size"));
                    }
                    self.count_bytes(fixed.size)?;
                    let mut padded = vec![
                        if integer.sign() == num_bigint::Sign::Minus {
                            255
                        } else {
                            0
                        };
                        fixed.size - bytes.len()
                    ];
                    padded.append(&mut bytes);
                    bytes = padded;
                } else {
                    self.count_bytes(bytes.len())?;
                }
                Ok(AvroValue::Decimal(Decimal::from(bytes)))
            }
            Schema::BigDecimal => {
                let parts: RArray =
                    self.ruby_result(self.logical.funcall("big_decimal_parts", (value,)))?;
                let coefficient: RString = parts.entry(0)?;
                if coefficient.len() > self.limits.max_bytes.saturating_mul(3) {
                    return Err(self.fail("big-decimal exceeds max_bytes"));
                }
                let scale: i64 = self.ruby_result(parts.entry(1))?;
                let coefficient = self.ruby_result(unsafe { coefficient.as_str() })?;
                let coefficient = apache_avro::BigDecimal::parse_bytes(coefficient.as_bytes(), 10)
                    .ok_or_else(|| self.fail("invalid big-decimal coefficient"))?
                    .into_bigint_and_scale()
                    .0;
                self.count_bytes((coefficient.bits() / 8 + 1) as usize)?;
                Ok(AvroValue::BigDecimal(apache_avro::BigDecimal::new(
                    coefficient,
                    scale,
                )))
            }
            Schema::Date => {
                let days: Value = self.ruby_result(self.logical.funcall("date_days", (value,)))?;
                Ok(AvroValue::Date(self.int32(days)?))
            }
            Schema::TimeMillis => {
                let ticks = self.int32(value)?;
                if !(0..86_400_000).contains(&ticks) {
                    return Err(self.fail("time-millis must be within a day"));
                }
                Ok(AvroValue::TimeMillis(ticks))
            }
            Schema::TimeMicros => {
                let ticks = self.integer(value)?;
                if !(0..86_400_000_000).contains(&ticks) {
                    return Err(self.fail("time-micros must be within a day"));
                }
                Ok(AvroValue::TimeMicros(ticks))
            }
            Schema::TimestampMillis | Schema::TimestampMicros | Schema::TimestampNanos => {
                let units = match schema {
                    Schema::TimestampMillis => 1000,
                    Schema::TimestampMicros => 1_000_000,
                    _ => 1_000_000_000,
                };
                let ticks: Value =
                    self.ruby_result(self.logical.funcall("timestamp_ticks", (value, units)))?;
                let ticks = self.integer(ticks)?;
                Ok(match schema {
                    Schema::TimestampMillis => AvroValue::TimestampMillis(ticks),
                    Schema::TimestampMicros => AvroValue::TimestampMicros(ticks),
                    _ => AvroValue::TimestampNanos(ticks),
                })
            }
            Schema::LocalTimestampMillis => {
                Ok(AvroValue::LocalTimestampMillis(self.integer(value)?))
            }
            Schema::LocalTimestampMicros => {
                Ok(AvroValue::LocalTimestampMicros(self.integer(value)?))
            }
            Schema::LocalTimestampNanos => Ok(AvroValue::LocalTimestampNanos(self.integer(value)?)),
            Schema::Uuid(inner) => {
                let text = self.string(value)?;
                let uuid = uuid::Uuid::parse_str(&text).map_err(|_| self.fail("invalid UUID"))?;
                Ok(match inner {
                    apache_avro::schema::UuidSchema::String => AvroValue::String(uuid.to_string()),
                    apache_avro::schema::UuidSchema::Bytes => {
                        AvroValue::Bytes(uuid.as_bytes().to_vec())
                    }
                    apache_avro::schema::UuidSchema::Fixed(fixed) => {
                        AvroValue::Fixed(fixed.size, uuid.as_bytes().to_vec())
                    }
                })
            }
            Schema::Duration(_) => {
                if !value.is_kind_of(self.duration) {
                    return Err(self.fail("expected Avrocadabra::Duration"));
                }
                let mut parts = [0u32; 3];
                for (i, name) in ["months", "days", "milliseconds"].iter().enumerate() {
                    let part: Value = self.ruby_result(value.funcall(*name, ()))?;
                    parts[i] = u32::try_from(self.integer(part)?)
                        .map_err(|_| self.fail("duration components must be uint32"))?;
                }
                Ok(AvroValue::Duration(Duration::new(
                    apache_avro::Months::new(parts[0]),
                    apache_avro::Days::new(parts[1]),
                    apache_avro::Millis::new(parts[2]),
                )))
            }
            _ => Err(self.fail("value does not match schema")),
        }
    }
}
