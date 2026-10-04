use apache_avro::{
    Schema,
    schema::{Alias, FixedSchema, InnerDecimalSchema, Name, RecordField, UuidSchema},
    types::Value,
};
use serde_json::Value as Json;
use std::collections::{BTreeMap, HashMap};

pub trait HeapSize {
    fn heap_size(&self) -> usize;
}

impl HeapSize for String {
    fn heap_size(&self) -> usize {
        self.capacity()
    }
}

impl<T: HeapSize> HeapSize for Vec<T> {
    fn heap_size(&self) -> usize {
        self.capacity() * size_of::<T>() + self.iter().map(HeapSize::heap_size).sum::<usize>()
    }
}

impl<T: HeapSize> HeapSize for Option<T> {
    fn heap_size(&self) -> usize {
        self.as_ref().map_or(0, HeapSize::heap_size)
    }
}

impl<T: HeapSize> HeapSize for Box<T> {
    fn heap_size(&self) -> usize {
        size_of::<T>() + (**self).heap_size()
    }
}

impl<T> HeapSize for &T {
    fn heap_size(&self) -> usize {
        0
    }
}

impl HeapSize for usize {
    fn heap_size(&self) -> usize {
        0
    }
}

impl<K: HeapSize, V: HeapSize> HeapSize for HashMap<K, V> {
    fn heap_size(&self) -> usize {
        // Account for spare hash buckets and control bytes as well as entries.
        let buckets = if self.capacity() == 0 {
            0
        } else {
            (self.capacity() + 1).next_power_of_two()
        };
        buckets * (size_of::<(K, V)>() + 1)
            + self
                .iter()
                .map(|(key, value)| key.heap_size() + value.heap_size())
                .sum::<usize>()
    }
}

impl<K: HeapSize, V: HeapSize> HeapSize for BTreeMap<K, V> {
    fn heap_size(&self) -> usize {
        tree_storage::<K, V>(self.len())
            + self
                .iter()
                .map(|(key, value)| key.heap_size() + value.heap_size())
                .sum::<usize>()
    }
}

pub fn tree_storage<K, V>(length: usize) -> usize {
    // Rust does not expose B-tree node capacities; include spare entries and links.
    length.div_ceil(6) * (11 * size_of::<(K, V)>() + 15 * size_of::<usize>())
}

impl HeapSize for Json {
    fn heap_size(&self) -> usize {
        match self {
            Self::String(value) => value.heap_size(),
            Self::Array(values) => values.heap_size(),
            Self::Object(values) => {
                tree_storage::<String, Json>(values.len())
                    + values
                        .iter()
                        .map(|(key, value)| key.heap_size() + value.heap_size())
                        .sum::<usize>()
            }
            _ => 0,
        }
    }
}

impl HeapSize for Name {
    fn heap_size(&self) -> usize {
        self.name().len() + self.namespace().map_or(0, |namespace| namespace.len() + 1)
    }
}

impl HeapSize for Alias {
    fn heap_size(&self) -> usize {
        self.name().len() + self.namespace().map_or(0, |namespace| namespace.len() + 1)
    }
}

impl HeapSize for FixedSchema {
    fn heap_size(&self) -> usize {
        self.name.heap_size()
            + self.aliases.heap_size()
            + self.doc.heap_size()
            + self.attributes.heap_size()
    }
}

impl HeapSize for RecordField {
    fn heap_size(&self) -> usize {
        self.name.heap_size()
            + self.doc.heap_size()
            + self.aliases.heap_size()
            + self.default.heap_size()
            + self.schema.heap_size()
            + self.custom_attributes.heap_size()
    }
}

impl HeapSize for Schema {
    fn heap_size(&self) -> usize {
        match self {
            Self::Record(record) => {
                record.name.heap_size()
                    + record.aliases.heap_size()
                    + record.doc.heap_size()
                    + record.fields.heap_size()
                    + record.lookup.heap_size()
                    + record.attributes.heap_size()
            }
            Self::Enum(value) => {
                value.name.heap_size()
                    + value.aliases.heap_size()
                    + value.doc.heap_size()
                    + value.symbols.heap_size()
                    + value.default.heap_size()
                    + value.attributes.heap_size()
            }
            Self::Fixed(value) | Self::Duration(value) | Self::Uuid(UuidSchema::Fixed(value)) => {
                value.heap_size()
            }
            Self::Decimal(value) => match &value.inner {
                InnerDecimalSchema::Fixed(fixed) => fixed.heap_size(),
                InnerDecimalSchema::Bytes => 0,
            },
            Self::Array(value) => value.items.heap_size() + value.attributes.heap_size(),
            Self::Map(value) => value.types.heap_size() + value.attributes.heap_size(),
            Self::Union(value) => {
                let variants = value.variants();
                variants.len() * (size_of::<Schema>() + size_of::<usize>())
                    + tree_storage::<apache_avro::schema::SchemaKind, usize>(variants.len())
                    + variants.iter().map(HeapSize::heap_size).sum::<usize>()
            }
            Self::Ref { name } => name.heap_size(),
            _ => 0,
        }
    }
}

impl HeapSize for Value {
    fn heap_size(&self) -> usize {
        match self {
            Self::String(value) | Self::Enum(_, value) => value.heap_size(),
            Self::Bytes(value) | Self::Fixed(_, value) => value.capacity(),
            Self::Array(values) => values.heap_size(),
            Self::Map(values) => values.heap_size(),
            Self::Record(values) => {
                values.capacity() * size_of::<(String, Value)>()
                    + values
                        .iter()
                        .map(|(name, value)| name.heap_size() + value.heap_size())
                        .sum::<usize>()
            }
            Self::Union(_, value) => value.heap_size(),
            Self::Decimal(value) => {
                let value = apache_avro::BigDecimal::new(value.clone().into(), 0);
                (value.as_bigint_and_scale().0.bits().div_ceil(64) * 8) as usize
            }
            Self::BigDecimal(value) => {
                (value.as_bigint_and_scale().0.bits().div_ceil(64) * 8) as usize
            }
            _ => 0,
        }
    }
}
