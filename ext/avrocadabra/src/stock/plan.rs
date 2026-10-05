use magnus::{
    DataTypeFunctions, Error, Ruby, TypedData, Value, gc,
    prelude::*,
    rb_sys::{AsRawId, AsRawValue, FromRawValue, protect},
};
use rb_sys::VALUE;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

const LIMIT: usize = 128;

#[derive(Clone, Copy)]
pub enum Logical {
    IntDate,
    TimestampMillis,
    TimestampMicros,
    TimestampNanos,
}

#[derive(Clone, Copy)]
pub enum Adapter {
    Pending,
    Identity,
    Decimal { precision: i64, scale: i64 },
    Stock(Logical),
    Custom,
}

pub struct Text {
    pub value: VALUE,
    bytes: Vec<u8>,
    encoding: i32,
}

pub struct Field {
    pub object: VALUE,
    pub name: Text,
    pub symbol: VALUE,
    pub node: usize,
}

pub enum Kind {
    Null,
    Boolean,
    Int,
    Long,
    Float,
    Double,
    Bytes,
    Str,
    Fixed {
        size: VALUE,
        bytes: usize,
    },
    Enum {
        symbols: VALUE,
        names: Vec<Text>,
    },
    Array {
        items: usize,
    },
    Map {
        values: usize,
    },
    Record {
        fields: VALUE,
        list: Vec<Field>,
    },
    Union {
        schemas: VALUE,
        branches: Vec<usize>,
    },
}

pub struct Node {
    pub schema: VALUE,
    class: VALUE,
    type_sym: VALUE,
    logical: VALUE,
    pub adapter_value: VALUE,
    adapter_state: [VALUE; 3],
    pub adapter: Adapter,
    pub kind: Kind,
}

pub struct Plan {
    pub nodes: Vec<Node>,
    field: VALUE,
}

pub struct Env {
    schemas: [VALUE; 8],
    field: VALUE,
    logical: [VALUE; 5],
    bytes_decimal: VALUE,
}

use super::check::{
    IV_FACTOR, IV_FIELDS, IV_ITEMS, IV_LOGICAL_TYPE, IV_NAME, IV_PRECISION, IV_SCALE, IV_SCHEMAS,
    IV_SIZE, IV_SYMBOLS, IV_TYPE, IV_TYPE_ADAPTER, IV_TYPE_SYM, IV_VALUES, ivar,
};

fn class_of(value: VALUE) -> VALUE {
    super::check::class_raw(value)
}

fn constant(ruby: &Ruby, path: &[&str]) -> Result<VALUE, Error> {
    let mut value = ruby.class_object().as_raw();
    for name in path {
        let name = ruby.intern(name).as_raw();
        value = protect(|| unsafe { rb_sys::rb_const_get(value, name) })?;
    }
    Ok(value)
}

impl Env {
    pub fn new(ruby: &Ruby, pin: fn(VALUE) -> VALUE) -> Result<Self, Error> {
        let schema = |name| constant(ruby, &["Avro", "Schema", name]).map(pin);
        let logical = |name| constant(ruby, &["Avro", "LogicalTypes", name]).map(pin);
        Ok(Self {
            schemas: [
                schema("PrimitiveSchema")?,
                schema("BytesSchema")?,
                schema("RecordSchema")?,
                schema("ArraySchema")?,
                schema("MapSchema")?,
                schema("UnionSchema")?,
                schema("EnumSchema")?,
                schema("FixedSchema")?,
            ],
            field: schema("Field")?,
            logical: [
                logical("Identity")?,
                logical("IntDate")?,
                logical("TimestampMillis")?,
                logical("TimestampMicros")?,
                logical("TimestampNanos")?,
            ],
            bytes_decimal: logical("BytesDecimal")?,
        })
    }
}

impl Env {
    /// `type_adapter` memoizes on first write; until then it is `Pending`.
    pub fn classify(&self, adapter: VALUE) -> (Adapter, [VALUE; 3]) {
        if adapter == rb_sys::Qnil as VALUE {
            return (Adapter::Pending, [0; 3]);
        }
        if adapter == self.logical[0] {
            return (Adapter::Identity, [0; 3]);
        }
        let logical = [
            Logical::IntDate,
            Logical::TimestampMillis,
            Logical::TimestampMicros,
            Logical::TimestampNanos,
        ];
        if let Some(position) = self.logical[1..]
            .iter()
            .position(|&module| module == adapter)
        {
            return (
                Adapter::Stock(
                    logical
                        .into_iter()
                        .nth(position)
                        .expect("four logical modules"),
                ),
                [0; 3],
            );
        }
        if class_of(adapter) == self.bytes_decimal {
            let state = [
                ivar(adapter, &IV_PRECISION),
                ivar(adapter, &IV_SCALE),
                ivar(adapter, &IV_FACTOR),
            ];
            if let (Some(precision), Some(scale)) = (fixnum(state[0]), fixnum(state[1])) {
                return (Adapter::Decimal { precision, scale }, state);
            }
        }
        (Adapter::Custom, [0; 3])
    }
}

impl Text {
    fn new(value: VALUE) -> Option<Self> {
        let ruby = unsafe { Ruby::get_unchecked() };
        if class_of(value) != ruby.class_string().as_raw() {
            return None;
        }
        let string = magnus::RString::from_value(unsafe { Value::from_raw(value) })?;
        Some(Self {
            value,
            bytes: unsafe { string.as_slice() }.to_vec(),
            encoding: unsafe { rb_sys::rb_enc_get_index(value) },
        })
    }

    fn current(&self, value: VALUE) -> bool {
        if value != self.value
            || class_of(value) != unsafe { Ruby::get_unchecked() }.class_string().as_raw()
        {
            return false;
        }
        let string =
            unsafe { magnus::RString::from_value(Value::from_raw(value)).unwrap_unchecked() };
        (unsafe { string.as_slice() } == self.bytes)
            && unsafe { rb_sys::rb_enc_get_index(value) } == self.encoding
    }
}

fn array(value: VALUE) -> Option<Vec<VALUE>> {
    let ruby = unsafe { Ruby::get_unchecked() };
    if class_of(value) != ruby.class_array().as_raw() {
        return None;
    }
    let array = magnus::RArray::from_value(unsafe { Value::from_raw(value) })?;
    Some(
        unsafe { array.as_slice() }
            .iter()
            .map(|value| value.as_raw())
            .collect(),
    )
}

fn same(array: VALUE, items: impl ExactSizeIterator<Item = VALUE>) -> bool {
    let Some(current) = self::array(array) else {
        return false;
    };
    current.len() == items.len()
        && current
            .into_iter()
            .zip(items)
            .all(|(left, right)| left == right)
}

fn symbol_name(symbol: VALUE) -> Option<String> {
    magnus::Symbol::from_value(unsafe { Value::from_raw(symbol) })?;
    let name = unsafe { Value::from_raw(rb_sys::rb_sym2str(symbol)) };
    String::try_convert(name).ok()
}

impl Plan {
    pub fn build(env: &Env, root: VALUE) -> Result<Option<Self>, Error> {
        let mut plan = Plan {
            nodes: Vec::new(),
            field: env.field,
        };
        let mut index = HashMap::new();
        Ok(plan.node(env, root, &mut index)?.map(|_| plan))
    }

    fn node(
        &mut self,
        env: &Env,
        schema: VALUE,
        index: &mut HashMap<VALUE, usize>,
    ) -> Result<Option<usize>, Error> {
        if let Some(&position) = index.get(&schema) {
            return Ok(Some(position));
        }
        let class = class_of(schema);
        if !env.schemas.contains(&class) {
            return Ok(None);
        }
        let adapter_value = ivar(schema, &IV_TYPE_ADAPTER);
        let (adapter, adapter_state) = env.classify(adapter_value);
        let type_sym = ivar(schema, &IV_TYPE_SYM);
        let Some(name) = symbol_name(type_sym) else {
            return Ok(None);
        };
        let position = self.nodes.len();
        index.insert(schema, position);
        self.nodes.push(Node {
            schema,
            class,
            type_sym,
            logical: ivar(schema, &IV_LOGICAL_TYPE),
            adapter_value,
            adapter_state,
            adapter,
            kind: Kind::Null,
        });
        let kind = match name.as_str() {
            "null" => Kind::Null,
            "boolean" => Kind::Boolean,
            "int" => Kind::Int,
            "long" => Kind::Long,
            "float" => Kind::Float,
            "double" => Kind::Double,
            "bytes" => Kind::Bytes,
            "string" => Kind::Str,
            "fixed" => {
                let size = ivar(schema, &IV_SIZE);
                let Some(bytes) = fixnum(size).and_then(|size| usize::try_from(size).ok()) else {
                    return Ok(None);
                };
                Kind::Fixed { size, bytes }
            }
            "enum" => {
                let symbols = ivar(schema, &IV_SYMBOLS);
                let Some(items) = array(symbols) else {
                    return Ok(None);
                };
                let Some(names) = items.into_iter().map(Text::new).collect::<Option<Vec<_>>>()
                else {
                    return Ok(None);
                };
                Kind::Enum { symbols, names }
            }
            "array" => match self.node(env, ivar(schema, &IV_ITEMS), index)? {
                Some(items) => Kind::Array { items },
                None => return Ok(None),
            },
            "map" => match self.node(env, ivar(schema, &IV_VALUES), index)? {
                Some(values) => Kind::Map { values },
                None => return Ok(None),
            },
            "union" => {
                let schemas = ivar(schema, &IV_SCHEMAS);
                let Some(items) = array(schemas) else {
                    return Ok(None);
                };
                let mut branches = Vec::with_capacity(items.len());
                for item in items {
                    match self.node(env, item, index)? {
                        Some(branch) => branches.push(branch),
                        None => return Ok(None),
                    }
                }
                Kind::Union { schemas, branches }
            }
            "record" | "error" => {
                let fields = ivar(schema, &IV_FIELDS);
                let Some(items) = array(fields) else {
                    return Ok(None);
                };
                let mut list = Vec::with_capacity(items.len());
                for object in items {
                    if class_of(object) != env.field {
                        return Ok(None);
                    }
                    let Some(name) = Text::new(ivar(object, &IV_NAME)) else {
                        return Ok(None);
                    };
                    let Some(node) = self.node(env, ivar(object, &IV_TYPE), index)? else {
                        return Ok(None);
                    };
                    let symbol = protect(|| unsafe { rb_sys::rb_str_intern(name.value) })?;
                    list.push(Field {
                        object,
                        name,
                        symbol,
                        node,
                    });
                }
                Kind::Record { fields, list }
            }
            _ => return Ok(None),
        };
        self.nodes[position].kind = kind;
        Ok(Some(position))
    }

    pub fn current(&self, position: usize) -> bool {
        let node = &self.nodes[position];
        let schema = node.schema;
        if class_of(schema) != node.class
            || ivar(schema, &IV_TYPE_SYM) != node.type_sym
            || ivar(schema, &IV_LOGICAL_TYPE) != node.logical
            || ivar(schema, &IV_TYPE_ADAPTER) != node.adapter_value
        {
            return false;
        }
        if let Adapter::Decimal { .. } = node.adapter {
            let adapter = node.adapter_value;
            if [
                ivar(adapter, &IV_PRECISION),
                ivar(adapter, &IV_SCALE),
                ivar(adapter, &IV_FACTOR),
            ] != node.adapter_state
            {
                return false;
            }
        }
        let child = |index: usize| self.nodes[index].schema;
        match &node.kind {
            Kind::Fixed { size, .. } => ivar(schema, &IV_SIZE) == *size,
            Kind::Enum { symbols, names } => {
                ivar(schema, &IV_SYMBOLS) == *symbols
                    && same(*symbols, names.iter().map(|name| name.value))
                    && names.iter().all(|name| name.current(name.value))
            }
            Kind::Array { items } => ivar(schema, &IV_ITEMS) == child(*items),
            Kind::Map { values } => ivar(schema, &IV_VALUES) == child(*values),
            Kind::Union { schemas, branches } => {
                ivar(schema, &IV_SCHEMAS) == *schemas
                    && same(*schemas, branches.iter().map(|&branch| child(branch)))
            }
            Kind::Record { fields, list } => {
                ivar(schema, &IV_FIELDS) == *fields
                    && same(*fields, list.iter().map(|field| field.object))
                    && list.iter().all(|field| {
                        class_of(field.object) == self.field
                            && field.name.current(ivar(field.object, &IV_NAME))
                            && ivar(field.object, &IV_TYPE) == child(field.node)
                    })
            }
            _ => true,
        }
    }

    pub fn complete(&self) -> bool {
        (0..self.nodes.len()).all(|position| self.current(position))
    }

    fn values(&self) -> impl Iterator<Item = VALUE> + '_ {
        self.nodes.iter().flat_map(|node| {
            let mut values = vec![node.schema, node.type_sym, node.logical, node.adapter_value];
            values.extend(node.adapter_state);
            match &node.kind {
                Kind::Fixed { size, .. } => values.push(*size),
                Kind::Enum { symbols, names } => {
                    values.push(*symbols);
                    values.extend(names.iter().map(|name| name.value));
                }
                Kind::Union { schemas, .. } => values.push(*schemas),
                Kind::Record { fields, list } => {
                    values.push(*fields);
                    for field in list {
                        values.extend([field.object, field.name.value, field.symbol]);
                    }
                }
                _ => {}
            }
            values
        })
    }
}

fn fixnum(value: VALUE) -> Option<i64> {
    rb_sys::FIXNUM_P(value).then(|| unsafe { rb_sys::rb_num2long(value) } as i64)
}

#[derive(TypedData, Default)]
#[magnus(class = "Avrocadabra::NativeSchema::Plans", free_immediately, mark)]
pub struct Plans {
    entries: Mutex<Vec<(VALUE, Arc<Plan>)>>,
}

impl DataTypeFunctions for Plans {
    fn mark(&self, marker: &gc::Marker) {
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for (_, plan) in entries.iter() {
            for value in plan.values() {
                if !rb_sys::SPECIAL_CONST_P(value) {
                    marker.mark(unsafe { Value::from_raw(value) });
                }
            }
        }
    }
}

impl Plans {
    pub fn fetch(&self, env: &Env, schema: VALUE) -> Result<Option<Arc<Plan>>, Error> {
        let cached = {
            let entries = self
                .entries
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            entries
                .iter()
                .find(|entry| entry.0 == schema)
                .map(|entry| entry.1.clone())
        };
        if let Some(plan) = cached
            && plan.complete()
        {
            return Ok(Some(plan));
        }
        let Some(plan) = Plan::build(env, schema)? else {
            return Ok(None);
        };
        let plan = Arc::new(plan);
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        entries.retain(|entry| entry.0 != schema);
        if entries.len() >= LIMIT
            && let Some(position) = entries
                .iter()
                .position(|entry| Arc::strong_count(&entry.1) == 1)
        {
            entries.remove(position);
        }
        entries.push((schema, plan.clone()));
        Ok(Some(plan))
    }
}
