use apache_avro::{
    Schema,
    schema::{InnerDecimalSchema, NamesRef, RecordField, RecordSchema, UnionSchema, UuidSchema},
    types::Value,
};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{MapAccess, SeqAccess, Visitor},
    ser::{Error, SerializeMap, SerializeSeq, SerializeStruct},
};
use std::fmt;

// Serde retains map order. A named one-field record represents each enum's wire index
// because Serde's enum serializer requires static symbol names.
pub fn schema(schema: &Schema) -> Result<Schema, String> {
    Ok(match schema {
        Schema::Enum(value) => Schema::Record(
            RecordSchema::builder()
                .name(value.name.clone())
                .aliases(value.aliases.clone())
                .fields(vec![
                    RecordField::builder()
                        .name("index")
                        .schema(Schema::Int)
                        .build(),
                ])
                .build(),
        ),
        Schema::Record(value) => {
            let mut value = value.clone();
            for field in &mut value.fields {
                field.schema = self::schema(&field.schema)?;
                field.default = None;
            }
            Schema::Record(value)
        }
        Schema::Array(value) => {
            let mut value = value.clone();
            value.items = Box::new(self::schema(&value.items)?);
            Schema::Array(value)
        }
        Schema::Map(value) => {
            let mut value = value.clone();
            value.types = Box::new(self::schema(&value.types)?);
            Schema::Map(value)
        }
        Schema::Union(value) => Schema::Union(
            UnionSchema::new(
                value
                    .variants()
                    .iter()
                    .map(self::schema)
                    .collect::<Result<_, _>>()?,
            )
            .map_err(|error| error.to_string())?,
        ),
        Schema::Decimal(value) => match &value.inner {
            InnerDecimalSchema::Bytes => Schema::Bytes,
            InnerDecimalSchema::Fixed(value) => Schema::Fixed(value.clone()),
        },
        Schema::BigDecimal | Schema::Uuid(UuidSchema::Bytes) => Schema::Bytes,
        Schema::Uuid(UuidSchema::String) => Schema::String,
        Schema::Duration(value) | Schema::Uuid(UuidSchema::Fixed(value)) => {
            Schema::Fixed(value.clone())
        }
        Schema::Date | Schema::TimeMillis => Schema::Int,
        Schema::TimeMicros
        | Schema::TimestampMillis
        | Schema::TimestampMicros
        | Schema::TimestampNanos
        | Schema::LocalTimestampMillis
        | Schema::LocalTimestampMicros
        | Schema::LocalTimestampNanos => Schema::Long,
        value => value.clone(),
    })
}

pub struct Datum(pub Value);

struct DatumRef<'a>(&'a Value);

impl Serialize for Datum {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        DatumRef(&self.0).serialize(serializer)
    }
}

impl Serialize for DatumRef<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::Null => serializer.serialize_unit(),
            Value::Boolean(value) => serializer.serialize_bool(*value),
            Value::Int(value) | Value::Date(value) | Value::TimeMillis(value) => {
                serializer.serialize_i32(*value)
            }
            Value::Long(value)
            | Value::TimeMicros(value)
            | Value::TimestampMillis(value)
            | Value::TimestampMicros(value)
            | Value::TimestampNanos(value)
            | Value::LocalTimestampMillis(value)
            | Value::LocalTimestampMicros(value)
            | Value::LocalTimestampNanos(value) => serializer.serialize_i64(*value),
            Value::Float(value) => serializer.serialize_f32(*value),
            Value::Double(value) => serializer.serialize_f64(*value),
            Value::String(value) => serializer.serialize_str(value),
            Value::Bytes(value) | Value::Fixed(_, value) => serializer.serialize_bytes(value),
            Value::Enum(index, _) => {
                let mut record = serializer.serialize_struct("", 1)?;
                record.serialize_field("index", &(*index as i32))?;
                record.end()
            }
            Value::Union(index, value) => {
                serializer.serialize_newtype_variant("", *index, "", &DatumRef(value))
            }
            Value::Array(values) => {
                let mut seq = serializer.serialize_seq(Some(values.len()))?;
                for value in values {
                    seq.serialize_element(&DatumRef(value))?;
                }
                seq.end()
            }
            Value::Record(values) => {
                let mut map = serializer.serialize_map(Some(values.len()))?;
                for (key, value) in values {
                    map.serialize_entry(key, &DatumRef(value))?;
                }
                map.end()
            }
            Value::Decimal(value) => value.serialize(serializer),
            Value::BigDecimal(value) => serializer
                .serialize_bytes(&crate::big_decimal::encode(value).map_err(S::Error::custom)?),
            Value::Duration(value) => serializer.serialize_bytes(&<[u8; 12]>::from(*value)),
            Value::Map(_) | Value::Uuid(_) => {
                Err(S::Error::custom("unordered or unconverted datum"))
            }
        }
    }
}

impl<'de> Deserialize<'de> for Datum {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(DatumVisitor)
    }
}

struct DatumVisitor;

struct Key(String);

impl<'de> Deserialize<'de> for Key {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match deserializer.deserialize_identifier(DatumVisitor)? {
            Datum(Value::String(key)) => Ok(Self(key)),
            _ => Err(serde::de::Error::custom("expected string key")),
        }
    }
}

impl<'de> Visitor<'de> for DatumVisitor {
    type Value = Datum;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an Avro datum")
    }

    fn visit_unit<E>(self) -> Result<Datum, E> {
        Ok(Datum(Value::Null))
    }
    fn visit_bool<E>(self, value: bool) -> Result<Datum, E> {
        Ok(Datum(Value::Boolean(value)))
    }
    fn visit_i32<E>(self, value: i32) -> Result<Datum, E> {
        Ok(Datum(Value::Int(value)))
    }
    fn visit_i64<E>(self, value: i64) -> Result<Datum, E> {
        Ok(Datum(Value::Long(value)))
    }
    fn visit_f32<E>(self, value: f32) -> Result<Datum, E> {
        Ok(Datum(Value::Float(value)))
    }
    fn visit_f64<E>(self, value: f64) -> Result<Datum, E> {
        Ok(Datum(Value::Double(value)))
    }
    fn visit_string<E>(self, value: String) -> Result<Datum, E> {
        Ok(Datum(Value::String(value)))
    }
    fn visit_str<E>(self, value: &str) -> Result<Datum, E> {
        Ok(Datum(Value::String(value.to_owned())))
    }
    fn visit_byte_buf<E>(self, value: Vec<u8>) -> Result<Datum, E> {
        Ok(Datum(Value::Bytes(value)))
    }
    fn visit_bytes<E>(self, value: &[u8]) -> Result<Datum, E> {
        Ok(Datum(Value::Bytes(value.to_owned())))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Datum, A::Error> {
        let mut values = Vec::new();
        while let Some(Datum(value)) = seq.next_element()? {
            values.push(value);
        }
        Ok(Datum(Value::Array(values)))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Datum, A::Error> {
        let mut values = Vec::new();
        while let Some((Key(key), Datum(value))) = map.next_entry()? {
            values.push((key, value));
        }
        Ok(Datum(Value::Record(values)))
    }
}

pub fn materialize(
    value: Value,
    schema: &Schema,
    names: &NamesRef<'_>,
    unions: &mut impl Iterator<Item = u32>,
) -> Result<Value, String> {
    match (schema, value) {
        (Schema::Ref { name }, value) => materialize(
            value,
            names.get(name).ok_or("unresolved wire schema")?,
            names,
            unions,
        ),
        (Schema::Union(union), value) => {
            let index = unions.next().ok_or("missing union index")?;
            let branch = union
                .variants()
                .get(index as usize)
                .ok_or("invalid union index")?;
            Ok(Value::Union(
                index,
                Box::new(materialize(value, branch, names, unions)?),
            ))
        }
        (Schema::Record(record), Value::Record(values)) => values
            .into_iter()
            .zip(&record.fields)
            .map(|((key, value), field)| {
                Ok((key, materialize(value, &field.schema, names, unions)?))
            })
            .collect::<Result<_, _>>()
            .map(Value::Record),
        (Schema::Map(map), Value::Record(values)) => values
            .into_iter()
            .map(|(key, value)| Ok((key, materialize(value, &map.types, names, unions)?)))
            .collect::<Result<_, _>>()
            .map(Value::Record),
        (Schema::Array(array), Value::Array(values)) => values
            .into_iter()
            .map(|value| materialize(value, &array.items, names, unions))
            .collect::<Result<_, _>>()
            .map(Value::Array),
        (Schema::Enum(enumeration), Value::Record(mut values)) => {
            let Some((_, Value::Int(index))) = values.pop() else {
                return Err("invalid enum index".into());
            };
            let symbol = enumeration
                .symbols
                .get(index as usize)
                .ok_or("invalid enum index")?;
            Ok(Value::Enum(index as u32, symbol.clone()))
        }
        (schema, value) => crate::resolution::logical_value(value, schema),
    }
}
