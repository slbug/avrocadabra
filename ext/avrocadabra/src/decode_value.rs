use apache_avro::{
    Schema,
    schema::{NamesRef, RecordSchema},
    types::Value as AvroValue,
};
use magnus::{Error, RArray, RModule, RString, Ruby, Value, prelude::*};
use serde_json::Value as Json;
use std::collections::HashMap;
use std::sync::Arc;

use crate::{
    guard::Limits,
    mapping::{self, Mapping},
};

pub struct Options {
    pub limits: Limits,
    pub tagged_unions: bool,
    pub mapping: Option<Mapping>,
    pub defaults: Vec<(usize, Arc<Json>)>,
    pub adapters: Vec<(usize, usize)>,
    pub failure: Option<(usize, String)>,
}

pub fn decode<'schema>(
    ruby: &Ruby,
    schema: &'schema Schema,
    names: &NamesRef<'schema>,
    value: AvroValue,
    keys: RArray,
    fields: &HashMap<String, usize>,
    options: Options,
) -> Result<Value, Error> {
    Decoder {
        ruby,
        names,
        keys,
        fields,
        limits: options.limits,
        tagged_unions: options.tagged_unions,
        remaining: options.limits.max_items,
        path: "$".into(),
        logical: None,
        defaults: options.defaults.into_iter().peekable(),
        adapters: options.adapters.into_iter().peekable(),
        field_position: 0,
        read_position: 0,
        failure: options.failure,
    }
    .value(schema, value, None, 0, options.mapping)
}

struct Decoder<'context, 'schema> {
    ruby: &'context Ruby,
    names: &'context NamesRef<'schema>,
    keys: RArray,
    fields: &'context HashMap<String, usize>,
    limits: Limits,
    remaining: usize,
    path: String,
    logical: Option<RModule>,
    tagged_unions: bool,
    defaults: std::iter::Peekable<std::vec::IntoIter<(usize, Arc<Json>)>>,
    adapters: std::iter::Peekable<std::vec::IntoIter<(usize, usize)>>,
    field_position: usize,
    read_position: usize,
    failure: Option<(usize, String)>,
}

impl<'schema> Decoder<'_, 'schema> {
    fn error(&self, message: impl std::fmt::Display) -> Error {
        crate::error(
            self.ruby,
            "DecodeError",
            format!("{}: {message}", self.path),
        )
    }

    fn enter(&mut self, depth: usize) -> Result<(), Error> {
        if depth > self.limits.max_depth {
            return Err(self.error("decoded value exceeds maximum depth"));
        }
        self.remaining = self
            .remaining
            .checked_sub(1)
            .ok_or_else(|| self.error("decoded value exceeds maximum item count"))?;
        Ok(())
    }

    fn logical(&mut self) -> Result<RModule, Error> {
        if let Some(module) = self.logical {
            return Ok(module);
        }
        let namespace: RModule = self.ruby.class_object().const_get("Avrocadabra")?;
        let module = namespace.const_get("Logical")?;
        self.logical = Some(module);
        Ok(module)
    }

    fn value(
        &mut self,
        schema: &'schema Schema,
        value: AvroValue,
        namespace: Option<&str>,
        depth: usize,
        mapping: Option<Mapping>,
    ) -> Result<Value, Error> {
        if let Some((position, error)) = &self.failure
            && *position == self.read_position
        {
            return Err(crate::error(self.ruby, "ResolutionError", error));
        }
        self.enter(depth)?;
        let adapters = self
            .adapters
            .next_if(|(position, _)| *position == self.read_position)
            .map_or(1, |(_, count)| count);
        self.read_position += 1;
        let mut value = self.physical(schema, value, namespace, depth, mapping)?;
        if let Some(mapping) = mapping {
            for _ in 0..adapters {
                value = mapping.convert("decode", value)?;
            }
        }
        Ok(value)
    }

    fn physical(
        &mut self,
        mut schema: &'schema Schema,
        value: AvroValue,
        namespace: Option<&str>,
        depth: usize,
        mapping: Option<Mapping>,
    ) -> Result<Value, Error> {
        let mut hops = 0;
        while let Schema::Ref { name } = schema {
            if hops > self.names.len() {
                return Err(self.error("cyclic unresolved schema reference"));
            }
            schema = self
                .names
                .get(name.fully_qualified_name(namespace).as_ref())
                .or_else(|| self.names.get(name))
                .copied()
                .ok_or_else(|| self.error(format_args!("unresolved schema reference {name}")))?;
            hops += 1;
        }

        match (schema, value) {
            (Schema::Null, AvroValue::Null) => Ok(self.ruby.qnil().as_value()),
            (Schema::Boolean, AvroValue::Boolean(value)) => Ok(self.ruby.into_value(value)),
            (
                Schema::Int | Schema::Long | Schema::Float | Schema::Double,
                AvroValue::Int(value),
            ) => Ok(self.ruby.into_value(value)),
            (Schema::Long | Schema::Float | Schema::Double, AvroValue::Long(value)) => {
                Ok(self.ruby.into_value(value))
            }
            (Schema::Float | Schema::Double, AvroValue::Float(value)) => {
                Ok(self.ruby.into_value(f64::from(value)))
            }
            (Schema::Float | Schema::Double, AvroValue::Double(value)) => {
                Ok(self.ruby.into_value(value))
            }
            (Schema::String | Schema::Bytes, AvroValue::String(value)) => self.string(&value),
            (Schema::Bytes | Schema::String, AvroValue::Bytes(value)) => self.bytes(&value),
            (Schema::Fixed(fixed), AvroValue::String(value)) if value.len() == fixed.size => {
                self.string(&value)
            }
            (Schema::Fixed(fixed), AvroValue::Fixed(size, value))
                if size == fixed.size && value.len() == fixed.size =>
            {
                self.bytes(&value)
            }
            (Schema::Enum(_), AvroValue::Enum(_, symbol)) => self.string(&symbol),
            (Schema::Union(union), AvroValue::Union(index, value)) => {
                let branch = union
                    .variants()
                    .get(index as usize)
                    .ok_or_else(|| self.error("decoded union branch index is invalid"))?;
                let value = self.value(
                    branch,
                    *value,
                    namespace,
                    depth,
                    mapping::child(mapping, index as usize)?,
                )?;
                if self.tagged_unions {
                    let union: magnus::RClass = crate::namespace(self.ruby)?.const_get("Union")?;
                    union.funcall("new", (index, value))
                } else {
                    Ok(value)
                }
            }
            (Schema::Record(record), AvroValue::Record(values)) => {
                self.record(record, values, namespace, depth, mapping)
            }
            (Schema::Array(array), AvroValue::Array(values)) => {
                self.collection_size(values.len())?;
                let output = self.ruby.ary_new_capa(values.len());
                let child = mapping::child(mapping, 0)?;
                for (index, value) in values.into_iter().enumerate() {
                    let path_length = self.path.len();
                    crate::push_index(&mut self.path, index);
                    let value = self.value(&array.items, value, namespace, depth + 1, child);
                    self.path.truncate(path_length);
                    output.push(value?)?;
                }
                Ok(output.as_value())
            }
            (Schema::Map(map), AvroValue::Record(values)) => {
                self.collection_size(values.len())?;
                let output = self.ruby.hash_new_capa(values.len());
                let child = mapping::child(mapping, 0)?;
                for (index, (name, value)) in values.into_iter().enumerate() {
                    let path_length = self.path.len();
                    crate::push_index(&mut self.path, index);
                    let key = self.ruby.str_new(&name);
                    key.freeze();
                    let value = self.value(&map.types, value, namespace, depth + 1, child);
                    self.path.truncate(path_length);
                    output.aset(key, value?)?;
                }
                Ok(output.as_value())
            }
            (Schema::Decimal(decimal), AvroValue::Decimal(value)) => {
                let unscaled = apache_avro::BigDecimal::new(value.into(), 0).to_string();
                if unscaled.trim_start_matches('-').len() > decimal.precision {
                    return Err(self.error("decimal exceeds schema precision"));
                }
                self.logical()?
                    .funcall("decimal_value", (unscaled, decimal.scale))
            }
            (Schema::BigDecimal, AvroValue::BigDecimal(value)) => {
                let (coefficient, scale) = value.as_bigint_and_exponent();
                self.logical()?
                    .funcall("decimal_value", (coefficient.to_str_radix(10), scale))
            }
            (Schema::Date, AvroValue::Date(days)) => self.logical()?.funcall("date_value", (days,)),
            (Schema::TimeMillis, AvroValue::TimeMillis(ticks))
                if (0..86_400_000).contains(&ticks) =>
            {
                Ok(self.ruby.into_value(ticks))
            }
            (Schema::TimeMicros, AvroValue::TimeMicros(ticks))
                if (0..86_400_000_000).contains(&ticks) =>
            {
                Ok(self.ruby.into_value(ticks))
            }
            (Schema::TimestampMillis, AvroValue::TimestampMillis(ticks)) => {
                self.logical()?.funcall("timestamp_value", (ticks, 1_000))
            }
            (Schema::TimestampMicros, AvroValue::TimestampMicros(ticks)) => self
                .logical()?
                .funcall("timestamp_value", (ticks, 1_000_000)),
            (Schema::TimestampNanos, AvroValue::TimestampNanos(ticks)) => self
                .logical()?
                .funcall("timestamp_value", (ticks, 1_000_000_000)),
            (Schema::LocalTimestampMillis, AvroValue::LocalTimestampMillis(ticks))
            | (Schema::LocalTimestampMicros, AvroValue::LocalTimestampMicros(ticks))
            | (Schema::LocalTimestampNanos, AvroValue::LocalTimestampNanos(ticks)) => {
                Ok(self.ruby.into_value(ticks))
            }
            (Schema::Uuid(_), AvroValue::Uuid(value)) => self.string(&value.to_string()),
            (Schema::Duration(fixed), AvroValue::Duration(value)) if fixed.size == 12 => {
                let namespace: RModule = self.ruby.class_object().const_get("Avrocadabra")?;
                let duration: magnus::RClass = namespace.const_get("Duration")?;
                duration.funcall(
                    "new",
                    (
                        u32::from(value.months()),
                        u32::from(value.days()),
                        u32::from(value.millis()),
                    ),
                )
            }
            _ => Err(self.error("decoded value does not match the prepared schema")),
        }
    }

    fn collection_size(&self, length: usize) -> Result<(), Error> {
        if length > self.remaining {
            return Err(self.error("decoded collection exceeds maximum item count"));
        }
        Ok(())
    }

    fn string(&self, value: &str) -> Result<Value, Error> {
        if value.len() > self.limits.max_bytes {
            return Err(self.error("decoded string exceeds maximum byte count"));
        }
        Ok(self.ruby.str_new(value).as_value())
    }

    fn bytes(&self, value: &[u8]) -> Result<Value, Error> {
        if value.len() > self.limits.max_bytes {
            return Err(self.error("decoded bytes exceed maximum byte count"));
        }
        Ok(self.ruby.str_from_slice(value).as_value())
    }

    fn record(
        &mut self,
        record: &'schema RecordSchema,
        values: Vec<(String, AvroValue)>,
        namespace: Option<&str>,
        depth: usize,
        mapping: Option<Mapping>,
    ) -> Result<Value, Error> {
        self.collection_size(values.len())?;
        let output = self.ruby.hash_new_capa(record.fields.len());
        for (name, value) in values {
            let &field_index = record
                .lookup
                .get(&name)
                .ok_or_else(|| self.error("decoded record field is missing from schema"))?;
            let field = &record.fields[field_index];
            let index = self
                .fields
                .get(&field.name)
                .ok_or_else(|| self.error("prepared field name is missing"))?;
            let key: RString = self.keys.entry((index * 2) as isize)?;
            let path_length = self.path.len();
            self.path.push('.');
            self.path.push_str(&field.name);
            let default = self
                .defaults
                .next_if(|(position, _)| *position == self.field_position)
                .map(|(_, default)| default);
            self.field_position += 1;
            let value = match (default, mapping) {
                (Some(_), Some(mapping)) => mapping.default_value(key),
                (Some(default), None) => self.default_value(
                    &field.schema,
                    Some(&default),
                    record.name.namespace().or(namespace),
                    depth + 1,
                ),
                (None, _) => self.value(
                    &field.schema,
                    value,
                    record.name.namespace().or(namespace),
                    depth + 1,
                    mapping::child(mapping, field_index)?,
                ),
            };
            self.path.truncate(path_length);
            output.aset(key, value?)?;
        }
        if output.len() != record.fields.len() {
            return Err(self.error("decoded record has incorrect field count"));
        }
        Ok(output.as_value())
    }

    fn default_value(
        &mut self,
        schema: &'schema Schema,
        json: Option<&Json>,
        namespace: Option<&str>,
        depth: usize,
    ) -> Result<Value, Error> {
        self.enter(depth)?;
        let schema = crate::resolution::dereference(schema, self.names)
            .map_err(|error| self.error(error))?;
        match schema {
            Schema::Record(record) => {
                let object = json
                    .and_then(Json::as_object)
                    .ok_or_else(|| self.error("record default must be an object"))?;
                self.collection_size(record.fields.len())?;
                let output = self.ruby.hash_new_capa(record.fields.len());
                for field in &record.fields {
                    let json = object
                        .get(&field.name)
                        .filter(|value| !matches!(value, Json::Null | Json::Bool(false)))
                        .or(field.default.as_ref());
                    let path_length = self.path.len();
                    self.path.push('.');
                    self.path.push_str(&field.name);
                    let value = self.default_value(
                        &field.schema,
                        json,
                        record.name.namespace().or(namespace),
                        depth + 1,
                    );
                    self.path.truncate(path_length);
                    let index = self.fields[&field.name];
                    let key: RString = self.keys.entry((index * 2) as isize)?;
                    output.aset(key, value?)?;
                }
                Ok(output.as_value())
            }
            Schema::Array(array) => {
                let values = json
                    .and_then(Json::as_array)
                    .ok_or_else(|| self.error("array default must be an array"))?;
                self.collection_size(values.len())?;
                let output = self.ruby.ary_new_capa(values.len());
                for (index, value) in values.iter().enumerate() {
                    let path_length = self.path.len();
                    crate::push_index(&mut self.path, index);
                    let value = self.default_value(&array.items, Some(value), namespace, depth + 1);
                    self.path.truncate(path_length);
                    output.push(value?)?;
                }
                Ok(output.as_value())
            }
            Schema::Map(map) => {
                let values = json
                    .and_then(Json::as_object)
                    .ok_or_else(|| self.error("map default must be an object"))?;
                self.collection_size(values.len())?;
                let output = self.ruby.hash_new_capa(values.len());
                for (key, value) in values {
                    let key = self.ruby.str_new(key);
                    let value =
                        self.default_value(&map.types, Some(value), namespace, depth + 1)?;
                    output.aset(key, value)?;
                }
                Ok(output.as_value())
            }
            Schema::Union(union) => {
                let value = self.default_value(&union.variants()[0], json, namespace, depth)?;
                if self.tagged_unions {
                    let union: magnus::RClass = crate::namespace(self.ruby)?.const_get("Union")?;
                    union.funcall("new", (0, value))
                } else {
                    Ok(value)
                }
            }
            Schema::Null => Ok(self.ruby.qnil().as_value()),
            Schema::Int | Schema::Long => {
                let value = self.default_scalar(json)?;
                self.ruby.module_kernel().funcall("Integer", (value,))
            }
            Schema::Float | Schema::Double => {
                let value = self.default_scalar(json)?;
                self.ruby.module_kernel().funcall("Float", (value,))
            }
            Schema::Boolean
            | Schema::String
            | Schema::Bytes
            | Schema::Fixed(_)
            | Schema::Enum(_) => self.default_scalar(json),
            _ => {
                let json = json.ok_or_else(|| self.error("missing logical default"))?;
                let value = crate::resolution::default_datum(json, schema, self.names, self.limits)
                    .map_err(|error| self.error(error))?;
                self.physical(schema, value, namespace, depth, None)
            }
        }
    }

    fn default_scalar(&self, json: Option<&Json>) -> Result<Value, Error> {
        match json {
            None => Ok(self.ruby.to_symbol("no_default").as_value()),
            Some(Json::Null) => Ok(self.ruby.qnil().as_value()),
            Some(Json::Bool(value)) => Ok(self.ruby.into_value(*value)),
            Some(Json::String(value)) => self.string(value),
            Some(Json::Number(value)) => Ok(match value.as_i64() {
                Some(value) => self.ruby.into_value(value),
                None => self.ruby.into_value(
                    value
                        .as_f64()
                        .ok_or_else(|| self.error("invalid default number"))?,
                ),
            }),
            _ => Err(self.error("invalid scalar default")),
        }
    }
}
