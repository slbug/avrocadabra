use crate::memory::HeapSize;
use apache_avro::{
    Decimal, Duration, Schema,
    schema::{Aliases, FixedSchema, InnerDecimalSchema, Name, NamesRef, UuidSchema},
    types::Value,
};
use serde_json::Value as Json;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub struct Resolution {
    nodes: Vec<Node>,
    root: usize,
    limits: Limits,
    memory_size: usize,
}

#[derive(Debug)]
pub struct Resolved {
    pub value: Value,
    pub defaults: Vec<(usize, Arc<Json>)>,
    pub adapters: Vec<(usize, usize)>,
    pub failure: Option<(usize, String)>,
}

enum Node {
    Error(String),
    Scalar {
        writer: Logical,
        reader: Logical,
        from: Scalar,
        to: Scalar,
    },
    Record(Vec<Field>),
    Enum {
        indices: Vec<Option<u32>>,
        symbols: Vec<String>,
    },
    Array(usize),
    Map(usize),
    WriterUnion(Vec<usize>),
    ReaderUnion(u32, usize),
}

struct Field {
    name: String,
    source: FieldSource,
}

enum FieldSource {
    Writer {
        index: usize,
        name: String,
        node: usize,
    },
    Default {
        value: Arc<Json>,
        usage: Usage,
    },
    Missing,
}

impl HeapSize for Field {
    fn heap_size(&self) -> usize {
        self.name.heap_size()
            + match &self.source {
                FieldSource::Writer { name, .. } => name.heap_size(),
                FieldSource::Default { value, .. } => {
                    size_of::<Json>() + 2 * size_of::<usize>() + Json::heap_size(value)
                }
                FieldSource::Missing => 0,
            }
    }
}

impl HeapSize for Node {
    fn heap_size(&self) -> usize {
        match self {
            Self::Error(error) => error.heap_size(),
            Self::Record(fields) => fields.heap_size(),
            Self::Enum { indices, symbols } => {
                indices.capacity() * size_of::<Option<u32>>() + symbols.heap_size()
            }
            Self::WriterUnion(branches) => branches.heap_size(),
            _ => 0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scalar {
    Null,
    Boolean,
    Int,
    Long,
    Float,
    Double,
    Bytes,
    String,
    Fixed(usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Logical {
    Plain,
    Decimal { precision: usize },
    BigDecimal,
    Uuid,
    Date,
    TimeMillis,
    TimeMicros,
    TimestampMillis,
    TimestampMicros,
    TimestampNanos,
    LocalTimestampMillis,
    LocalTimestampMicros,
    LocalTimestampNanos,
    Duration,
}

#[derive(Clone, Copy)]
struct Limits {
    depth: usize,
    items: usize,
    bytes: usize,
}

#[derive(Clone, Copy, Default)]
struct Usage {
    depth: usize,
    items: usize,
    bytes: usize,
}

struct Budget {
    limits: Limits,
    usage: Usage,
    exhausted: bool,
    defaults: Vec<(usize, Arc<Json>)>,
    adapters: Vec<(usize, usize)>,
    field_position: usize,
    read_position: usize,
    pending_adapters: usize,
    failure: Option<(usize, String)>,
}

fn field_default(field: &apache_avro::schema::RecordField) -> Option<&Json> {
    field.default.as_ref().or_else(|| match &field.schema {
        Schema::Null => Some(&Json::Null),
        Schema::Union(union) if union.variants().iter().any(|s| matches!(s, Schema::Null)) => {
            Some(&Json::Null)
        }
        _ => None,
    })
}

pub(crate) fn validate_defaults<'a>(
    schemas: &'a [Schema],
    names: &NamesRef<'a>,
    limits: crate::guard::Limits,
) -> Result<(), String> {
    let mut budget = Budget::new(Limits {
        depth: limits.max_depth,
        items: limits.max_items,
        bytes: limits.max_bytes,
    });
    let mut pending: Vec<_> = schemas.iter().collect();
    while let Some(schema) = pending.pop() {
        match schema {
            Schema::Record(record) => {
                for field in &record.fields {
                    if let Some(default) = field.default.as_ref() {
                        let path = format!("$.{}", field.name);
                        let mut field_budget = Budget::new(budget.limits);
                        default_value(default, &field.schema, names, 0, &path, &mut field_budget)?;
                        budget.add(field_budget.usage, &path)?;
                    }
                    pending.push(&field.schema);
                }
            }
            Schema::Array(array) => pending.push(&array.items),
            Schema::Map(map) => pending.push(&map.types),
            Schema::Union(union) => pending.extend(union.variants()),
            _ => {}
        }
    }
    Ok(())
}

impl Budget {
    fn new(limits: Limits) -> Self {
        Self {
            limits,
            usage: Usage::default(),
            exhausted: false,
            defaults: Vec::new(),
            adapters: Vec::new(),
            field_position: 0,
            read_position: 0,
            pending_adapters: 0,
            failure: None,
        }
    }

    fn adapters(&mut self, count: usize) {
        let count = count + std::mem::take(&mut self.pending_adapters);
        if count != 1 {
            self.adapters.push((self.read_position, count));
        }
        self.read_position += 1;
    }

    fn node(&mut self, depth: usize, path: &str) -> Result<(), String> {
        self.add(
            Usage {
                depth,
                items: 1,
                bytes: 0,
            },
            path,
        )
    }

    fn bytes(&mut self, bytes: usize, path: &str) -> Result<(), String> {
        self.add(
            Usage {
                bytes,
                ..Usage::default()
            },
            path,
        )
    }

    fn add(&mut self, usage: Usage, path: &str) -> Result<(), String> {
        if usage.depth > self.limits.depth {
            self.exhausted = true;
            return Err(format!("{path}: resolution exceeds maximum depth"));
        }
        self.usage.depth = self.usage.depth.max(usage.depth);
        self.usage.items = self
            .usage
            .items
            .checked_add(usage.items)
            .filter(|&n| n <= self.limits.items)
            .ok_or_else(|| {
                self.exhausted = true;
                format!("{path}: resolution exceeds maximum item count")
            })?;
        self.usage.bytes = self
            .usage
            .bytes
            .checked_add(usage.bytes)
            .filter(|&n| n <= self.limits.bytes)
            .ok_or_else(|| {
                self.exhausted = true;
                format!("{path}: resolution exceeds maximum byte count")
            })?;
        Ok(())
    }
}

impl Resolution {
    pub fn new(
        writer: &Schema,
        writer_names: &NamesRef<'_>,
        reader: &Schema,
        reader_names: &NamesRef<'_>,
        max_depth: usize,
        max_items: usize,
        max_bytes: usize,
    ) -> Result<Self, String> {
        let limits = Limits {
            depth: max_depth,
            items: max_items,
            bytes: max_bytes,
        };
        let mut builder = Builder {
            writer_names,
            reader_names,
            nodes: Vec::new(),
            pairs: HashMap::new(),
            limits,
            defaults: Budget::new(limits),
        };
        let root = builder.compile(writer, reader, 0)?;
        let memory_size = size_of::<Self>() + 2 * size_of::<usize>() + builder.nodes.heap_size();
        Ok(Self {
            nodes: builder.nodes,
            root,
            limits,
            memory_size,
        })
    }

    pub fn memory_size(&self) -> usize {
        self.memory_size
    }

    pub fn apply(&self, value: Value) -> Result<Resolved, String> {
        let mut budget = Budget::new(self.limits);
        let value = self.resolve(self.root, value, 0, "$", &mut budget)?;
        Ok(Resolved {
            value,
            defaults: budget.defaults,
            adapters: budget.adapters,
            failure: budget.failure,
        })
    }

    fn resolve(
        &self,
        node: usize,
        value: Value,
        depth: usize,
        path: &str,
        budget: &mut Budget,
    ) -> Result<Value, String> {
        if budget.failure.is_some() {
            return Ok(Value::Null);
        }
        let position = budget.read_position;
        match self.resolve_value(node, value, depth, path, budget) {
            Err(error) if position > 0 && !budget.exhausted => {
                budget.failure = Some((position, error));
                Ok(Value::Null)
            }
            result => result,
        }
    }

    fn resolve_value(
        &self,
        node: usize,
        value: Value,
        depth: usize,
        path: &str,
        budget: &mut Budget,
    ) -> Result<Value, String> {
        match &self.nodes[node] {
            Node::Error(error) => return Err(format!("{path}: {error}")),
            Node::WriterUnion(branches) => {
                let Value::Union(index, value) = value else {
                    return Err(format!("{path}: expected writer union"));
                };
                let node = branches
                    .get(index as usize)
                    .ok_or_else(|| format!("{path}: invalid writer union index {index}"))?;
                budget.pending_adapters += 1;
                return self.resolve(*node, *value, depth, path, budget);
            }
            Node::ReaderUnion(index, child) => {
                budget.node(depth, path)?;
                budget.adapters(0);
                return self
                    .resolve(*child, value, depth, path, budget)
                    .map(|value| Value::Union(*index, Box::new(value)));
            }
            _ => {}
        }
        budget.node(depth, path)?;
        budget.adapters(1);
        match (&self.nodes[node], value) {
            (
                Node::Scalar {
                    writer,
                    reader,
                    from,
                    to,
                },
                value,
            ) => {
                let plain = strip_logical(value, *writer, *from)
                    .map_err(|error| format!("{path}: {error}"))?;
                let promoted = if *reader == Logical::Plain
                    && matches!(
                        (*from, *to),
                        (Scalar::Int | Scalar::Long, Scalar::Float | Scalar::Double)
                            | (Scalar::String, Scalar::Bytes)
                            | (Scalar::Bytes, Scalar::String)
                    ) {
                    plain
                } else {
                    promote(plain, *from, *to).map_err(|error| format!("{path}: {error}"))?
                };
                budget.bytes(value_bytes(&promoted), path)?;
                apply_logical(promoted, *reader).map_err(|error| format!("{path}: {error}"))
            }
            (Node::Record(fields), Value::Record(mut values)) => {
                if fields.len() > budget.limits.items.saturating_sub(budget.usage.items) {
                    return Err(format!("{path}: resolution exceeds maximum item count"));
                }
                let mut output = Vec::with_capacity(fields.len());
                for field in fields {
                    let position = budget.field_position;
                    budget.field_position += 1;
                    let field_path = format!("{path}.{}", field.name);
                    budget.bytes(field.name.len(), &field_path)?;
                    let value = match &field.source {
                        FieldSource::Writer { index, name, node } => {
                            let (actual, value) = values
                                .get_mut(*index)
                                .ok_or_else(|| format!("{field_path}: missing writer field"))?;
                            if actual != name {
                                return Err(format!(
                                    "{field_path}: unexpected writer field {actual}"
                                ));
                            }
                            self.resolve(
                                *node,
                                std::mem::replace(value, Value::Null),
                                depth + 1,
                                &field_path,
                                budget,
                            )?
                        }
                        FieldSource::Default { value, usage } => {
                            let mut usage = *usage;
                            usage.depth = usage
                                .depth
                                .checked_add(depth + 1)
                                .ok_or_else(|| format!("{field_path}: default depth overflow"))?;
                            budget.add(usage, &field_path)?;
                            budget.defaults.push((position, Arc::clone(value)));
                            Value::Null
                        }
                        FieldSource::Missing => {
                            budget.failure.get_or_insert_with(|| (budget.read_position, format!(
                                "{field_path}: reader field is absent from writer schema and has no default"
                            )));
                            Value::Null
                        }
                    };
                    output.push((field.name.clone(), value));
                }
                Ok(Value::Record(output))
            }
            (Node::Enum { indices, symbols }, Value::Enum(index, symbol)) => {
                let resolved = indices
                    .get(index as usize)
                    .copied()
                    .ok_or_else(|| format!("{path}: invalid writer enum index"))?;
                let (index, symbol) = match resolved {
                    Some(index) => (index, symbols[index as usize].clone()),
                    None => (index, symbol),
                };
                budget.bytes(symbol.len(), path)?;
                Ok(Value::Enum(index, symbol))
            }
            (Node::Array(child), Value::Array(values)) => {
                if values.len() > budget.limits.items.saturating_sub(budget.usage.items) {
                    return Err(format!("{path}: resolution exceeds maximum item count"));
                }
                values
                    .into_iter()
                    .enumerate()
                    .map(|(index, value)| {
                        self.resolve(
                            *child,
                            value,
                            depth + 1,
                            &format!("{path}[{index}]"),
                            budget,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(Value::Array)
            }
            (Node::Map(child), Value::Record(values)) => {
                if values.len() > budget.limits.items.saturating_sub(budget.usage.items) {
                    return Err(format!("{path}: resolution exceeds maximum item count"));
                }
                values
                    .into_iter()
                    .map(|(key, value)| {
                        budget.bytes(key.len(), path)?;
                        let value = self.resolve(
                            *child,
                            value,
                            depth + 1,
                            &format!("{path}[{key:?}]"),
                            budget,
                        )?;
                        Ok((key, value))
                    })
                    .collect::<Result<Vec<_>, String>>()
                    .map(Value::Record)
            }
            _ => Err(format!(
                "{path}: decoded value does not match writer schema"
            )),
        }
    }
}

struct Builder<'a, 'b> {
    writer_names: &'a NamesRef<'b>,
    reader_names: &'a NamesRef<'b>,
    nodes: Vec<Node>,
    pairs: HashMap<(usize, usize), usize>,
    limits: Limits,
    defaults: Budget,
}

impl<'b> Builder<'_, 'b> {
    fn compile(
        &mut self,
        writer: &'b Schema,
        reader: &'b Schema,
        depth: usize,
    ) -> Result<usize, String> {
        let writer = dereference(writer, self.writer_names)?;
        let reader = dereference(reader, self.reader_names)?;
        let pair = (
            std::ptr::from_ref(writer) as usize,
            std::ptr::from_ref(reader) as usize,
        );
        if let Some(&node) = self.pairs.get(&pair) {
            return Ok(node);
        }
        if depth > self.limits.depth || self.nodes.len() >= self.limits.items {
            return Err("$: schema resolution plan exceeds configured limits".into());
        }
        let index = self.nodes.len();
        self.nodes
            .push(Node::Error("incomplete resolution plan".into()));
        self.pairs.insert(pair, index);
        let node = self.build(writer, reader, depth)?;
        self.nodes[index] = node;
        Ok(index)
    }

    fn build(
        &mut self,
        writer: &'b Schema,
        reader: &'b Schema,
        depth: usize,
    ) -> Result<Node, String> {
        if let Schema::Union(union) = writer {
            if union.variants().len() > self.limits.items {
                return Err("$: writer union exceeds maximum item count".into());
            }
            return union
                .variants()
                .iter()
                .map(|branch| self.compile(branch, reader, depth + 1))
                .collect::<Result<Vec<_>, _>>()
                .map(Node::WriterUnion);
        }
        if let Schema::Union(union) = reader {
            for (index, branch) in union.variants().iter().enumerate() {
                if self.matches(writer, branch, depth + 1)? {
                    return Ok(Node::ReaderUnion(
                        index as u32,
                        self.compile(writer, branch, depth + 1)?,
                    ));
                }
            }
            return Ok(Node::Error(format!(
                "writer {writer} has no matching reader union branch"
            )));
        }
        if !self.matches(writer, reader, depth)? {
            return Ok(Node::Error(format!(
                "writer {writer} cannot resolve to reader {reader}"
            )));
        }
        match (writer, reader) {
            (Schema::Record(writer), Schema::Record(reader)) => {
                if reader.fields.len() > self.limits.items {
                    return Err("$: reader record exceeds maximum item count".into());
                }
                let mut used = HashSet::new();
                let aliases: HashMap<_, _> = reader
                    .fields
                    .iter()
                    .flat_map(|field| {
                        field
                            .aliases
                            .iter()
                            .map(move |alias| (alias.as_str(), field))
                    })
                    .collect();
                let reader_fields: HashMap<_, _> = reader
                    .fields
                    .iter()
                    .map(|field| (field.name.as_str(), field))
                    .collect();
                let mut fields = Vec::with_capacity(reader.fields.len());
                for (index, written) in writer.fields.iter().enumerate() {
                    let Some(field) = reader_fields
                        .get(written.name.as_str())
                        .or_else(|| aliases.get(written.name.as_str()))
                    else {
                        continue;
                    };
                    used.insert(field.name.as_str());
                    self.defaults.bytes(written.name.len(), &field.name)?;
                    self.defaults.bytes(field.name.len(), &field.name)?;
                    fields.push(Field {
                        name: field.name.clone(),
                        source: FieldSource::Writer {
                            index,
                            name: written.name.clone(),
                            node: self.compile(&written.schema, &field.schema, depth + 1)?,
                        },
                    });
                }
                for field in &reader.fields {
                    if used.contains(field.name.as_str()) {
                        continue;
                    }
                    let source = if let Some(default) = field.default.as_ref() {
                        let path = format!("$.{}", field.name);
                        let mut budget = Budget::new(self.limits);
                        default_value(
                            default,
                            &field.schema,
                            self.reader_names,
                            0,
                            &path,
                            &mut budget,
                        )?;
                        self.defaults.add(budget.usage, &path)?;
                        FieldSource::Default {
                            value: Arc::new(default.clone()),
                            usage: budget.usage,
                        }
                    } else {
                        FieldSource::Missing
                    };
                    self.defaults.bytes(field.name.len(), &field.name)?;
                    fields.push(Field {
                        name: field.name.clone(),
                        source,
                    });
                }
                Ok(Node::Record(fields))
            }
            (Schema::Enum(writer), Schema::Enum(reader)) => {
                if writer.symbols.len() > self.limits.items
                    || reader.symbols.len() > self.limits.items
                {
                    return Err("$: enum exceeds maximum item count".into());
                }
                for symbol in &reader.symbols {
                    self.defaults.bytes(symbol.len(), "$")?;
                }
                let reader_symbols: HashMap<_, _> = reader
                    .symbols
                    .iter()
                    .enumerate()
                    .map(|(index, symbol)| (symbol.as_str(), index as u32))
                    .collect();
                let default = reader
                    .default
                    .as_ref()
                    .and_then(|symbol| reader_symbols.get(symbol.as_str()))
                    .copied();
                let indices = writer
                    .symbols
                    .iter()
                    .map(|symbol| reader_symbols.get(symbol.as_str()).copied().or(default))
                    .collect();
                Ok(Node::Enum {
                    indices,
                    symbols: reader.symbols.clone(),
                })
            }
            (Schema::Array(writer), Schema::Array(reader)) => Ok(Node::Array(self.compile(
                &writer.items,
                &reader.items,
                depth + 1,
            )?)),
            (Schema::Map(writer), Schema::Map(reader)) => Ok(Node::Map(self.compile(
                &writer.types,
                &reader.types,
                depth + 1,
            )?)),
            _ => {
                let Some((from, writer)) = scalar(writer) else {
                    return Ok(Node::Error("unsupported writer logical type".into()));
                };
                let Some((to, reader)) = scalar(reader) else {
                    return Ok(Node::Error("unsupported reader logical type".into()));
                };
                Ok(Node::Scalar {
                    writer,
                    reader,
                    from,
                    to,
                })
            }
        }
    }

    fn matches(
        &self,
        writer: &'b Schema,
        reader: &'b Schema,
        depth: usize,
    ) -> Result<bool, String> {
        if depth > self.limits.depth {
            return Err("$: schema matching exceeds maximum depth".into());
        }
        let writer = dereference(writer, self.writer_names)?;
        let reader = dereference(reader, self.reader_names)?;
        Ok(match (writer, reader) {
            (Schema::Union(_), _) | (_, Schema::Union(_)) => true,
            (Schema::Record(writer), Schema::Record(reader)) => {
                writer.attributes.get("type") == reader.attributes.get("type")
                    && names_match(&writer.name, &reader.name, &reader.aliases)
            }
            (Schema::Enum(writer), Schema::Enum(reader)) => {
                names_match(&writer.name, &reader.name, &reader.aliases)
            }
            (Schema::Array(writer), Schema::Array(reader)) => {
                self.matches(&writer.items, &reader.items, depth + 1)?
            }
            (Schema::Map(writer), Schema::Map(reader)) => {
                self.matches(&writer.types, &reader.types, depth + 1)?
            }
            _ => {
                if let (Schema::Decimal(writer), Schema::Decimal(reader)) = (writer, reader)
                    && (writer.scale != reader.scale || writer.precision != reader.precision)
                {
                    return Ok(false);
                }
                if let (Some(writer), Some(reader)) = (fixed(writer), fixed(reader)) {
                    return Ok(writer.size == reader.size
                        && names_match(&writer.name, &reader.name, &reader.aliases));
                }
                match (scalar(writer), scalar(reader)) {
                    (Some((from, _)), Some((to, _))) => scalar_matches(from, to),
                    _ => false,
                }
            }
        })
    }
}

pub(crate) fn dereference<'a>(
    mut schema: &'a Schema,
    names: &NamesRef<'a>,
) -> Result<&'a Schema, String> {
    let mut hops = 0;
    while let Schema::Ref { name } = schema {
        if hops > names.len() {
            return Err(format!("$: cyclic unresolved schema reference {name}"));
        }
        schema = names
            .get(name)
            .copied()
            .ok_or_else(|| format!("$: unresolved schema reference {name}"))?;
        hops += 1;
    }
    Ok(schema)
}

fn names_match(writer: &Name, reader: &Name, aliases: &Aliases) -> bool {
    writer == reader
        || aliases.as_ref().is_some_and(|aliases| {
            aliases
                .iter()
                .any(|alias| alias.fully_qualified_name(reader.namespace()).as_ref() == writer)
        })
}

fn fixed(schema: &Schema) -> Option<&FixedSchema> {
    match schema {
        Schema::Fixed(fixed) | Schema::Duration(fixed) | Schema::Uuid(UuidSchema::Fixed(fixed)) => {
            Some(fixed)
        }
        Schema::Decimal(decimal) => match &decimal.inner {
            InnerDecimalSchema::Fixed(fixed) => Some(fixed),
            InnerDecimalSchema::Bytes => None,
        },
        _ => None,
    }
}

fn scalar(schema: &Schema) -> Option<(Scalar, Logical)> {
    Some(match schema {
        Schema::Null => (Scalar::Null, Logical::Plain),
        Schema::Boolean => (Scalar::Boolean, Logical::Plain),
        Schema::Int => (Scalar::Int, Logical::Plain),
        Schema::Long => (Scalar::Long, Logical::Plain),
        Schema::Float => (Scalar::Float, Logical::Plain),
        Schema::Double => (Scalar::Double, Logical::Plain),
        Schema::Bytes => (Scalar::Bytes, Logical::Plain),
        Schema::String => (Scalar::String, Logical::Plain),
        Schema::Fixed(fixed) => (Scalar::Fixed(fixed.size), Logical::Plain),
        Schema::Decimal(decimal) => (
            match &decimal.inner {
                InnerDecimalSchema::Bytes => Scalar::Bytes,
                InnerDecimalSchema::Fixed(fixed) => Scalar::Fixed(fixed.size),
            },
            Logical::Decimal {
                precision: decimal.precision,
            },
        ),
        Schema::BigDecimal => (Scalar::Bytes, Logical::BigDecimal),
        Schema::Uuid(inner) => (
            match inner {
                UuidSchema::String => Scalar::String,
                UuidSchema::Bytes => Scalar::Bytes,
                UuidSchema::Fixed(fixed) => Scalar::Fixed(fixed.size),
            },
            Logical::Uuid,
        ),
        Schema::Date => (Scalar::Int, Logical::Date),
        Schema::TimeMillis => (Scalar::Int, Logical::TimeMillis),
        Schema::TimeMicros => (Scalar::Long, Logical::TimeMicros),
        Schema::TimestampMillis => (Scalar::Long, Logical::TimestampMillis),
        Schema::TimestampMicros => (Scalar::Long, Logical::TimestampMicros),
        Schema::TimestampNanos => (Scalar::Long, Logical::TimestampNanos),
        Schema::LocalTimestampMillis => (Scalar::Long, Logical::LocalTimestampMillis),
        Schema::LocalTimestampMicros => (Scalar::Long, Logical::LocalTimestampMicros),
        Schema::LocalTimestampNanos => (Scalar::Long, Logical::LocalTimestampNanos),
        Schema::Duration(fixed) => (Scalar::Fixed(fixed.size), Logical::Duration),
        _ => return None,
    })
}

fn scalar_matches(from: Scalar, to: Scalar) -> bool {
    if matches!(from, Scalar::Fixed(_)) || matches!(to, Scalar::Fixed(_)) {
        return false; // Named fixed matching is handled before primitive matching.
    }
    from == to
        || matches!(
            (from, to),
            (Scalar::Int, Scalar::Long | Scalar::Float | Scalar::Double)
                | (Scalar::Long, Scalar::Float | Scalar::Double)
                | (Scalar::Float, Scalar::Double)
                | (Scalar::String, Scalar::Bytes)
                | (Scalar::Bytes, Scalar::String)
        )
}

fn promote(value: Value, from: Scalar, to: Scalar) -> Result<Value, String> {
    Ok(match (from, to, value) {
        (Scalar::Null, Scalar::Null, Value::Null) => Value::Null,
        (Scalar::Boolean, Scalar::Boolean, value @ Value::Boolean(_)) => value,
        (Scalar::Int, Scalar::Int, value @ Value::Int(_)) => value,
        (Scalar::Int, Scalar::Long, Value::Int(value)) => Value::Long(i64::from(value)),
        (Scalar::Int, Scalar::Float, Value::Int(value)) => Value::Float(value as f32),
        (Scalar::Int, Scalar::Double, Value::Int(value)) => Value::Double(f64::from(value)),
        (Scalar::Long, Scalar::Long, value @ Value::Long(_)) => value,
        (Scalar::Long, Scalar::Float, Value::Long(value)) => Value::Float(value as f32),
        (Scalar::Long, Scalar::Double, Value::Long(value)) => Value::Double(value as f64),
        (Scalar::Float, Scalar::Float, value @ Value::Float(_)) => value,
        (Scalar::Float, Scalar::Double, Value::Float(value)) => Value::Double(f64::from(value)),
        (Scalar::Double, Scalar::Double, value @ Value::Double(_)) => value,
        (Scalar::Bytes, Scalar::Bytes, value @ Value::Bytes(_)) => value,
        (Scalar::String, Scalar::String, value @ Value::String(_)) => value,
        (Scalar::String, Scalar::Bytes, Value::String(value)) => Value::Bytes(value.into_bytes()),
        (Scalar::Bytes, Scalar::String, Value::Bytes(value)) => Value::String(
            String::from_utf8(value).map_err(|_| "bytes promoted to string are not valid UTF-8")?,
        ),
        (Scalar::Fixed(writer), Scalar::Fixed(reader), Value::Fixed(size, bytes))
            if writer == reader && size == writer && bytes.len() == size =>
        {
            Value::Fixed(size, bytes)
        }
        _ => return Err("decoded value does not match writer scalar".into()),
    })
}

fn strip_logical(value: Value, logical: Logical, physical: Scalar) -> Result<Value, String> {
    Ok(match (logical, value) {
        (Logical::Plain, value) => value,
        (Logical::Decimal { precision }, Value::Decimal(decimal)) => {
            let bytes = Vec::<u8>::try_from(decimal).map_err(|error| error.to_string())?;
            validate_decimal(&bytes, precision)?;
            match physical {
                Scalar::Fixed(size) => Value::Fixed(size, bytes),
                _ => Value::Bytes(bytes),
            }
        }
        (Logical::BigDecimal, Value::BigDecimal(decimal)) => {
            Value::Bytes(crate::big_decimal::encode(&decimal)?)
        }
        (Logical::Uuid, Value::Uuid(uuid)) => match physical {
            Scalar::String => Value::String(uuid.hyphenated().to_string()),
            Scalar::Fixed(size) => Value::Fixed(size, uuid.as_bytes().to_vec()),
            _ => Value::Bytes(uuid.as_bytes().to_vec()),
        },
        (Logical::Date, Value::Date(value)) => Value::Int(value),
        (Logical::TimeMillis, Value::TimeMillis(value)) => {
            validate_time(i64::from(value), 86_400_000)?;
            Value::Int(value)
        }
        (Logical::TimeMicros, Value::TimeMicros(value)) => {
            validate_time(value, 86_400_000_000)?;
            Value::Long(value)
        }
        (Logical::TimestampMillis, Value::TimestampMillis(value))
        | (Logical::TimestampMicros, Value::TimestampMicros(value))
        | (Logical::TimestampNanos, Value::TimestampNanos(value))
        | (Logical::LocalTimestampMillis, Value::LocalTimestampMillis(value))
        | (Logical::LocalTimestampMicros, Value::LocalTimestampMicros(value))
        | (Logical::LocalTimestampNanos, Value::LocalTimestampNanos(value)) => Value::Long(value),
        (Logical::Duration, Value::Duration(duration)) => {
            Value::Fixed(12, <[u8; 12]>::from(duration).to_vec())
        }
        _ => return Err("decoded value does not match writer logical type".into()),
    })
}

fn apply_logical(value: Value, logical: Logical) -> Result<Value, String> {
    Ok(match (logical, value) {
        (Logical::Plain, value) => value,
        (Logical::Decimal { precision }, Value::Bytes(bytes) | Value::Fixed(_, bytes)) => {
            validate_decimal(&bytes, precision)?;
            Value::Decimal(Decimal::from(bytes))
        }
        (Logical::BigDecimal, Value::Bytes(bytes)) => {
            Value::BigDecimal(crate::big_decimal::decode(&bytes)?)
        }
        (Logical::Uuid, Value::String(string)) => Value::Uuid(
            uuid::Uuid::parse_str(&string).map_err(|error| format!("invalid UUID: {error}"))?,
        ),
        (Logical::Uuid, Value::Bytes(bytes) | Value::Fixed(_, bytes)) => Value::Uuid(
            uuid::Uuid::from_slice(&bytes).map_err(|error| format!("invalid UUID: {error}"))?,
        ),
        (Logical::Date, Value::Int(value)) => Value::Date(value),
        (Logical::TimeMillis, Value::Int(value)) => {
            validate_time(i64::from(value), 86_400_000)?;
            Value::TimeMillis(value)
        }
        (Logical::TimeMicros, Value::Long(value)) => {
            validate_time(value, 86_400_000_000)?;
            Value::TimeMicros(value)
        }
        (Logical::TimestampMillis, Value::Long(value)) => Value::TimestampMillis(value),
        (Logical::TimestampMicros, Value::Long(value)) => Value::TimestampMicros(value),
        (Logical::TimestampNanos, Value::Long(value)) => Value::TimestampNanos(value),
        (Logical::LocalTimestampMillis, Value::Long(value)) => Value::LocalTimestampMillis(value),
        (Logical::LocalTimestampMicros, Value::Long(value)) => Value::LocalTimestampMicros(value),
        (Logical::LocalTimestampNanos, Value::Long(value)) => Value::LocalTimestampNanos(value),
        (Logical::Duration, Value::Fixed(12, bytes)) => {
            let bytes: [u8; 12] = bytes.try_into().map_err(|_| "duration requires 12 bytes")?;
            Value::Duration(Duration::from(bytes))
        }
        _ => return Err("value cannot represent reader logical type".into()),
    })
}

pub(crate) fn logical_value(value: Value, schema: &Schema) -> Result<Value, String> {
    let (physical, logical) = scalar(schema).ok_or("unexpected wire schema")?;
    let value = match (physical, value) {
        (Scalar::Fixed(size), Value::Bytes(bytes)) => Value::Fixed(size, bytes),
        (_, value) => value,
    };
    apply_logical(value, logical)
}

fn validate_decimal(bytes: &[u8], precision: usize) -> Result<(), String> {
    crate::guard::decimal_precision(bytes, precision)
        .map_err(|error| error.replace("schema precision", "reader precision"))
}

fn validate_time(value: i64, units_per_day: i64) -> Result<(), String> {
    if !(0..units_per_day).contains(&value) {
        return Err("time logical value is outside one day".into());
    }
    Ok(())
}

fn value_bytes(value: &Value) -> usize {
    match value {
        Value::String(value) | Value::Enum(_, value) => value.len(),
        Value::Bytes(value) | Value::Fixed(_, value) => value.len(),
        _ => 0,
    }
}

pub(crate) fn default_datum<'a>(
    json: &Json,
    schema: &'a Schema,
    names: &NamesRef<'a>,
    limits: crate::guard::Limits,
) -> Result<Value, String> {
    default_value(
        json,
        schema,
        names,
        0,
        "$",
        &mut Budget::new(Limits {
            depth: limits.max_depth,
            items: limits.max_items,
            bytes: limits.max_bytes,
        }),
    )
}

fn default_value<'a>(
    json: &Json,
    schema: &'a Schema,
    names: &NamesRef<'a>,
    depth: usize,
    path: &str,
    budget: &mut Budget,
) -> Result<Value, String> {
    budget.node(depth, path)?;
    let schema = dereference(schema, names)?;
    let invalid = || format!("{path}: invalid default for {schema}");
    let value = match schema {
        Schema::Union(union) => {
            for (index, branch) in union.variants().iter().enumerate() {
                // Failed alternatives also consume work and allocation budgets.
                match default_value(json, branch, names, depth + 1, path, budget) {
                    Ok(value) => return Ok(Value::Union(index as u32, Box::new(value))),
                    Err(error) if budget.exhausted => return Err(error),
                    Err(_) => {}
                }
            }
            return Err(invalid());
        }
        Schema::Record(record) => {
            let object = json.as_object().ok_or_else(invalid)?;
            if record.fields.len() > budget.limits.items.saturating_sub(budget.usage.items) {
                budget.exhausted = true;
                return Err(format!("{path}: default exceeds maximum item count"));
            }
            let mut fields = Vec::with_capacity(record.fields.len());
            for field in &record.fields {
                let path = format!("{path}.{}", field.name);
                let json = object
                    .get(&field.name)
                    .filter(|value| {
                        field.default.is_none() || !matches!(value, Json::Null | Json::Bool(false))
                    })
                    .or_else(|| field_default(field))
                    .ok_or_else(|| format!("{path}: missing field in record default"))?;
                budget.bytes(field.name.len(), &path)?;
                let value = default_value(json, &field.schema, names, depth + 1, &path, budget)?;
                fields.push((field.name.clone(), value));
            }
            return Ok(Value::Record(fields));
        }
        Schema::Array(array) => {
            let values = json.as_array().ok_or_else(invalid)?;
            if values.len() > budget.limits.items.saturating_sub(budget.usage.items) {
                budget.exhausted = true;
                return Err(format!("{path}: default exceeds maximum item count"));
            }
            return values
                .iter()
                .enumerate()
                .map(|(index, json)| {
                    default_value(
                        json,
                        &array.items,
                        names,
                        depth + 1,
                        &format!("{path}[{index}]"),
                        budget,
                    )
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array);
        }
        Schema::Map(map) => {
            let values = json.as_object().ok_or_else(invalid)?;
            if values.len() > budget.limits.items.saturating_sub(budget.usage.items) {
                budget.exhausted = true;
                return Err(format!("{path}: default exceeds maximum item count"));
            }
            return values
                .iter()
                .map(|(key, json)| {
                    budget.bytes(key.len(), path)?;
                    let value = default_value(
                        json,
                        &map.types,
                        names,
                        depth + 1,
                        &format!("{path}[{key:?}]"),
                        budget,
                    )?;
                    Ok((key.clone(), value))
                })
                .collect::<Result<Vec<_>, String>>()
                .map(Value::Record);
        }
        Schema::Enum(enumeration) => {
            let symbol = json.as_str().ok_or_else(invalid)?;
            let index = enumeration
                .symbols
                .iter()
                .position(|candidate| candidate == symbol)
                .ok_or_else(invalid)?;
            budget.bytes(symbol.len(), path)?;
            return Ok(Value::Enum(index as u32, symbol.into()));
        }
        _ => {
            let (physical, _) = scalar(schema).ok_or_else(invalid)?;
            match physical {
                Scalar::Null if json.is_null() => Value::Null,
                Scalar::Boolean => Value::Boolean(json.as_bool().ok_or_else(invalid)?),
                Scalar::Int => Value::Int(
                    i32::try_from(json.as_i64().ok_or_else(invalid)?).map_err(|_| invalid())?,
                ),
                Scalar::Long => Value::Long(json.as_i64().ok_or_else(invalid)?),
                Scalar::Float => Value::Double(json.as_f64().ok_or_else(invalid)?),
                Scalar::Double => Value::Double(json.as_f64().ok_or_else(invalid)?),
                Scalar::String => {
                    let value = json.as_str().ok_or_else(invalid)?;
                    budget.bytes(value.len(), path)?;
                    Value::String(value.into())
                }
                Scalar::Bytes | Scalar::Fixed(_) => {
                    let value = json.as_str().ok_or_else(invalid)?;
                    let logical = scalar(schema).ok_or_else(invalid)?.1;
                    let length = if logical == Logical::Plain {
                        value.len()
                    } else {
                        value.chars().count()
                    };
                    budget.bytes(length, path)?;
                    if let Scalar::Fixed(size) = physical
                        && size != length
                    {
                        return Err(invalid());
                    }
                    match logical {
                        Logical::Plain => Value::String(value.into()),
                        _ => {
                            let bytes = value
                                .chars()
                                .map(|character| {
                                    u8::try_from(u32::from(character)).map_err(|_| invalid())
                                })
                                .collect::<Result<Vec<_>, _>>()?;
                            match physical {
                                Scalar::Fixed(size) => Value::Fixed(size, bytes),
                                _ => Value::Bytes(bytes),
                            }
                        }
                    }
                }
                _ => return Err(invalid()),
            }
        }
    };
    let (_, logical) = scalar(schema).ok_or_else(invalid)?;
    apply_logical(value, logical).map_err(|error| format!("{path}: invalid default: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use apache_avro::schema::ResolvedSchema;
    use serde_json::json;

    fn parse(json: &str) -> apache_avro::AvroResult<Schema> {
        crate::validation::initialize().unwrap();
        Schema::parse_str(json)
    }

    fn prepare(writer: &Schema, reader: &Schema, limits: Limits) -> Resolution {
        crate::validation::initialize().unwrap();
        let writer_names = ResolvedSchema::new(writer).unwrap();
        let reader_names = ResolvedSchema::new(reader).unwrap();
        Resolution::new(
            writer,
            writer_names.get_names(),
            reader,
            reader_names.get_names(),
            limits.depth,
            limits.items,
            limits.bytes,
        )
        .unwrap()
    }

    fn limits() -> Limits {
        crate::validation::initialize().unwrap();
        Limits {
            depth: 64,
            items: 10_000,
            bytes: 1_000_000,
        }
    }

    fn resolve(writer: &str, reader: &str, value: Value) -> Result<Value, String> {
        let writer = parse(writer).unwrap();
        let reader = parse(reader).unwrap();
        prepare(&writer, &reader, limits())
            .apply(value)
            .and_then(|result| match result.failure {
                Some((_, error)) => Err(error),
                None => Ok(result.value),
            })
    }

    #[test]
    fn resolution_is_owned_send_and_sync() {
        fn send_sync<T: Send + Sync>() {}
        send_sync::<Resolution>();
        let plan = {
            let writer = Schema::Int;
            let reader = Schema::Long;
            prepare(&writer, &reader, limits())
        };
        assert_eq!(plan.apply(Value::Int(7)).unwrap().value, Value::Long(7));
    }

    #[test]
    fn named_dependency_plans_survive_dropping_all_schemas() {
        let plan = {
            let writer =
                parse(r#"{"type":"record","name":"old.R","fields":[{"name":"v","type":"int"}]}"#)
                    .unwrap();
            let reader = parse(r#"{"type":"record","name":"new.R","aliases":["old.R"],"fields":[{"name":"v","type":"long"}]}"#).unwrap();
            let writer_ref = Schema::Ref {
                name: Name::new("old.R").unwrap(),
            };
            let reader_ref = Schema::Ref {
                name: Name::new("new.R").unwrap(),
            };
            let writer_names = ResolvedSchema::new(&writer).unwrap();
            let reader_names = ResolvedSchema::new(&reader).unwrap();
            Resolution::new(
                &writer_ref,
                writer_names.get_names(),
                &reader_ref,
                reader_names.get_names(),
                64,
                1_000,
                10_000,
            )
            .unwrap()
        };
        assert_eq!(
            plan.apply(Value::Record(vec![("v".into(), Value::Int(8))]))
                .unwrap()
                .value,
            Value::Record(vec![("v".into(), Value::Long(8))])
        );
    }

    #[test]
    fn numeric_promotions_are_directional_even_when_the_value_fits() {
        let schemas = [Schema::Int, Schema::Long, Schema::Float, Schema::Double];
        let values = [
            Value::Int(1),
            Value::Long(1),
            Value::Float(1.0),
            Value::Double(1.0),
        ];
        for (writer_index, writer) in schemas.iter().enumerate() {
            for (reader_index, reader) in schemas.iter().enumerate() {
                let result = prepare(writer, reader, limits())
                    .apply(values[writer_index].clone())
                    .map(|result| result.value);
                if writer_index <= reader_index {
                    let output = if writer_index < 2 && reader_index >= 2 {
                        writer_index
                    } else {
                        reader_index
                    };
                    assert_eq!(result.unwrap(), values[output]);
                } else {
                    assert!(result.unwrap_err().contains("cannot resolve"));
                }
            }
        }
    }

    #[test]
    fn selected_writer_union_branch_alone_controls_compatibility() {
        assert_eq!(
            resolve(
                r#"["int","string"]"#,
                r#""long""#,
                Value::Union(0, Box::new(Value::Int(42)))
            )
            .unwrap(),
            Value::Long(42)
        );
        assert!(
            resolve(
                r#"["int","string"]"#,
                r#""long""#,
                Value::Union(1, Box::new(Value::String("42".into())))
            )
            .is_err()
        );
        let writer = r#"["null",{"type":"record","name":"R","fields":[]}]"#;
        let reader =
            r#"["null",{"type":"record","name":"R","fields":[{"name":"v","type":"int"}]}]"#;
        assert_eq!(
            resolve(writer, reader, Value::Union(0, Box::new(Value::Null))).unwrap(),
            Value::Union(0, Box::new(Value::Null))
        );
        assert!(
            resolve(
                writer,
                reader,
                Value::Union(1, Box::new(Value::Record(vec![])))
            )
            .is_err()
        );
    }

    #[test]
    fn reader_union_uses_first_matching_branch_and_remaps_index() {
        assert_eq!(
            resolve(
                r#"["null","int"]"#,
                r#"["double","long","null"]"#,
                Value::Union(1, Box::new(Value::Int(9)))
            )
            .unwrap(),
            Value::Union(0, Box::new(Value::Int(9)))
        );
        assert_eq!(
            resolve(
                r#"["null","int"]"#,
                r#"["double","long","null"]"#,
                Value::Union(0, Box::new(Value::Null))
            )
            .unwrap(),
            Value::Union(2, Box::new(Value::Null))
        );
    }

    #[test]
    fn record_aliases_field_aliases_reordering_and_defaults() {
        let writer = r#"{"type":"record","name":"old.R","fields":[
            {"name":"old_name","type":"int"},{"name":"ignored","type":"string"}]}"#;
        let reader = r#"{"type":"record","name":"new.R","aliases":["old.R"],"fields":[
            {"name":"flag","type":"boolean","default":false},
            {"name":"new_name","aliases":["old_name"],"type":"long"},
            {"name":"optional","type":["null","string"],"default":null}]}"#;
        let plan = prepare(&parse(writer).unwrap(), &parse(reader).unwrap(), limits());
        let result = plan
            .apply(Value::Record(vec![
                ("old_name".into(), Value::Int(12)),
                ("ignored".into(), Value::String("x".into())),
            ]))
            .unwrap();
        assert_eq!(
            result.value,
            Value::Record(vec![
                ("new_name".into(), Value::Long(12)),
                ("flag".into(), Value::Null),
                ("optional".into(), Value::Null)
            ])
        );
        assert_eq!(
            result
                .defaults
                .iter()
                .map(|(position, value)| (*position, value.as_ref()))
                .collect::<Vec<_>>(),
            vec![(1, &json!(false)), (2, &Json::Null)]
        );
    }

    #[test]
    fn repeated_reads_share_owned_default_definitions() {
        let plan = {
            let writer = parse(r#"{"type":"record","name":"R","fields":[]}"#).unwrap();
            let reader = parse(
                r#"{"type":"record","name":"R","fields":[
                {"name":"values","type":{"type":"array","items":"long"},"default":[1,2,3]}]}"#,
            )
            .unwrap();
            prepare(&writer, &reader, limits())
        };
        let first = plan.apply(Value::Record(vec![])).unwrap();
        let second = plan.apply(Value::Record(vec![])).unwrap();
        assert!(Arc::ptr_eq(&first.defaults[0].1, &second.defaults[0].1));
        assert_eq!(first.defaults[0].1.as_ref(), &json!([1, 2, 3]));
        assert_eq!(
            first.value,
            Value::Record(vec![("values".into(), Value::Null)])
        );
    }

    #[test]
    fn reader_defaults_do_not_replace_incompatible_writer_fields() {
        let writer = r#"{"type":"record","name":"R","fields":[{"name":"v","type":"string"}]}"#;
        let reader =
            r#"{"type":"record","name":"R","fields":[{"name":"v","type":"int","default":7}]}"#;
        let error = resolve(
            writer,
            reader,
            Value::Record(vec![("v".into(), Value::String("7".into()))]),
        )
        .unwrap_err();
        assert!(error.starts_with("$.v:"));
        assert!(error.contains("cannot resolve"));
    }

    #[test]
    fn names_include_namespace_and_only_reader_aliases_apply() {
        let writer = r#"{"type":"record","name":"a.R","aliases":["b.R"],"fields":[]}"#;
        let reader = r#"{"type":"record","name":"b.R","fields":[]}"#;
        assert!(resolve(writer, reader, Value::Record(vec![])).is_err());
        assert!(resolve(reader, writer, Value::Record(vec![])).is_ok());
    }

    #[test]
    fn recursive_names_compile_once_and_promote_recursively() {
        let writer = parse(
            r#"{"type":"record","name":"N","fields":[
            {"name":"v","type":"int"},{"name":"next","type":["null","N"]}]}"#,
        )
        .unwrap();
        let reader = parse(
            r#"{"type":"record","name":"N","fields":[
            {"name":"v","type":"long"},{"name":"next","type":["null","N"]}]}"#,
        )
        .unwrap();
        let tail = Value::Record(vec![
            ("v".into(), Value::Int(2)),
            ("next".into(), Value::Union(0, Box::new(Value::Null))),
        ]);
        let value = Value::Record(vec![
            ("v".into(), Value::Int(1)),
            ("next".into(), Value::Union(1, Box::new(tail))),
        ]);
        let plan = prepare(&writer, &reader, limits());
        assert!(plan.nodes.len() < 10);
        let Value::Record(output) = plan.apply(value).unwrap().value else {
            panic!("expected record");
        };
        assert_eq!(output[0].1, Value::Long(1));
        let Value::Union(1, tail) = &output[1].1 else {
            panic!("expected record union");
        };
        let Value::Record(tail) = tail.as_ref() else {
            panic!("expected tail");
        };
        assert_eq!(tail[0].1, Value::Long(2));
    }

    #[test]
    fn enum_resolution_remaps_symbols_and_uses_reader_default() {
        let writer = r#"{"type":"enum","name":"E","symbols":["A","B","C"]}"#;
        let reader = r#"{"type":"enum","name":"E","symbols":["B","A"],"default":"B"}"#;
        assert_eq!(
            resolve(writer, reader, Value::Enum(0, "A".into())).unwrap(),
            Value::Enum(1, "A".into())
        );
        assert_eq!(
            resolve(writer, reader, Value::Enum(2, "C".into())).unwrap(),
            Value::Enum(0, "B".into())
        );
        assert_eq!(
            resolve(
                writer,
                r#"{"type":"enum","name":"E","symbols":["A"]}"#,
                Value::Enum(2, "C".into())
            )
            .unwrap(),
            Value::Enum(2, "C".into())
        );
        assert!(
            resolve(
                writer,
                r#"{"type":"enum","name":"Other","symbols":["A"]}"#,
                Value::Enum(0, "A".into())
            )
            .is_err()
        );
    }

    #[test]
    fn fixed_requires_name_and_size_with_reader_aliases() {
        let writer = r#"{"type":"fixed","name":"F","size":2}"#;
        let reader = r#"{"type":"fixed","name":"G","aliases":["F"],"size":2}"#;
        assert_eq!(
            resolve(writer, reader, Value::Fixed(2, vec![0, 255])).unwrap(),
            Value::Fixed(2, vec![0, 255])
        );
        assert!(
            resolve(
                writer,
                r#"{"type":"fixed","name":"G","size":2}"#,
                Value::Fixed(2, vec![0, 255])
            )
            .is_err()
        );
        assert!(
            resolve(
                writer,
                r#"{"type":"fixed","name":"F","size":3}"#,
                Value::Fixed(2, vec![0, 255])
            )
            .is_err()
        );
    }

    #[test]
    fn strings_and_bytes_preserve_the_writer_representation() {
        assert_eq!(
            resolve(r#""string""#, r#""bytes""#, Value::String("é".into())).unwrap(),
            Value::String("é".into())
        );
        assert_eq!(
            resolve(r#""bytes""#, r#""string""#, Value::Bytes(vec![0xc3, 0xa9])).unwrap(),
            Value::Bytes(vec![0xc3, 0xa9])
        );
        assert_eq!(
            resolve(r#""bytes""#, r#""string""#, Value::Bytes(vec![255])).unwrap(),
            Value::Bytes(vec![255])
        );
    }

    #[test]
    fn logical_values_use_reader_annotations_and_underlying_promotions() {
        assert_eq!(
            prepare(&Schema::Date, &Schema::Long, limits())
                .apply(Value::Date(-1))
                .unwrap()
                .value,
            Value::Long(-1)
        );
        assert_eq!(
            prepare(&Schema::Int, &Schema::Date, limits())
                .apply(Value::Int(42))
                .unwrap()
                .value,
            Value::Date(42)
        );
        assert_eq!(
            prepare(&Schema::TimestampMillis, &Schema::TimestampMicros, limits())
                .apply(Value::TimestampMillis(123))
                .unwrap()
                .value,
            Value::TimestampMicros(123)
        );
        assert!(
            prepare(&Schema::Long, &Schema::Date, limits())
                .apply(Value::Long(1))
                .map(|result| result.value)
                .is_err()
        );
        let uuid = uuid::Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        assert_eq!(
            prepare(&Schema::String, &Schema::Uuid(UuidSchema::String), limits())
                .apply(Value::String(uuid.to_string()))
                .unwrap()
                .value,
            Value::Uuid(uuid)
        );
        assert!(
            prepare(&Schema::TimeMillis, &Schema::Int, limits())
                .apply(Value::TimeMillis(-1))
                .map(|result| result.value)
                .is_err()
        );
        assert!(
            prepare(&Schema::Long, &Schema::TimeMicros, limits())
                .apply(Value::Long(86_400_000_000))
                .map(|result| result.value)
                .is_err()
        );
    }

    #[test]
    fn decimal_resolution_preserves_metadata_and_rejects_overflow() {
        let decimal = r#"{"type":"bytes","logicalType":"decimal","precision":2,"scale":1}"#;
        assert_eq!(
            resolve(r#""bytes""#, decimal, Value::Bytes(vec![99])).unwrap(),
            Value::Decimal(Decimal::from(vec![99]))
        );
        assert!(resolve(r#""bytes""#, decimal, Value::Bytes(vec![100])).is_err());
        assert!(resolve(r#""bytes""#, decimal, Value::Bytes(vec![156])).is_err());
        assert_eq!(
            resolve(
                decimal,
                r#""bytes""#,
                Value::Decimal(Decimal::from(vec![157]))
            )
            .unwrap(),
            Value::Bytes(vec![157])
        );
        assert!(
            resolve(
                decimal,
                r#""bytes""#,
                Value::Decimal(Decimal::from(vec![100]))
            )
            .is_err()
        );
        assert!(
            resolve(
                decimal,
                r#"{"type":"bytes","logicalType":"decimal","precision":2,"scale":0}"#,
                Value::Decimal(Decimal::from(vec![99]))
            )
            .is_err()
        );
        assert!(
            resolve(
                decimal,
                r#"{"type":"bytes","logicalType":"decimal","precision":3,"scale":1}"#,
                Value::Decimal(Decimal::from(vec![99]))
            )
            .is_err()
        );
    }

    #[test]
    fn big_decimal_resolves_bytes_without_expanding_extreme_scales() {
        for coefficient in [0, 127, 128, -1, -128, -129] {
            for scale in [0, 2, -3, 5000, -5000, i64::MIN, i64::MAX] {
                let number = apache_avro::BigDecimal::new(coefficient.into(), scale);
                let bytes = crate::big_decimal::encode(&number).unwrap();
                assert_eq!(
                    prepare(&Schema::BigDecimal, &Schema::Bytes, limits())
                        .apply(Value::BigDecimal(number.clone()))
                        .unwrap()
                        .value,
                    Value::Bytes(bytes.clone())
                );
                for (writer, value) in [
                    (Schema::Bytes, Value::Bytes(bytes)),
                    (Schema::BigDecimal, Value::BigDecimal(number)),
                ] {
                    let Value::BigDecimal(decoded) =
                        prepare(&writer, &Schema::BigDecimal, limits())
                            .apply(value)
                            .unwrap()
                            .value
                    else {
                        panic!("expected big-decimal");
                    };
                    assert_eq!(
                        decoded.as_bigint_and_exponent(),
                        (coefficient.into(), scale)
                    );
                }
            }
        }
    }

    #[test]
    fn big_decimal_string_promotions_preserve_bytes_and_selected_union_branch() {
        let bytes = vec![2, 7, 4];
        let number = apache_avro::BigDecimal::new(7.into(), 2);
        assert_eq!(
            prepare(&Schema::BigDecimal, &Schema::String, limits())
                .apply(Value::BigDecimal(number.clone()))
                .unwrap()
                .value,
            Value::Bytes(bytes.clone())
        );
        assert_eq!(
            prepare(&Schema::String, &Schema::BigDecimal, limits())
                .apply(Value::String(String::from_utf8(bytes).unwrap()))
                .unwrap()
                .value,
            Value::BigDecimal(number.clone())
        );
        assert_eq!(
            prepare(&Schema::BigDecimal, &Schema::String, limits())
                .apply(Value::BigDecimal(apache_avro::BigDecimal::from(-1)))
                .unwrap()
                .value,
            Value::Bytes(crate::big_decimal::encode(&apache_avro::BigDecimal::from(-1)).unwrap())
        );
        let writer = parse(r#"["long",{"type":"bytes","logicalType":"big-decimal"}]"#).unwrap();
        let reader =
            parse(r#"["null",{"type":"bytes","logicalType":"big-decimal"},"string"]"#).unwrap();
        let plan = prepare(&writer, &reader, limits());
        assert_eq!(
            plan.apply(Value::Union(1, Box::new(Value::BigDecimal(number.clone()))))
                .unwrap()
                .value,
            Value::Union(1, Box::new(Value::BigDecimal(number)))
        );
        assert!(
            plan.apply(Value::Union(0, Box::new(Value::Long(7))))
                .map(|result| result.value)
                .is_err()
        );
    }

    #[test]
    fn big_decimal_resolution_rejects_malformed_inner_bytes_and_enforces_budget() {
        let plan = prepare(&Schema::Bytes, &Schema::BigDecimal, limits());
        for bytes in [
            vec![],
            vec![0, 0],
            vec![2],
            vec![2, 7],
            vec![2, 7, 4, 0],
            vec![1, 0],
        ] {
            assert!(
                plan.apply(Value::Bytes(bytes))
                    .map(|result| result.value)
                    .is_err()
            );
        }
        let mut limits = limits();
        limits.bytes = 2;
        assert!(
            prepare(&Schema::Bytes, &Schema::BigDecimal, limits)
                .apply(Value::Bytes(vec![2, 7, 4]))
                .unwrap_err()
                .contains("byte count")
        );
        assert!(
            prepare(&Schema::BigDecimal, &Schema::Bytes, limits)
                .apply(Value::BigDecimal(apache_avro::BigDecimal::from(7)))
                .unwrap_err()
                .contains("byte count")
        );
    }

    #[test]
    fn defaults_preserve_json_floats_and_utf8_bytes() {
        let mut budget = Budget::new(limits());
        let names = NamesRef::new();
        assert_eq!(
            default_value(
                &json!("ÿ\u{0}"),
                &Schema::Bytes,
                &names,
                0,
                "$",
                &mut budget
            )
            .unwrap(),
            Value::String("ÿ\u{0}".into())
        );
        assert_eq!(
            default_value(&json!("Ā"), &Schema::Bytes, &names, 0, "$", &mut budget).unwrap(),
            Value::String("Ā".into())
        );
        assert_eq!(
            default_value(&json!(0.1), &Schema::Float, &names, 0, "$", &mut budget).unwrap(),
            Value::Double(0.1)
        );
        assert!(default_value(&json!(1.0), &Schema::Int, &names, 0, "$", &mut budget).is_err());
        assert!(
            default_value(
                &json!(2_147_483_648_i64),
                &Schema::Int,
                &names,
                0,
                "$",
                &mut budget
            )
            .is_err()
        );
        assert!(
            default_value(
                &json!("false"),
                &Schema::Boolean,
                &names,
                0,
                "$",
                &mut budget
            )
            .is_err()
        );
        assert_eq!(
            default_value(&json!(-1), &Schema::Date, &names, 0, "$", &mut budget).unwrap(),
            Value::Date(-1)
        );
    }

    #[test]
    fn union_default_uses_first_matching_branch_under_avro_1_12() {
        let schema = parse(r#"["null","int","long"]"#).unwrap();
        assert_eq!(
            default_value(
                &json!(42),
                &schema,
                &NamesRef::new(),
                0,
                "$",
                &mut Budget::new(limits())
            )
            .unwrap(),
            Value::Union(1, Box::new(Value::Int(42)))
        );
    }

    #[test]
    fn failed_union_defaults_cannot_reset_the_preparation_budget() {
        let schema = parse(r#"["null","boolean","int","string"]"#).unwrap();
        let mut limits = limits();
        limits.items = 3;
        let error = default_value(
            &json!("value"),
            &schema,
            &NamesRef::new(),
            0,
            "$",
            &mut Budget::new(limits),
        )
        .unwrap_err();
        assert!(error.contains("item count"));
    }

    #[test]
    fn default_expansion_counts_against_whole_result_budget() {
        let writer = parse(r#"{"type":"record","name":"R","fields":[]}"#).unwrap();
        let reader = parse(
            r#"{"type":"record","name":"R","fields":[
            {"name":"v","type":{"type":"array","items":"int"},"default":[1,2]}]}"#,
        )
        .unwrap();
        let mut limits = limits();
        limits.items = 3;
        assert!(
            prepare(&writer, &reader, limits)
                .apply(Value::Record(vec![]))
                .unwrap_err()
                .contains("item count")
        );
    }

    #[test]
    fn implicit_defaults_cannot_expand_an_exponential_schema_graph() {
        crate::validation::initialize().unwrap();
        use apache_avro::schema::{RecordField, RecordSchema};

        let mut dependencies = vec![Schema::Record(
            RecordSchema::builder()
                .name(Name::new("Tree0").unwrap())
                .build(),
        )];
        for level in 1..24 {
            let fields = ["left", "right"].map(|name| {
                RecordField::builder()
                    .name(name)
                    .schema(Schema::Ref {
                        name: Name::new(format!("Tree{}", level - 1)).unwrap(),
                    })
                    .default(json!({}))
                    .build()
            });
            dependencies.push(Schema::Record(
                RecordSchema::builder()
                    .name(Name::new(format!("Tree{level}")).unwrap())
                    .fields(fields.to_vec())
                    .build(),
            ));
        }
        let writer = Schema::Record(
            RecordSchema::builder()
                .name(Name::new("Root").unwrap())
                .build(),
        );
        let reader = Schema::Record(
            RecordSchema::builder()
                .name(Name::new("Root").unwrap())
                .fields(vec![
                    RecordField::builder()
                        .name("tree")
                        .schema(Schema::Ref {
                            name: Name::new("Tree23").unwrap(),
                        })
                        .default(json!({}))
                        .build(),
                ])
                .build(),
        );
        let writer_names = ResolvedSchema::new(&writer).unwrap();
        let reader_names =
            ResolvedSchema::new_with_schemata(dependencies.iter().chain([&reader]).collect())
                .unwrap();
        let error = Resolution::new(
            &writer,
            writer_names.get_names(),
            &reader,
            reader_names.get_names(),
            64,
            1_000,
            1_000_000,
        )
        .err()
        .expect("bounded expansion must fail");
        assert!(error.contains("item count"));
    }

    #[test]
    fn depth_limit_includes_defaults_at_their_result_position() {
        let writer = parse(
            r#"{"type":"record","name":"R","fields":[
            {"name":"child","type":{"type":"record","name":"C","fields":[]}}]}"#,
        )
        .unwrap();
        let reader = parse(
            r#"{"type":"record","name":"R","fields":[
            {"name":"child","type":{"type":"record","name":"C","fields":[
                {"name":"v","type":{"type":"array","items":"int"},"default":[1]}]}}]}"#,
        )
        .unwrap();
        let mut limits = limits();
        limits.depth = 2;
        let error = prepare(&writer, &reader, limits)
            .apply(Value::Record(vec![("child".into(), Value::Record(vec![]))]))
            .unwrap_err();
        assert!(error.starts_with("$.child.v:"));
        assert!(error.contains("depth"));
    }

    #[test]
    fn array_and_map_contents_are_resolved_recursively() {
        let writer = r#"{"type":"map","values":{"type":"array","items":"int"}}"#;
        let reader = r#"{"type":"map","values":{"type":"array","items":"double"}}"#;
        assert_eq!(
            resolve(
                writer,
                reader,
                Value::Record(Vec::from([("a".into(), Value::Array(vec![Value::Int(1)]))]))
            )
            .unwrap(),
            Value::Record(Vec::from([("a".into(), Value::Array(vec![Value::Int(1)]))]))
        );
        assert!(
            resolve(
                r#"{"type":"array","items":"string"}"#,
                r#"{"type":"array","items":"int"}"#,
                Value::Array(vec![])
            )
            .is_err()
        );
    }
}
