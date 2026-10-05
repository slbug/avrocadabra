use super::{
    World,
    check::{
        self, ARRAY_CAP, Caps, INERT_CAP, INTEGER_CAP, LOOKUP_CAP, MAP_CAP, STRING_CAP, VALUE_CAP,
        WALK_CAP,
    },
    plan::{Adapter, Field, Kind, Logical, Plan},
};
use crate::{big_decimal::write_long, convert::shortest_digits};
use magnus::{
    Error, RArray, RString, Ruby, TryConvert, Value,
    encoding::Coderange,
    prelude::*,
    rb_sys::{AsRawId, AsRawValue, FromRawValue, protect},
    value::LazyId,
};
use num_bigint::BigInt;
use rb_sys::VALUE;
use std::{
    collections::HashMap,
    hash::{BuildHasherDefault, Hasher},
};

const MAX_BYTES: usize = 16 * 1024 * 1024;

pub enum Fail {
    Invalid,
    /// Hit a call Ruby could observe: restart with Ruby Avro's selection.
    Abort,
    Raise(Error),
    Limit(Error),
}

impl From<Error> for Fail {
    fn from(error: Error) -> Self {
        Fail::Raise(error)
    }
}

type R<T> = Result<T, Fail>;

fn nil() -> Value {
    unsafe { Value::from_raw(rb_sys::Qnil as VALUE) }
}

fn raw(value: VALUE) -> Value {
    unsafe { Value::from_raw(value) }
}

fn int(value: i64) -> Value {
    raw(unsafe { rb_sys::rb_ll2inum(value) })
}

/// Ruby Avro's explicit receivers: private and protected methods raise.
fn funcall(recv: VALUE, name: &str, args: &[VALUE]) -> Result<Value, Error> {
    let ruby = unsafe { Ruby::get_unchecked() };
    let id = ruby.intern(name).as_raw();
    let value = protect(|| unsafe {
        rb_sys::rb_funcallv_public(recv, id, args.len() as _, args.as_ptr())
    })?;
    Ok(raw(value))
}

fn fcall(recv: VALUE, name: &str, args: &[VALUE]) -> Result<Value, Error> {
    let ruby = unsafe { Ruby::get_unchecked() };
    let id = ruby.intern(name).as_raw();
    let value =
        protect(|| unsafe { rb_sys::rb_funcallv(recv, id, args.len() as _, args.as_ptr()) })?;
    Ok(raw(value))
}

fn send(recv: VALUE, name: &LazyId, args: &[VALUE]) -> Result<Value, Error> {
    let id = check::id(name);
    let value = protect(|| unsafe {
        rb_sys::rb_funcallv_public(recv, id, args.len() as _, args.as_ptr())
    })?;
    Ok(raw(value))
}

static ENCODE: LazyId = LazyId::new("encode");
static DEFAULT_PROC: LazyId = LazyId::new("default_proc");
static SCHEMA: LazyId = LazyId::new("Schema");
static VALIDATION_OPTIONS: LazyId = LazyId::new("VALIDATION_OPTIONS");
static AVRO_TYPE_ERROR: LazyId = LazyId::new("AvroTypeError");
static AVRO_ERROR: LazyId = LazyId::new("AvroError");
static HASH: LazyId = LazyId::new("Hash");
static ARRAY: LazyId = LazyId::new("Array");

const CASES: [(&[&str], Step); 14] = [
    (&["null"], Step::Encoder("write_null")),
    (&["boolean"], Step::Encoder("write_boolean")),
    (&["string"], Step::Encoder("write_string")),
    (&["int"], Step::Encoder("write_int")),
    (&["long"], Step::Encoder("write_long")),
    (&["float"], Step::Encoder("write_float")),
    (&["double"], Step::Encoder("write_double")),
    (&["bytes"], Step::Encoder("write_bytes")),
    (&["fixed"], Step::Writer("write_fixed")),
    (&["enum"], Step::Writer("write_enum")),
    (&["array"], Step::Writer("write_array")),
    (&["map"], Step::Writer("write_map")),
    (&["union"], Step::Writer("write_union")),
    (
        &["record", "error", "request"],
        Step::Writer("write_record"),
    ),
];

#[derive(Clone, Copy)]
enum Step {
    Encoder(&'static str),
    Writer(&'static str),
}

/// `opt_case_dispatch` jumps by hash for literal-typed keys and never calls an unredefined `===`.
fn case_dispatch(ruby: &Ruby, key: VALUE) -> Option<Option<Step>> {
    let value = raw(key);
    let literal = rb_sys::SPECIAL_CONST_P(key)
        || magnus::Float::from_value(value).is_some()
        || magnus::Symbol::from_value(value).is_some()
        || magnus::RBignum::from_value(value).is_some()
        || RString::from_value(value).is_some();
    let classes = [
        ruby.class_symbol(),
        ruby.class_string(),
        ruby.class_integer(),
        ruby.class_float(),
        ruby.class_nil_class(),
        ruby.class_true_class(),
        ruby.class_false_class(),
    ];
    if !literal
        || !classes
            .iter()
            .all(|class| check::basic(class.as_raw(), &[&check::TRIPLE]))
    {
        return None;
    }
    Some(
        CASES
            .iter()
            .find(|(patterns, _)| {
                patterns
                    .iter()
                    .any(|pattern| ruby.to_symbol(pattern).as_raw() == key)
            })
            .map(|&(_, step)| step),
    )
}

fn integer(value: Value) -> bool {
    magnus::Integer::from_value(value).is_some()
}

fn float(value: Value) -> bool {
    magnus::Float::from_value(value).is_some()
}

struct Mark {
    out: usize,
    items: usize,
    bytes: usize,
}

pub struct Writer<'a> {
    ruby: &'a Ruby,
    world: &'a World,
    plan: &'a Plan,
    writer: VALUE,
    encoder: VALUE,
    io: VALUE,
    io_encoding: i32,
    pub out: Vec<u8>,
    flushed: usize,
    caps: Caps,
    groups: u8,
    generation: u64,
    checked: Vec<u64>,
    fused: usize,
    unlimited: Option<bool>,
    depth: usize,
    items: usize,
    budget: RArray,
    max_items: usize,
    max_depth: usize,
    work: i64,
    inert_cache: HashMap<VALUE, bool, BuildHasherDefault<Mix>>,
    inert_stamp: (u64, u64),
    back_edges: usize,
    factors: Vec<(VALUE, i64, bool)>,
}

#[derive(Default)]
struct Mix(u64);

impl Hasher for Mix {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = (value >> 3).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

pub struct Bounds {
    pub items: usize,
    pub depth: usize,
    pub budget: RArray,
    pub groups: u8,
}

impl<'a> Writer<'a> {
    pub fn new(
        ruby: &'a Ruby,
        world: &'a World,
        plan: &'a Plan,
        (writer, encoder, io, io_encoding): (VALUE, VALUE, VALUE, i32),
        bounds: Bounds,
    ) -> Self {
        let mut caps = Caps::default();
        caps.reset(bounds.groups);
        Self {
            ruby,
            world,
            plan,
            writer,
            encoder,
            io,
            io_encoding,
            out: Vec::new(),
            flushed: 0,
            caps,
            groups: bounds.groups,
            generation: 0,
            checked: vec![0; plan.nodes.len()],
            fused: 0,
            unlimited: None,
            depth: 0,
            items: 0,
            work: bounds.budget.entry(0).unwrap_or_default(),
            budget: bounds.budget,
            max_items: bounds.items,
            max_depth: bounds.depth,
            inert_cache: HashMap::default(),
            factors: Vec::new(),
            inert_stamp: (u64::MAX, 0),
            back_edges: 0,
        }
    }

    pub fn flush(&mut self) -> Result<(), Error> {
        if self.out.is_empty() {
            return Ok(());
        }
        let chunk = crate::allocate(|| self.ruby.str_from_slice(&self.out))?;
        self.flushed += self.out.len();
        self.out.clear();
        send(self.io, &check::WRITE, &[chunk.as_raw()])?;
        Ok(())
    }

    fn refresh(&mut self) -> Result<(), Error> {
        self.generation += 1;
        self.unlimited = None;
        let groups = check::stock(self.ruby)?;
        // A closed or frozen stream raises at the next flush, before anything else is observable.
        let ready = check::ivar(self.encoder, &check::IV_WRITER) == self.io
            && super::io_encoding(self.io)? == Some(self.io_encoding);
        self.groups = if ready { groups } else { 0 };
        self.caps.reset(self.groups);
        Ok(())
    }

    fn native(&self) -> bool {
        self.groups & check::CORE != 0
    }

    fn group(&self, group: u8) -> bool {
        self.groups & group == group
    }

    fn call<T>(&mut self, work: impl FnOnce(&mut Self) -> Result<T, Error>) -> R<T> {
        if self.fused > 0 {
            return Err(Fail::Abort);
        }
        self.flush()?;
        self.budget.store(0, self.work)?;
        let result = work(self);
        self.work = self.budget.entry(0)?;
        self.refresh()?;
        result.map_err(Fail::Raise)
    }

    fn limit(&self, message: &str) -> Fail {
        Fail::Limit(Error::new(self.world.encode_error(), message.to_owned()))
    }

    fn room(&self, size: usize) -> R<()> {
        if self.flushed + self.out.len() + size > MAX_BYTES {
            return Err(self.limit("encoded datum exceeds max_bytes"));
        }
        Ok(())
    }

    fn emit(&mut self, bytes: &[u8]) -> R<()> {
        self.room(bytes.len())?;
        self.out.extend_from_slice(bytes);
        Ok(())
    }

    /// Unwinds an over-limit varint: a failed encode still flushes `out`.
    fn emit_long(&mut self, value: i64) -> R<()> {
        let start = self.out.len();
        write_long(value, &mut self.out);
        let room = self.room(0);
        if room.is_err() {
            self.out.truncate(start);
        }
        room
    }

    fn mark(&self) -> Mark {
        Mark {
            out: self.out.len(),
            items: self.items,
            bytes: self.flushed,
        }
    }

    fn rollback(&mut self, mark: Mark) {
        debug_assert_eq!(mark.bytes, self.flushed);
        self.out.truncate(mark.out);
        self.items = mark.items;
    }

    fn current(&mut self, node: usize) -> bool {
        if self.checked[node] == self.generation {
            return true;
        }
        let current = self.plan.current(node);
        if current {
            self.checked[node] = self.generation;
        }
        current
    }

    fn ready(&mut self, node: usize) -> bool {
        self.native() && self.current(node)
    }

    fn has(&mut self, value: Value, cap: u16) -> bool {
        self.caps.has(check::class_of(value), cap)
    }

    /// `Ok` only when an overridden `raise` returns.
    fn reject(&mut self, node: usize, datum: Value) -> R<()> {
        if self.fused > 0 {
            return Err(Fail::Invalid);
        }
        let (schema, writer, cref) = (self.plan.nodes[node].schema, self.writer, self.world.cref());
        self.call(|_| {
            let class = check::lexical(&cref, check::id(&AVRO_TYPE_ERROR))?;
            let exception = funcall(class, "new", &[schema, datum.as_raw()])?;
            fcall(writer, "raise", &[exception.as_raw()]).map(drop)
        })
    }

    fn enter(&mut self) -> R<()> {
        if self.depth > self.max_depth {
            return Err(self.limit("value exceeds maximum depth"));
        }
        self.items += 1;
        if self.items > self.max_items {
            return Err(self.limit("value exceeds maximum item count"));
        }
        Ok(())
    }

    pub fn write(&mut self, node: usize, datum: Value) -> R<Value> {
        self.enter()?;
        self.write_data(node, datum)
    }

    fn child(&mut self, node: usize, datum: Value) -> R<Value> {
        self.depth += 1;
        let result = self.write(node, datum);
        self.depth -= 1;
        result
    }

    fn ruby_write(&mut self, schema: VALUE, datum: Value) -> R<Value> {
        self.enter()?;
        let (writer, encoder) = (self.writer, self.encoder);
        self.call(|_| fcall(writer, "write_data", &[schema, datum.as_raw(), encoder]))
    }

    fn ruby_child(&mut self, schema: VALUE, datum: Value) -> R<Value> {
        self.depth += 1;
        let result = self.ruby_write(schema, datum);
        self.depth -= 1;
        result
    }

    fn write_data(&mut self, node: usize, datum: Value) -> R<Value> {
        if !self.ready(node) {
            let (writer, schema, encoder) =
                (self.writer, self.plan.nodes[node].schema, self.encoder);
            return self.call(|_| fcall(writer, "write_data", &[schema, datum.as_raw(), encoder]));
        }
        let datum = self.adapt(node, datum)?;
        if !self.ready(node) {
            return self.tail(node, datum);
        }
        if !self.simple(node, datum)? {
            self.reject(node, datum)?;
            return self.case(node, datum);
        }
        if !self.ready(node) {
            return self.case(node, datum);
        }
        self.dispatch(node, datum)
    }

    fn adapt(&mut self, node: usize, datum: Value) -> R<Value> {
        let (mut adapter, mut adapter_value) = (
            self.plan.nodes[node].adapter,
            self.plan.nodes[node].adapter_value,
        );
        if let Adapter::Pending = adapter {
            let schema = self.plan.nodes[node].schema;
            adapter_value = self
                .call(|_| funcall(schema, "type_adapter", &[]))?
                .as_raw();
            adapter = self.world.env.classify(adapter_value).0;
        }
        match adapter {
            Adapter::Pending => {}
            Adapter::Identity => return Ok(datum),
            Adapter::Decimal {
                precision,
                scale,
                factor,
            } => {
                if let Some(bytes) = self.native_decimal(datum, precision, scale, factor)? {
                    let text = crate::allocate(|| {
                        let text = self.ruby.str_from_slice(&bytes);
                        text.freeze();
                        text
                    })?;
                    return Ok(text.as_value());
                }
            }
            Adapter::Stock(logical) => {
                let logical = match logical {
                    Logical::IntDate => 0,
                    _ => 1,
                };
                if self.stock_logical(datum, logical == 1) {
                    return match send(adapter_value, &ENCODE, &[datum.as_raw()]) {
                        Ok(value) => Ok(value),
                        Err(error)
                            if self.fused > 0
                                && error.is_kind_of(self.ruby.exception_standard_error()) =>
                        {
                            Err(Fail::Invalid)
                        }
                        Err(error) => Err(Fail::Raise(error)),
                    };
                }
            }
            Adapter::Custom => {}
        }
        self.call(|_| funcall(adapter_value, "encode", &[datum.as_raw()]))
    }

    fn stock_logical(&mut self, datum: Value, timestamp: bool) -> bool {
        if !self.group(check::LOGICAL) || !self.has(datum, VALUE_CAP) {
            return false;
        }
        if integer(datum) {
            return self.has(datum, INTEGER_CAP);
        }
        if float(datum) {
            return true;
        }
        timestamp
            && self.group(check::TIME)
            && check::class_of(datum) == self.world.time
            && check::public(self.world.time, &[&check::TO_I, &USEC, &NSEC])
    }

    fn simple(&mut self, node: usize, datum: Value) -> R<bool> {
        if !self.has(datum, VALUE_CAP) || (integer(datum) && !self.has(datum, INTEGER_CAP)) {
            return self.validate(node, datum, true);
        }
        Ok(match &self.plan.nodes[node].kind {
            Kind::Null => datum.is_nil(),
            Kind::Boolean => {
                datum.as_raw() == rb_sys::Qtrue as VALUE
                    || datum.as_raw() == rb_sys::Qfalse as VALUE
            }
            Kind::Int => {
                i64::try_convert(datum).is_ok_and(|value| i32::try_from(value).is_ok())
                    && integer(datum)
            }
            Kind::Long => integer(datum) && i64::try_convert(datum).is_ok(),
            Kind::Float | Kind::Double => {
                float(datum)
                    || integer(datum)
                    || unsafe {
                        rb_sys::rb_obj_is_kind_of(datum.as_raw(), self.world.decimal_class())
                    } != rb_sys::Qfalse as VALUE
            }
            Kind::Bytes | Kind::Str => RString::from_value(datum).is_some(),
            Kind::Fixed { bytes, .. } => {
                let bytes = *bytes;
                match RString::from_value(datum) {
                    Some(text) if self.has(datum, STRING_CAP) => text.len() == bytes,
                    Some(_) => return self.validate(node, datum, true),
                    None => false,
                }
            }
            Kind::Enum { .. } => self.enum_index(node, datum).is_some(),
            _ => true,
        })
    }

    fn validate(&mut self, node: usize, datum: Value, encoded: bool) -> R<bool> {
        self.validate_schema(self.plan.nodes[node].schema, datum, encoded)
    }

    fn validate_schema(&mut self, schema: VALUE, datum: Value, encoded: bool) -> R<bool> {
        let (native, cref) = (self.native(), self.world.cref());
        let (module, options) = (self.world.schema, self.world.validation_options);
        let depth = (self.max_depth - self.depth.min(self.max_depth) + 1) as i64;
        self.budget.store(1, depth)?;
        let valid = self.call(|_| {
            let scoped = |name: &LazyId, verified: VALUE| {
                if native {
                    Ok(verified)
                } else {
                    check::lexical(&cref, check::id(name))
                }
            };
            let module = scoped(&SCHEMA, module)?;
            if encoded {
                funcall(
                    module,
                    "validate",
                    &[
                        schema,
                        datum.as_raw(),
                        scoped(&VALIDATION_OPTIONS, options)?,
                    ],
                )
            } else {
                funcall(module, "validate", &[schema, datum.as_raw()])
            }
        })?;
        Ok(valid.to_bool())
    }

    fn enum_index(&self, node: usize, datum: Value) -> Option<usize> {
        let Kind::Enum { names, .. } = &self.plan.nodes[node].kind else {
            return None;
        };
        names.iter().position(|name| {
            rb_sys::TEST(unsafe { rb_sys::rb_str_equal(name.value, datum.as_raw()) })
        })
    }

    fn tail(&mut self, node: usize, datum: Value) -> R<Value> {
        if !self.validate(node, datum, true)? {
            self.reject(node, datum)?;
        }
        self.case(node, datum)
    }

    fn case(&mut self, node: usize, datum: Value) -> R<Value> {
        let (schema, writer, encoder, cref) = (
            self.plan.nodes[node].schema,
            self.writer,
            self.encoder,
            self.world.cref(),
        );
        let ruby = self.ruby;
        self.call(|_| {
            let symbol = funcall(schema, "type_sym", &[])?.as_raw();
            let step = match case_dispatch(ruby, symbol) {
                Some(step) => step,
                None => {
                    let mut chosen = None;
                    'cases: for (patterns, step) in CASES {
                        for pattern in patterns {
                            if fcall(ruby.to_symbol(pattern).as_raw(), "===", &[symbol])?.to_bool()
                            {
                                chosen = Some(step);
                                break 'cases;
                            }
                        }
                    }
                    chosen
                }
            };
            match step {
                Some(Step::Encoder(method)) => funcall(encoder, method, &[datum.as_raw()]),
                Some(Step::Writer(method)) => {
                    fcall(writer, method, &[schema, datum.as_raw(), encoder])
                }
                None => {
                    let kind = funcall(schema, "type", &[])?.as_raw();
                    let message = protect(|| unsafe {
                        rb_sys::rb_str_plus(
                            ruby.str_new("Unknown type: ").as_raw(),
                            rb_sys::rb_obj_as_string(kind),
                        )
                    })?;
                    let class = check::lexical(&cref, check::id(&AVRO_ERROR))?;
                    let exception = funcall(class, "new", &[message])?;
                    fcall(writer, "raise", &[exception.as_raw()])
                }
            }
        })
    }

    fn integer_ready(&mut self) -> bool {
        let integer = self.ruby.class_integer().as_raw();
        self.caps.has(integer, INTEGER_CAP)
    }

    fn dispatch(&mut self, node: usize, datum: Value) -> R<Value> {
        let plan = self.plan;
        let kind = &plan.nodes[node].kind;
        if !self.integer_ready()
            && !matches!(
                kind,
                Kind::Null
                    | Kind::Array { .. }
                    | Kind::Map { .. }
                    | Kind::Union { .. }
                    | Kind::Record { .. }
            )
        {
            return self.encoder_call(node, datum);
        }
        match kind {
            Kind::Null => Ok(nil()),
            Kind::Boolean => {
                self.emit(&[u8::from(datum.to_bool())])?;
                Ok(int(1))
            }
            Kind::Int | Kind::Long => {
                self.emit_long(i64::try_convert(datum)?)?;
                Ok(int(1))
            }
            Kind::Enum { .. } => {
                let index = self.enum_index(node, datum).unwrap_or_default() as i64;
                self.emit_long(index)?;
                Ok(int(1))
            }
            Kind::Float | Kind::Double => {
                let value = if let Some(float) = magnus::Float::from_value(datum) {
                    float.to_f64()
                } else if integer(datum) {
                    unsafe { rb_sys::rb_num2dbl(datum.as_raw()) }
                } else if check::class_of(datum) == self.world.decimal_class()
                    && self.group(check::DECIMAL_VALUE)
                {
                    unsafe { rb_sys::rb_num2dbl(protect(|| rb_sys::rb_to_float(datum.as_raw()))?) }
                } else {
                    return self.encoder_call(node, datum);
                };
                if matches!(kind, Kind::Float) {
                    self.emit(&(value as f32).to_le_bytes())?;
                    Ok(int(4))
                } else {
                    self.emit(&value.to_le_bytes())?;
                    Ok(int(8))
                }
            }
            Kind::Bytes | Kind::Fixed { .. } => {
                let text = RString::try_convert(datum)?;
                if !self.has(datum, STRING_CAP) || !self.unconverted(text) {
                    return self.encoder_call(node, datum);
                }
                let bytes = unsafe { text.as_slice() };
                if matches!(kind, Kind::Bytes) {
                    self.emit_long(bytes.len() as i64)?;
                }
                self.emit(bytes)?;
                Ok(int(bytes.len() as i64))
            }
            Kind::Str => self.write_string(datum),
            Kind::Array { items } => self.write_array(node, *items, datum),
            Kind::Map { values } => self.write_map(node, *values, datum),
            Kind::Union { schemas, branches } => self.write_union(node, *schemas, branches, datum),
            Kind::Record { fields, list } => self.write_record(node, *fields, list, datum),
        }
    }

    fn encoder_call(&mut self, node: usize, datum: Value) -> R<Value> {
        let (schema, writer, encoder) = (self.plan.nodes[node].schema, self.writer, self.encoder);
        let step = match self.plan.nodes[node].kind {
            Kind::Boolean => Step::Encoder("write_boolean"),
            Kind::Int => Step::Encoder("write_int"),
            Kind::Long => Step::Encoder("write_long"),
            Kind::Float => Step::Encoder("write_float"),
            Kind::Double => Step::Encoder("write_double"),
            Kind::Bytes => Step::Encoder("write_bytes"),
            Kind::Str => Step::Encoder("write_string"),
            Kind::Fixed { .. } => Step::Writer("write_fixed"),
            Kind::Enum { .. } => Step::Writer("write_enum"),
            _ => Step::Encoder("write_null"),
        };
        self.call(|_| match step {
            Step::Encoder(method) => funcall(encoder, method, &[datum.as_raw()]),
            Step::Writer(method) => fcall(writer, method, &[schema, datum.as_raw(), encoder]),
        })
    }

    /// StringIO#write transcodes strings whose encoding differs from the stream's.
    fn unconverted(&self, text: RString) -> bool {
        let encoding = unsafe { rb_sys::rb_enc_get_index(text.as_raw()) };
        let (binary, ascii) = unsafe {
            (
                rb_sys::rb_ascii8bit_encindex(),
                rb_sys::rb_usascii_encindex(),
            )
        };
        [binary, ascii].contains(&self.io_encoding)
            || [binary, ascii, self.io_encoding].contains(&encoding)
    }

    fn write_string(&mut self, datum: Value) -> R<Value> {
        let Some(text) = RString::from_value(datum)
            .filter(|_| self.has(datum, VALUE_CAP) && self.has(datum, STRING_CAP))
        else {
            let encoder = self.encoder;
            return self.call(|_| funcall(encoder, "write_string", &[datum.as_raw()]));
        };
        let encoding = unsafe { rb_sys::rb_enc_get_index(text.as_raw()) };
        let (binary, ascii, utf8) = unsafe {
            (
                rb_sys::rb_ascii8bit_encindex(),
                rb_sys::rb_usascii_encindex(),
                rb_sys::rb_utf8_encindex(),
            )
        };
        let same = encoding == utf8
            || ([binary, ascii].contains(&encoding)
                && text.enc_coderange_scan() == Coderange::SevenBit);
        let text = if same {
            text
        } else {
            let utf8 = crate::allocate(|| self.ruby.str_new("utf-8"))?;
            match send(text.as_raw(), &ENCODE, &[utf8.as_raw()]) {
                Ok(value) => RString::try_convert(value)?,
                Err(error) => return Err(Fail::Raise(error)),
            }
        };
        if ![binary, ascii, utf8].contains(&self.io_encoding) {
            let encoder = self.encoder;
            return self.call(|_| fcall(encoder, "write_bytes", &[text.as_raw()]));
        }
        let bytes = unsafe { text.as_slice() };
        self.emit_long(bytes.len() as i64)?;
        self.emit(bytes)?;
        Ok(int(bytes.len() as i64))
    }

    fn write_long_value(&mut self, value: Value) -> R<Value> {
        if integer(value)
            && let Ok(value) = i64::try_convert(value)
            && self
                .caps
                .has(self.ruby.class_integer().as_raw(), INTEGER_CAP)
        {
            self.emit_long(value)?;
            return Ok(int(1));
        }
        let encoder = self.encoder;
        self.call(|_| funcall(encoder, "write_long", &[value.as_raw()]))
    }

    fn kind(&mut self, datum: Value, native: bool, name: &LazyId) -> R<bool> {
        if self.native() && self.has(datum, VALUE_CAP) {
            return Ok(native);
        }
        let cref = self.world.cref();
        self.call(|_| {
            funcall(
                datum.as_raw(),
                "is_a?",
                &[check::lexical(&cref, check::id(name))?],
            )
        })
        .map(|value| value.to_bool())
    }

    /// Ruby Avro re-reads `items` or `values` per element.
    fn element(&mut self, node: usize, child: usize, reader: &str, datum: Value) -> R<()> {
        if self.ready(node) {
            return self.child(child, datum).map(drop);
        }
        let schema = self.plan.nodes[node].schema;
        let target = self.call(|_| funcall(schema, reader, &[]))?;
        self.ruby_child(target.as_raw(), datum).map(drop)
    }

    fn write_array(&mut self, node: usize, items: usize, datum: Value) -> R<Value> {
        let values = RArray::from_value(datum);
        if !self.kind(datum, values.is_some(), &ARRAY)? {
            self.reject(node, datum)?;
        }
        if let Some(values) = values.filter(|_| self.has(datum, ARRAY_CAP)) {
            if !values.is_empty() {
                self.emit_long(values.len() as i64)?;
                let mut index = 0;
                while index < values.len() {
                    let item = raw(unsafe { rb_sys::rb_ary_entry(values.as_raw(), index as _) });
                    self.element(node, items, "items", item)?;
                    index += 1;
                }
            }
            return self.write_long_value(int(0));
        }
        let size = self.call(|_| funcall(datum.as_raw(), "size", &[]))?;
        if self.positive(size)? {
            let size = self.call(|_| funcall(datum.as_raw(), "size", &[]))?;
            self.write_long_value(size)?;
            self.each(datum, |this, args| {
                let item = args.first().copied().unwrap_or_else(nil);
                this.element(node, items, "items", item)
            })?;
        }
        self.write_long_value(int(0))
    }

    fn positive(&mut self, size: Value) -> R<bool> {
        if integer(size)
            && self
                .caps
                .has(self.ruby.class_integer().as_raw(), INTEGER_CAP)
        {
            return Ok(i64::try_convert(size).map_or(true, |size| size > 0));
        }
        let zero = int(0);
        self.call(|_| funcall(size.as_raw(), ">", &[zero.as_raw()]))
            .map(|value| value.to_bool())
    }

    fn each(
        &mut self,
        datum: Value,
        mut item: impl FnMut(&mut Self, &[Value]) -> R<()>,
    ) -> R<Value> {
        if self.fused > 0 {
            return Err(Fail::Abort);
        }
        self.flush()?;
        let mut failure = None;
        let ruby = self.ruby;
        let result = crate::callback::each(ruby, datum, |args| {
            if failure.is_some() {
                return Ok(());
            }
            self.refresh()?;
            match item(self, args).and_then(|()| self.flush().map_err(Fail::Raise)) {
                Ok(()) => Ok(()),
                Err(Fail::Raise(error)) | Err(Fail::Limit(error)) => Err(error),
                Err(other) => {
                    failure = Some(other);
                    Ok(())
                }
            }
        });
        self.refresh()?;
        if let Some(failure) = failure {
            return Err(failure);
        }
        result.map_err(Fail::Raise)
    }

    fn write_map(&mut self, node: usize, values: usize, datum: Value) -> R<Value> {
        let hash = magnus::RHash::from_value(datum);
        if !self.kind(datum, hash.is_some(), &HASH)? {
            self.reject(node, datum)?;
        }
        if let Some(hash) = hash.filter(|_| self.has(datum, MAP_CAP)) {
            if !hash.is_empty() {
                self.emit_long(hash.len() as i64)?;
                let mut failure = None;
                hash.foreach(|key: Value, value: Value| {
                    let step = self
                        .write_key(key)
                        .and_then(|()| self.element(node, values, "values", value));
                    Ok(match step {
                        Ok(()) => magnus::r_hash::ForEach::Continue,
                        Err(fail) => {
                            failure = Some(fail);
                            magnus::r_hash::ForEach::Stop
                        }
                    })
                })?;
                if let Some(failure) = failure {
                    return Err(failure);
                }
            }
            return self.write_long_value(int(0));
        }
        let size = self.call(|_| funcall(datum.as_raw(), "size", &[]))?;
        if self.positive(size)? {
            let size = self.call(|_| funcall(datum.as_raw(), "size", &[]))?;
            self.write_long_value(size)?;
            self.each(datum, |this, args| {
                let (key, value) = match args {
                    // `|k, v|` splats a lone argument through `to_ary`.
                    [pair] => {
                        let pair = *pair;
                        let array = if RArray::from_value(pair).is_some() {
                            pair
                        } else {
                            this.call(|_| {
                                Ok(raw(protect(|| unsafe {
                                    rb_sys::rb_check_array_type(pair.as_raw())
                                })?))
                            })?
                        };
                        match RArray::from_value(array) {
                            Some(pair) => (pair.entry(0)?, pair.entry(1)?),
                            None => (pair, nil()),
                        }
                    }
                    [key, value, ..] => (*key, *value),
                    [] => (nil(), nil()),
                };
                this.write_key(key)?;
                this.element(node, values, "values", value)
            })?;
        }
        self.write_long_value(int(0))
    }

    fn write_key(&mut self, key: Value) -> R<()> {
        if RString::from_value(key).is_some()
            && self.has(key, VALUE_CAP)
            && self.has(key, STRING_CAP)
        {
            return self.write_string(key).map(drop);
        }
        if self.fused > 0 {
            return Err(
                if RString::from_value(key).is_some() || !self.has(key, VALUE_CAP) {
                    Fail::Abort
                } else {
                    Fail::Invalid
                },
            );
        }
        let encoder = self.encoder;
        self.call(|_| funcall(encoder, "write_string", &[key.as_raw()]))
            .map(drop)
    }

    fn write_record(
        &mut self,
        node: usize,
        fields: VALUE,
        list: &[Field],
        datum: Value,
    ) -> R<Value> {
        if !self.kind(datum, magnus::RHash::from_value(datum).is_some(), &HASH)? {
            self.reject(node, datum)?;
        }
        if !self.ready(node) {
            let schema = self.plan.nodes[node].schema;
            let fields = self.call(|_| funcall(schema, "fields", &[]))?;
            return self.each(fields, |this, args| {
                let field = args.first().copied().unwrap_or_else(nil);
                this.ruby_field(field.as_raw(), datum)
            });
        }
        // `fields.each` sees in-place mutation of the live array.
        let mut index = 0;
        loop {
            if self.ready(node) {
                let Some(field) = list.get(index) else {
                    break;
                };
                let value = self.field(datum, field)?;
                self.child(field.node, value)?;
            } else {
                if index >= unsafe { rb_sys::RARRAY_LEN(fields) } as usize {
                    break;
                }
                self.ruby_field(unsafe { rb_sys::rb_ary_entry(fields, index as _) }, datum)?;
            }
            index += 1;
        }
        Ok(raw(fields))
    }

    fn ruby_field(&mut self, field: VALUE, datum: Value) -> R<()> {
        let datum = datum.as_raw();
        let (schema, value) = self.call(|_| {
            let schema = funcall(field, "type", &[])?;
            let name = funcall(field, "name", &[])?;
            let value = if funcall(datum, "key?", &[name.as_raw()])?.to_bool() {
                funcall(datum, "[]", &[funcall(field, "name", &[])?.as_raw()])?
            } else {
                let name = funcall(field, "name", &[])?;
                funcall(
                    datum,
                    "[]",
                    &[funcall(name.as_raw(), "to_sym", &[])?.as_raw()],
                )?
            };
            Ok((schema, value))
        })?;
        self.ruby_child(schema.as_raw(), value).map(drop)
    }

    fn field(&mut self, datum: Value, field: &Field) -> R<Value> {
        let (object, name, symbol) = (field.object, field.name.value, field.symbol);
        if let Some(hash) = magnus::RHash::from_value(datum).filter(|_| self.has(datum, LOOKUP_CAP))
        {
            for key in [name, symbol] {
                let found =
                    unsafe { rb_sys::rb_hash_lookup2(hash.as_raw(), key, rb_sys::Qundef as VALUE) };
                if found != rb_sys::Qundef as VALUE {
                    return Ok(raw(found));
                }
            }
            let default = send(hash.as_raw(), &DEFAULT_PROC, &[])?;
            if default.is_nil() {
                return Ok(raw(protect(|| unsafe {
                    rb_sys::rb_hash_aref(hash.as_raw(), symbol)
                })?));
            }
            return self.call(|_| {
                Ok(raw(protect(|| unsafe {
                    rb_sys::rb_hash_aref(hash.as_raw(), symbol)
                })?))
            });
        }
        // `key?` may run Ruby that renames the field; Ruby Avro reads `field.name` again after it.
        self.call(|_| {
            let name = if funcall(datum.as_raw(), "key?", &[name])?.to_bool() {
                funcall(object, "name", &[])?
            } else {
                funcall(funcall(object, "name", &[])?.as_raw(), "to_sym", &[])?
            };
            funcall(datum.as_raw(), "[]", &[name.as_raw()])
        })
    }

    fn write_union(
        &mut self,
        node: usize,
        schemas: VALUE,
        branches: &[usize],
        datum: Value,
    ) -> R<Value> {
        if self.native() {
            let outer = self.fused;
            let mark = self.mark();
            match self.fused_union(branches, datum) {
                Ok(value) => return Ok(value),
                Err(Fail::Invalid) if outer == 0 => return self.unmatched(node, datum),
                Err(Fail::Abort) if outer == 0 => self.rollback(mark),
                Err(fail) => return Err(fail),
            }
        }
        // `find_index` sees in-place mutation of the live array.
        let mut chosen = None;
        let mut index = 0;
        while index < unsafe { rb_sys::RARRAY_LEN(schemas) } as usize {
            self.charge()?;
            let schema = unsafe { rb_sys::rb_ary_entry(schemas, index as _) };
            let matched = match branches
                .get(index)
                .copied()
                .filter(|&branch| self.plan.nodes[branch].schema == schema)
            {
                Some(branch) if self.ready(branch) => {
                    if matches!(self.plan.nodes[branch].kind, Kind::Null) {
                        self.nil(datum)?
                    } else {
                        self.validate(branch, datum, false)?
                    }
                }
                _ => self.ruby_branch(schema, datum)?,
            };
            if matched {
                chosen = Some(index);
                break;
            }
            index += 1;
        }
        let Some(index) = chosen else {
            return self.unmatched(node, datum);
        };
        self.write_long_value(int(index as i64))?;
        // Ruby Avro re-reads `schemas` here.
        match branches.get(index) {
            Some(&branch) if self.ready(node) => self.write(branch, datum),
            _ => {
                let schema = self.plan.nodes[node].schema;
                let target = self.call(|_| {
                    funcall(
                        funcall(schema, "schemas", &[])?.as_raw(),
                        "[]",
                        &[int(index as i64).as_raw()],
                    )
                })?;
                self.ruby_write(target.as_raw(), datum)
            }
        }
    }

    fn nil(&mut self, datum: Value) -> R<bool> {
        if self.has(datum, VALUE_CAP) {
            return Ok(datum.is_nil());
        }
        self.call(|_| funcall(datum.as_raw(), "nil?", &[]))
            .map(|value| value.to_bool())
    }

    fn ruby_branch(&mut self, schema: VALUE, datum: Value) -> R<bool> {
        let null = self.ruby.to_symbol("null").as_raw();
        let symbol =
            self.call(|_| funcall(funcall(schema, "type_sym", &[])?.as_raw(), "==", &[null]))?;
        if symbol.to_bool() {
            return self.nil(datum);
        }
        self.validate_schema(schema, datum, false)
    }

    /// A `raise` that returns leaves Ruby Avro with a nil index.
    fn unmatched(&mut self, node: usize, datum: Value) -> R<Value> {
        self.reject(node, datum)?;
        let (schema, encoder, nil) = (
            self.plan.nodes[node].schema,
            self.encoder,
            rb_sys::Qnil as VALUE,
        );
        self.call(|_| funcall(encoder, "write_long", &[nil]))?;
        let target =
            self.call(|_| funcall(funcall(schema, "schemas", &[])?.as_raw(), "[]", &[nil]))?;
        self.ruby_write(target.as_raw(), datum)
    }

    fn charge(&mut self) -> R<()> {
        self.work -= 1;
        if self.work < 0 {
            return Err(self.limit("union search exceeds maximum item count"));
        }
        Ok(())
    }

    fn fused_union(&mut self, branches: &[usize], datum: Value) -> R<Value> {
        self.fused += 1;
        let result = self.fused_branches(branches, datum);
        self.fused -= 1;
        result
    }

    fn fused_branches(&mut self, branches: &[usize], datum: Value) -> R<Value> {
        if !self.has(datum, VALUE_CAP)
            || !self
                .caps
                .has(self.ruby.class_integer().as_raw(), INTEGER_CAP)
        {
            return Err(Fail::Abort);
        }
        let plan = self.plan;
        for (index, &branch) in branches.iter().enumerate() {
            self.charge()?;
            if matches!(plan.nodes[branch].kind, Kind::Null) {
                if datum.is_nil() {
                    self.emit_long(index as i64)?;
                    return Ok(nil());
                }
                continue;
            }
            let mark = self.mark();
            self.emit_long(index as i64)?;
            match self.write(branch, datum) {
                Ok(value) => return Ok(value),
                Err(Fail::Invalid) => {
                    self.rollback(mark);
                    // Ruby Avro rejected the branch only after validating all of it.
                    if self.walk(branch, datum)? != Some(false) {
                        return Err(Fail::Abort);
                    }
                }
                // Ruby Avro raises a write error only for the branch its validation chose.
                Err(Fail::Raise(error)) => match self.walk(branch, datum)? {
                    Some(true) => return Err(Fail::Raise(error)),
                    Some(false) => self.rollback(mark),
                    None => {
                        self.rollback(mark);
                        return Err(Fail::Abort);
                    }
                },
                Err(fail) => {
                    self.rollback(mark);
                    return Err(fail);
                }
            }
        }
        Err(Fail::Invalid)
    }

    /// `validate_recursive`'s verdict; `None` once Ruby could observe it, `inspect` messages included.
    fn walk(&mut self, node: usize, datum: Value) -> Result<Option<bool>, Error> {
        if self.depth > 2 * self.max_depth {
            return Ok(None);
        }
        self.depth += 1;
        let result = self.walk_node(node, datum);
        self.depth -= 1;
        result
    }

    fn mismatch(&mut self, value: Value) -> Option<bool> {
        self.inert(value).then_some(false)
    }

    fn walk_node(&mut self, node: usize, datum: Value) -> Result<Option<bool>, Error> {
        if !self.ready(node) {
            return Ok(None);
        }
        let datum = match self.plan.nodes[node].adapter {
            Adapter::Identity => datum,
            Adapter::Custom | Adapter::Pending => return Ok(None),
            _ => {
                self.fused += 1;
                let adapted = self.adapt(node, datum);
                self.fused -= 1;
                match adapted {
                    Ok(value) => value,
                    Err(Fail::Invalid) => nil(),
                    Err(Fail::Raise(error))
                        if error.is_kind_of(self.ruby.exception_standard_error()) =>
                    {
                        nil()
                    }
                    Err(Fail::Raise(error)) => return Err(error),
                    Err(_) => return Ok(None),
                }
            }
        };
        if !self.has(datum, VALUE_CAP) || (integer(datum) && !self.has(datum, INTEGER_CAP)) {
            return Ok(None);
        }
        // Every element, entry and field is walked even after a failure, as Ruby Avro's validator does.
        let fold = |verdict: Option<bool>, next: Option<bool>| Some(verdict? & next?);
        let plan = self.plan;
        match &plan.nodes[node].kind {
            Kind::Array { items } => {
                let Some(values) = RArray::from_value(datum) else {
                    return Ok(self.mismatch(datum));
                };
                if !self.has(datum, WALK_CAP) {
                    return Ok(None);
                }
                let mut verdict = Some(true);
                let mut index = 0;
                while index < values.len() && verdict.is_some() {
                    let item = raw(unsafe { rb_sys::rb_ary_entry(values.as_raw(), index as _) });
                    verdict = fold(verdict, self.walk(*items, item)?);
                    index += 1;
                }
                Ok(verdict)
            }
            Kind::Map { values } => {
                let Some(hash) = magnus::RHash::from_value(datum) else {
                    return Ok(self.mismatch(datum));
                };
                if !self.has(datum, WALK_CAP) {
                    return Ok(None);
                }
                let mut entries = Vec::with_capacity(hash.len());
                hash.foreach(|key: Value, value: Value| {
                    entries.push((key, value));
                    Ok(magnus::r_hash::ForEach::Continue)
                })?;
                let mut verdict = Some(true);
                for &(key, _) in &entries {
                    if !self.has(key, VALUE_CAP) || !self.has(key, INERT_CAP) {
                        return Ok(None);
                    }
                    if RString::from_value(key).is_none() {
                        verdict = Some(false);
                    }
                }
                for (_, value) in entries {
                    verdict = fold(verdict, self.walk(*values, value)?);
                }
                Ok(verdict)
            }
            Kind::Record { list, .. } => {
                let Some(hash) = magnus::RHash::from_value(datum) else {
                    return Ok(self.mismatch(datum));
                };
                if !self.has(datum, WALK_CAP) {
                    return Ok(None);
                }
                let mut verdict = Some(true);
                for field in list {
                    self.fused += 1;
                    let value = self.field(hash.as_value(), field);
                    self.fused -= 1;
                    let value = match value {
                        Ok(value) => value,
                        Err(Fail::Raise(error)) => return Err(error),
                        Err(_) => return Ok(None),
                    };
                    verdict = fold(verdict, self.walk(field.node, value)?);
                }
                Ok(verdict)
            }
            Kind::Union { branches, .. } => {
                if branches.len() == 1 {
                    return self.walk(branches[0], datum);
                }
                let mut complex = false;
                for &branch in branches {
                    if matches!(plan.nodes[branch].kind, Kind::Null) {
                        if datum.is_nil() {
                            return Ok(Some(true));
                        }
                        continue;
                    }
                    match self.walk(branch, datum)? {
                        Some(true) => return Ok(Some(true)),
                        Some(false) => {
                            complex |= matches!(
                                plan.nodes[branch].kind,
                                Kind::Array { .. } | Kind::Map { .. } | Kind::Record { .. }
                            );
                        }
                        None => return Ok(None),
                    }
                }
                // Without a failed complex branch, the union's own error inspects the value.
                Ok(if complex {
                    Some(false)
                } else {
                    self.mismatch(datum)
                })
            }
            _ => {
                self.fused += 1;
                let simple = self.simple(node, datum);
                self.fused -= 1;
                match simple {
                    Ok(true) => Ok(Some(true)),
                    Ok(false) => Ok(self.mismatch(datum)),
                    Err(Fail::Raise(error)) => Err(error),
                    Err(_) => Ok(None),
                }
            }
        }
    }

    /// Nested union rejections ask about the same subtrees; answers hold until Ruby runs or GC
    /// compaction can reuse an address.
    fn inert(&mut self, value: Value) -> bool {
        let stamp = (self.generation, unsafe { rb_sys::rb_gc_count() } as u64);
        if self.inert_stamp != stamp {
            self.inert_cache.clear();
            self.inert_stamp = stamp;
        }
        let mut path = Vec::new();
        self.inspectable(value, &mut path)
    }

    /// `path` stops where `inspect` prints `[...]`; a subtree that hit it is cached nowhere.
    fn inspectable(&mut self, value: Value, path: &mut Vec<VALUE>) -> bool {
        let raw_value = value.as_raw();
        if !self.has(value, VALUE_CAP) || !self.has(value, INERT_CAP) {
            return false;
        }
        if rb_sys::SPECIAL_CONST_P(raw_value) {
            return !integer(value) || self.has(value, INTEGER_CAP);
        }
        if path.contains(&raw_value) {
            self.back_edges += 1;
            return true;
        }
        let class = check::class_of(value);
        if RString::from_value(value).is_some() {
            self.has(value, STRING_CAP)
        } else if integer(value) {
            self.has(value, INTEGER_CAP)
        } else if float(value) || magnus::Symbol::from_value(value).is_some() {
            true
        } else if let Some(values) = RArray::from_value(value) {
            if !self.has(value, WALK_CAP) {
                return false;
            }
            if let Some(&known) = self.inert_cache.get(&raw_value) {
                return known;
            }
            let edges = self.back_edges;
            path.push(raw_value);
            let mut index = 0;
            let mut inert = true;
            while inert && index < values.len() {
                inert = self.inspectable(
                    raw(unsafe { rb_sys::rb_ary_entry(values.as_raw(), index as _) }),
                    path,
                );
                index += 1;
            }
            path.pop();
            if self.back_edges == edges {
                self.inert_cache.insert(raw_value, inert);
            }
            inert
        } else if let Some(hash) = magnus::RHash::from_value(value) {
            if !self.has(value, WALK_CAP) {
                return false;
            }
            if let Some(&known) = self.inert_cache.get(&raw_value) {
                return known;
            }
            let edges = self.back_edges;
            path.push(raw_value);
            let mut inert = true;
            let _ = hash.foreach(|key: Value, item: Value| {
                inert = self.inspectable(key, path) && self.inspectable(item, path);
                Ok(if inert {
                    magnus::r_hash::ForEach::Continue
                } else {
                    magnus::r_hash::ForEach::Stop
                })
            });
            path.pop();
            if self.back_edges == edges {
                self.inert_cache.insert(raw_value, inert);
            }
            inert
        } else {
            class == self.world.decimal_class() || class == self.world.time
        }
    }

    fn decimal_ready(&mut self) -> Result<bool, Error> {
        if let Some(ready) = self.unlimited {
            return Ok(ready);
        }
        let limit = send(self.world.decimal_class(), &check::LIMIT, &[])?;
        let ready = rb_sys::FIXNUM_P(limit.as_raw()) && i64::try_convert(limit)? == 0;
        self.unlimited = Some(ready);
        Ok(ready)
    }

    /// BytesDecimal multiplies by `@factor`, not by `10**scale`.
    fn unit_factor(&mut self, factor: VALUE, scale: i64) -> Result<bool, Error> {
        if let Some(&(_, _, unit)) = self
            .factors
            .iter()
            .find(|entry| entry.0 == factor && entry.1 == scale)
        {
            return Ok(unit);
        }
        let unit = check::class_raw(factor) == self.world.decimal_class() && {
            let parts = RArray::try_convert(send(factor, &check::SPLIT, &[])?)?;
            let digits = RString::from_value(parts.entry(1)?);
            magnus::Fixnum::from_value(parts.entry(0)?).is_some_and(|sign| sign.to_i64() == 1)
                && digits.is_some_and(|digits| unsafe { digits.as_slice() } == b"1")
                && magnus::Fixnum::from_value(parts.entry(3)?)
                    .is_some_and(|exponent| exponent.to_i64() == scale + 1)
        };
        self.factors.push((factor, scale, unit));
        Ok(unit)
    }

    fn native_decimal(
        &mut self,
        datum: Value,
        precision: i64,
        scale: i64,
        factor: VALUE,
    ) -> R<Option<Vec<u8>>> {
        if !self.group(check::DECIMAL)
            || !self.has(datum, VALUE_CAP)
            || !self.decimal_ready()?
            || !self.unit_factor(factor, scale)?
        {
            return Ok(None);
        }
        let Some((negative, digits, exponent)) = self.decimal_parts(datum)? else {
            return Ok(None);
        };
        let length = digits.len() as i64;
        let fractional = length - exponent;
        // Its `raise` resolves constants in BytesDecimal's scope: Ruby replays the encode.
        if fractional > scale || length > precision || exponent > precision - scale {
            return if self.fused > 0 {
                Err(Fail::Invalid)
            } else {
                Ok(None)
            };
        }
        let mut unscaled = String::with_capacity(digits.len() + (scale - fractional) as usize + 1);
        if negative && digits != "0" {
            unscaled.push('-');
        }
        unscaled.push_str(&digits);
        unscaled.extend(std::iter::repeat_n('0', (scale - fractional) as usize));
        Ok(
            BigInt::parse_bytes(unscaled.as_bytes(), 10)
                .map(|integer| integer.to_signed_bytes_be()),
        )
    }

    fn decimal_parts(&mut self, value: Value) -> R<Option<(bool, String, i64)>> {
        if let Some(float) = magnus::Float::from_value(value) {
            let float = float.to_f64();
            if !float.is_finite() {
                return Ok(None);
            }
            if float == 0.0 {
                return Ok(Some((false, "0".into(), 0)));
            }
            let Some((mut digits, exponent)) = shortest_digits(float) else {
                return Ok(None);
            };
            digits.truncate(16);
            let digits = digits.trim_end_matches('0');
            return Ok(Some((float < 0.0, digits.into(), exponent)));
        }
        if let Some(integer) = magnus::Fixnum::from_value(value) {
            if !self.has(value, INTEGER_CAP) {
                return Ok(None);
            }
            let integer = integer.to_i64();
            if integer == 0 {
                return Ok(Some((false, "0".into(), 0)));
            }
            let text = (integer as i128).unsigned_abs().to_string();
            let exponent = text.len() as i64;
            return Ok(Some((
                integer < 0,
                text.trim_end_matches('0').into(),
                exponent,
            )));
        }
        if check::class_of(value) != self.world.decimal_class() {
            return Ok(None);
        }
        let parts = RArray::try_convert(send(value.as_raw(), &check::SPLIT, &[])?)?;
        let (Some(sign), Some(digits), Some(exponent)) = (
            magnus::Fixnum::from_value(parts.entry(0)?),
            RString::from_value(parts.entry(1)?),
            magnus::Fixnum::from_value(parts.entry(3)?),
        ) else {
            return Ok(None);
        };
        let digits = unsafe { digits.as_slice() };
        if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
            return Ok(None);
        }
        let digits = String::from_utf8_lossy(digits).into_owned();
        Ok(Some((sign.to_i64() < 0, digits, exponent.to_i64())))
    }
}

static USEC: LazyId = LazyId::new("usec");
static NSEC: LazyId = LazyId::new("nsec");
