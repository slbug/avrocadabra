use crate::{
    Core,
    big_decimal::write_long,
    error,
    guard::Limits,
    mapping::{self, Mapping},
    namespace,
};
use apache_avro::{
    Schema,
    schema::{InnerDecimalSchema, NamesRef, NamespaceRef, RecordSchema, UuidSchema},
};
use magnus::{
    Error, RArray, RClass, RHash, RModule, RString, Ruby, TryConvert, Value,
    encoding::{Coderange, EncodingCapable, Index},
    prelude::*,
    r_hash::ForEach,
    rb_sys::{AsRawId, AsRawValue, FromRawValue},
    value::{Id, IntoId, LazyId},
};
use num_bigint::BigInt;
use rb_sys::VALUE;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    fmt::Write,
    sync::atomic::Ordering,
};

pub fn encode(
    ruby: &Ruby,
    core: &Core,
    value: Value,
    keys: RArray,
    mapping: Option<Mapping>,
) -> Result<Vec<u8>, Error> {
    let module = namespace(ruby)?;
    let codec = core.prepared.borrow_codec();
    let decimal: RClass = ruby.class_object().const_get("BigDecimal")?;
    let date: RClass = ruby.class_object().const_get("Date")?;
    let date_time: RClass = ruby.class_object().const_get("DateTime")?;
    let union: RClass = module.const_get("Union")?;
    let duration: RClass = module.const_get("Duration")?;
    let classes = Classes {
        array: ruby.class_array().as_raw(),
        string: ruby.class_string().as_raw(),
        symbol: ruby.class_symbol().as_raw(),
        integer: ruby.class_integer().as_raw(),
        float: ruby.class_float().as_raw(),
        rational: ruby.class_rational().as_raw(),
        time: ruby.class_time().as_raw(),
        decimal: decimal.as_raw(),
        date: date.as_raw(),
        date_time: date_time.as_raw(),
        union: union.as_raw(),
        duration: duration.as_raw(),
    };
    let mut ctx = Encoder {
        ruby,
        names: codec.resolved.get_names(),
        keys,
        records: &core.records,
        limits: core.limits,
        path: "$".into(),
        out: Vec::with_capacity(core.encoded_size.load(Ordering::Relaxed)),
        items: 0,
        work: 0,
        bytes: 0,
        logical: module.const_get("Logical")?,
        union,
        duration,
        decimal,
        date,
        error_class: module.const_get("EncodeError")?,
        hash_key: ruby.intern("key?"),
        hash_read: ruby.intern("[]"),
        encode: ruby.intern("encode"),
        utf8: ruby.utf8_encindex(),
        ascii: ruby.usascii_encindex(),
        binary: ruby.ascii8bit_encindex(),
        classes,
        dispatch: RefCell::new(Vec::new()),
        core: Cell::new(None),
        decimals: Cell::new(None),
        modules: RefCell::new(Vec::new()),
        unlimited: Cell::new(None),
        bytes_decimal: Cell::new(None),
        speculating: false,
    };
    ctx.value(codec.schema, None, value, 0, mapping)?;
    if ctx.out.len() > core.limits.max_bytes {
        return Err(error(
            ruby,
            "EncodeError",
            "encoded datum exceeds max_bytes",
        ));
    }
    core.encoded_size.store(ctx.out.len(), Ordering::Relaxed);
    Ok(ctx.out)
}

struct Classes {
    array: VALUE,
    string: VALUE,
    symbol: VALUE,
    integer: VALUE,
    float: VALUE,
    rational: VALUE,
    time: VALUE,
    decimal: VALUE,
    date: VALUE,
    date_time: VALUE,
    union: VALUE,
    duration: VALUE,
}

#[derive(Clone, Copy, PartialEq)]
enum Op {
    Sequence,
    Map,
    Lookup,
    Text,
    Validate,
}

static EACH: LazyId = LazyId::new("each");
static SIZE: LazyId = LazyId::new("size");
static EACH_WITH_INDEX: LazyId = LazyId::new("each_with_index");
static KEYS: LazyId = LazyId::new("keys");
static KEY: LazyId = LazyId::new("key?");
static AREF: LazyId = LazyId::new("[]");
static DEFAULT: LazyId = LazyId::new("default");
static DEFAULT_PROC: LazyId = LazyId::new("default_proc");
static ENCODE: LazyId = LazyId::new("encode");
static TO_S: LazyId = LazyId::new("to_s");
static IS_A: LazyId = LazyId::new("is_a?");
static NIL: LazyId = LazyId::new("nil?");
static CLASS: LazyId = LazyId::new("class");
static INSPECT: LazyId = LazyId::new("inspect");
static RESPOND_TO: LazyId = LazyId::new("respond_to?");
static EQL: LazyId = LazyId::new("eql?");
static EQUAL: LazyId = LazyId::new("==");
static INDEX: LazyId = LazyId::new("index");
static AND: LazyId = LazyId::new("&");
static SHIFT: LazyId = LazyId::new(">>");
static TIMES: LazyId = LazyId::new("*");
static MINUS: LazyId = LazyId::new("-");
static PLUS: LazyId = LazyId::new("+");
static DIVIDE: LazyId = LazyId::new("/");
static GREATER: LazyId = LazyId::new(">");
static TO_I: LazyId = LazyId::new("to_i");
static UNSHIFT: LazyId = LazyId::new("unshift");
static FIRST: LazyId = LazyId::new("first");
static PACK: LazyId = LazyId::new("pack");
static LENGTH: LazyId = LazyId::new("length");
static FREEZE: LazyId = LazyId::new("freeze");
static USEC: LazyId = LazyId::new("usec");
static NSEC: LazyId = LazyId::new("nsec");

static SEQUENCE: [&LazyId; 3] = [&EACH, &SIZE, &EACH_WITH_INDEX];
static MAP: [&LazyId; 3] = [&EACH, &SIZE, &KEYS];
static LOOKUP: [&LazyId; 4] = [&KEY, &AREF, &DEFAULT, &DEFAULT_PROC];
static TEXT: [&LazyId; 2] = [&ENCODE, &TO_S];
static VALIDATE: [&LazyId; 5] = [&IS_A, &NIL, &CLASS, &INSPECT, &RESPOND_TO];

impl Op {
    fn methods(self) -> &'static [&'static LazyId] {
        match self {
            Self::Sequence => &SEQUENCE,
            Self::Map => &MAP,
            Self::Lookup => &LOOKUP,
            Self::Text => &TEXT,
            Self::Validate => &VALIDATE,
        }
    }
}

fn basic(class: VALUE, names: &[&LazyId]) -> bool {
    let ruby = unsafe { Ruby::get_unchecked() };
    names.iter().all(|name| unsafe {
        rb_sys::rb_method_basic_definition_p(class, (***name).into_id_with(&ruby).as_raw()) != 0
    })
}

fn class_of(value: Value) -> Option<VALUE> {
    let raw = value.as_raw();
    (!rb_sys::SPECIAL_CONST_P(raw)).then(|| unsafe { (*(raw as *const rb_sys::RBasic)).klass })
}

fn shortest_digits(value: f64) -> Option<(String, i64)> {
    let text = format!("{value:e}");
    let (mantissa, exponent) = text.split_once('e')?;
    let exponent = exponent.parse::<i64>().ok()? + 1;
    let mut digits: Vec<u8> = mantissa.bytes().filter(|&byte| byte != b'.').collect();
    let last = *digits.last()?;
    if (last - b'0') % 2 == 1 && halfway(value, &digits, exponent) {
        *digits.last_mut()? -= 1;
    }
    Some((String::from_utf8(digits).ok()?, exponent))
}

fn halfway(value: f64, digits: &[u8], exponent: i64) -> bool {
    let shift = digits.len() as i64 - exponent;
    let bits = value.to_bits();
    let biased = ((bits >> 52) & 0x7ff) as i64;
    let fraction = bits & ((1 << 52) - 1);
    let (mantissa, power) = if biased == 0 {
        (fraction, -1074)
    } else {
        (fraction | (1 << 52), biased - 1075)
    };
    let zeros = mantissa.trailing_zeros() as i64;
    if zeros + power + 1 + shift != 0 {
        return false;
    }
    let odd = BigInt::from(mantissa >> zeros);
    let five = BigInt::from(5).pow(shift.unsigned_abs() as u32);
    let doubled = if shift >= 0 {
        odd * five
    } else if (&odd % &five) == BigInt::ZERO {
        odd / five
    } else {
        return false;
    };
    BigInt::parse_bytes(digits, 10).is_some_and(|candidate| doubled == candidate * 2 - 1)
}

fn integer(value: Value) -> bool {
    magnus::Integer::from_value(value).is_some()
}

fn float(value: Value) -> bool {
    magnus::Float::from_value(value).is_some()
}

fn boolean(value: Value) -> bool {
    let raw = value.as_raw();
    raw == rb_sys::Qtrue as VALUE || raw == rb_sys::Qfalse as VALUE
}

struct Encoder<'a, 's> {
    ruby: &'a Ruby,
    names: &'a NamesRef<'s>,
    keys: RArray,
    records: &'a HashMap<usize, Vec<usize>>,
    limits: Limits,
    path: String,
    out: Vec<u8>,
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
    encode: Id,
    utf8: Index,
    ascii: Index,
    binary: Index,
    classes: Classes,
    dispatch: RefCell<Vec<(VALUE, Op, bool)>>,
    core: Cell<Option<bool>>,
    decimals: Cell<Option<bool>>,
    modules: RefCell<Vec<(VALUE, bool)>>,
    unlimited: Cell<Option<bool>>,
    bytes_decimal: Cell<Option<VALUE>>,
    speculating: bool,
}

impl Encoder<'_, '_> {
    fn allows(&self, class: VALUE, op: Op) -> bool {
        let mut dispatch = self.dispatch.borrow_mut();
        if let Some(&(_, _, allowed)) = dispatch
            .iter()
            .find(|entry| entry.0 == class && entry.1 == op)
        {
            return allowed;
        }
        let allowed = basic(class, op.methods());
        dispatch.push((class, op, allowed));
        allowed
    }

    fn plain(&self, value: Value, op: Op) -> bool {
        class_of(value).is_some_and(|class| self.allows(class, op))
    }

    fn core(&self) -> bool {
        if let Some(core) = self.core.get() {
            return core;
        }
        let c = &self.classes;
        let core = basic(c.string, &[&EQL, &EQUAL])
            && basic(c.symbol, &[&EQL])
            && basic(c.array, &[&INDEX]);
        self.core.set(Some(core));
        core
    }

    fn trusted(&self, value: Value) -> bool {
        let Some(class) = class_of(value) else {
            return true;
        };
        let c = &self.classes;
        if [c.decimal, c.date, c.date_time, c.time].contains(&class) {
            return true;
        }
        (RString::from_value(value).is_some()
            || RArray::from_value(value).is_some()
            || RHash::from_value(value).is_some()
            || integer(value)
            || float(value)
            || magnus::Symbol::from_value(value).is_some())
            && self.allows(class, Op::Validate)
    }

    fn refresh(&self) {
        self.dispatch.borrow_mut().clear();
        self.core.set(None);
        self.decimals.set(None);
        self.modules.borrow_mut().clear();
        self.unlimited.set(None);
    }

    fn bytes_decimal(&self) -> Result<VALUE, Error> {
        if let Some(class) = self.bytes_decimal.get() {
            return Ok(class);
        }
        let logical: RModule = self
            .ruby
            .class_object()
            .const_get::<_, RModule>("Avro")?
            .const_get("LogicalTypes")?;
        let class = logical.const_get::<_, RClass>("BytesDecimal")?.as_raw();
        self.bytes_decimal.set(Some(class));
        Ok(class)
    }

    fn cached(
        cell: &Cell<Option<bool>>,
        check: impl FnOnce() -> Result<bool, Error>,
    ) -> Result<bool, Error> {
        if let Some(value) = cell.get() {
            return Ok(value);
        }
        let value = check()?;
        cell.set(Some(value));
        Ok(value)
    }

    fn decimal_adapter(&self, mapping: Mapping) -> Result<bool, Error> {
        Ok(class_of(mapping.adapter()) == Some(self.bytes_decimal()?))
    }

    fn stock_adapter(&self, mapping: Mapping, value: Value) -> Result<bool, Error> {
        if !mapping.builtin() {
            return Ok(false);
        }
        if self.decimal_adapter(mapping)? {
            let c = &self.classes;
            return Self::cached(&self.decimals, || {
                Ok(basic(
                    c.integer,
                    &[&AND, &SHIFT, &AREF, &EQUAL, &TIMES, &MINUS, &GREATER, &TO_I],
                ) && basic(c.array, &[&UNSHIFT, &FIRST, &PACK])
                    && basic(c.string, &[&LENGTH, &FREEZE])
                    && mapping.native_decimal()?)
            });
        }
        let c = &self.classes;
        let class = class_of(value).unwrap_or_else(|| value.class().as_raw());
        if let Some(&(_, stock)) = self.modules.borrow().iter().find(|entry| entry.0 == class) {
            return Ok(stock);
        }
        let stock = basic(c.integer, &[&TO_I, &TIMES, &PLUS, &DIVIDE])
            && basic(c.float, &[&TO_I])
            && basic(c.rational, &[&TO_I])
            && basic(c.time, &[&TO_I, &USEC, &NSEC])
            && mapping.stock_modules(unsafe { Value::from_raw(class) })?;
        self.modules.borrow_mut().push((class, stock));
        Ok(stock)
    }

    fn native_decimal(&self, mapping: Mapping, value: Value) -> Result<Option<Vec<u8>>, Error> {
        if !self.decimal_adapter(mapping)? {
            return Ok(None);
        }
        let decimal = self.decimal;
        let unlimited = Self::cached(&self.unlimited, || {
            let limit: Value = decimal.funcall("limit", ())?;
            Ok(magnus::Fixnum::from_value(limit).is_some_and(|limit| limit.to_i64() == 0))
        })?;
        if !unlimited {
            return Ok(None);
        }
        let adapter = mapping.adapter();
        let Some((negative, digits, exponent)) = self.decimal_parts(value)? else {
            return Ok(None);
        };
        let ivar = |name: &str| unsafe {
            let id = rb_sys::rb_intern2(name.as_ptr().cast(), name.len() as _);
            magnus::Fixnum::from_value(Value::from_raw(rb_sys::rb_ivar_get(adapter.as_raw(), id)))
                .map(|value| value.to_i64())
        };
        let (Some(precision), Some(scale)) = (ivar("@precision"), ivar("@scale")) else {
            return Ok(None);
        };
        let length = digits.len() as i64;
        let fractional = length - exponent;
        if fractional > scale {
            return Err(Error::new(
                self.ruby.exception_range_error(),
                "Rounding necessary",
            ));
        }
        if length > precision || exponent > precision - scale {
            return Err(Error::new(
                self.ruby.exception_range_error(),
                "Precision is too small",
            ));
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

    fn decimal_parts(&self, value: Value) -> Result<Option<(bool, String, i64)>, Error> {
        if let Some(float) = magnus::Float::from_value(value) {
            let float = float.to_f64();
            if !float.is_finite() {
                return Ok(None);
            }
            if float == 0.0 {
                return Ok(Some((false, "0".into(), 0)));
            }
            let Some((mut digits, exponent)) = shortest_digits(float.abs()) else {
                return Ok(None);
            };
            digits.truncate(16);
            let digits = digits.trim_end_matches('0');
            return Ok(Some((float < 0.0, digits.into(), exponent)));
        }
        if let Some(integer) = magnus::Fixnum::from_value(value) {
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
        if class_of(value) != Some(self.classes.decimal) {
            return Ok(None);
        }
        let parts: RArray = value.funcall("split", ())?;
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

    fn adapter_value(&mut self, mapping: Mapping, value: Value) -> Result<Value, Error> {
        if let Some(bytes) = self.native_decimal(mapping, value)? {
            let text = crate::allocate(|| {
                let text = self.ruby.str_from_slice(&bytes);
                text.freeze();
                text
            })?;
            return Ok(text.as_value());
        }
        mapping.convert(self.encode, value)
    }

    fn abort(&self) -> Error {
        self.fail("union attempt reached a Ruby callback")
    }

    fn callback<T>(
        &mut self,
        call: impl FnOnce(&mut Self) -> Result<T, Error>,
    ) -> Result<T, Error> {
        if self.speculating {
            return Err(self.abort());
        }
        let result = call(self);
        self.refresh();
        result
    }

    fn emit(&mut self, bytes: &[u8]) {
        self.out.extend_from_slice(bytes);
    }

    fn emit_long(&mut self, value: i64) {
        write_long(value, &mut self.out);
    }

    fn emit_bytes(&mut self, bytes: &[u8]) {
        self.emit_long(bytes.len() as i64);
        self.emit(bytes);
    }

    fn field_value(&mut self, data: RHash, key: Value, symbol: Value) -> Result<Value, Error> {
        if class_of(key) == Some(self.classes.string)
            && self.plain(data.as_value(), Op::Lookup)
            && self.core()
        {
            for candidate in [key, symbol] {
                let found = unsafe {
                    rb_sys::rb_hash_lookup2(
                        data.as_raw(),
                        candidate.as_raw(),
                        rb_sys::Qundef as VALUE,
                    )
                };
                if found != rb_sys::Qundef as VALUE {
                    return Ok(unsafe { Value::from_raw(found) });
                }
            }
            let default: Value = data.funcall("default_proc", ())?;
            if default.is_nil() {
                return data.aref(symbol);
            }
            return self.callback(|_| data.aref(symbol));
        }
        let exact = class_of(key) == Some(self.classes.string);
        self.callback(|this| {
            let present: bool = data.funcall_public(this.hash_key, (key,))?;
            if present {
                return data.funcall_public(this.hash_read, (key,));
            }
            let symbol = if exact {
                symbol
            } else {
                key.funcall_public("to_sym", ())?
            };
            data.funcall_public(this.hash_read, (symbol,))
        })
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

    fn overflowed(&self) -> bool {
        self.out.len() > self.limits.max_bytes
    }

    fn charge_work(&mut self) -> Result<(), Error> {
        self.work += 1;
        if self.work > self.limits.max_items {
            return Err(self.fail("union search exceeds maximum item count"));
        }
        Ok(())
    }

    fn integer(&self, value: Value) -> Result<i64, Error> {
        if !integer(value) {
            return Err(self.fail("expected Integer"));
        }
        self.ruby_result(i64::try_convert(value))
    }

    fn int32(&self, value: Value) -> Result<i32, Error> {
        i32::try_from(self.integer(value)?).map_err(|_| self.fail("Avro int out of range"))
    }

    fn float(&self, value: Value) -> Result<f64, Error> {
        if !float(value) && !integer(value) && !value.is_kind_of(self.decimal) {
            return Err(self.fail("expected Float, Integer or BigDecimal"));
        }
        self.ruby_result(f64::try_convert(value))
    }

    fn write_text(&mut self, value: Value) -> Result<usize, Error> {
        let text = RString::from_value(value).ok_or_else(|| self.fail("expected UTF-8 String"))?;
        let text = if self.plain(value, Op::Text) {
            let encoding = text.enc_get();
            if encoding == self.utf8
                || ((encoding == self.ascii || encoding == self.binary)
                    && text.enc_coderange_scan() == Coderange::SevenBit)
            {
                text
            } else {
                text.funcall_public(self.encode, ("UTF-8",))?
            }
        } else {
            self.callback(|this| text.funcall_public(this.encode, ("UTF-8",)))?
        };
        self.count_bytes(text.len())?;
        let text = self.ruby_result(unsafe { text.as_str() })?;
        write_long(text.len() as i64, &mut self.out);
        let start = self.out.len();
        self.out.extend_from_slice(text.as_bytes());
        Ok(start)
    }

    fn binary(&mut self, value: Value, message: &str) -> Result<RString, Error> {
        let value = RString::from_value(value).ok_or_else(|| self.fail(message))?;
        self.count_bytes(value.len())?;
        Ok(value)
    }

    fn logical<T: TryConvert>(
        &mut self,
        call: &str,
        args: impl magnus::ArgList,
    ) -> Result<T, Error> {
        let logical = self.logical;
        let result = self.callback(|_| logical.funcall(call, args));
        self.ruby_result(result)
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
            Schema::Boolean => boolean(value),
            Schema::Int
            | Schema::Long
            | Schema::TimeMillis
            | Schema::TimeMicros
            | Schema::LocalTimestampMillis
            | Schema::LocalTimestampMicros
            | Schema::LocalTimestampNanos => integer(value),
            Schema::Float | Schema::Double => {
                float(value) || integer(value) || value.is_kind_of(self.decimal)
            }
            Schema::String
            | Schema::Bytes
            | Schema::Enum(_)
            | Schema::Fixed(_)
            | Schema::Uuid(_) => RString::from_value(value).is_some(),
            Schema::Array(_) => RArray::from_value(value).is_some(),
            Schema::Map(_) | Schema::Record(_) => RHash::from_value(value).is_some(),
            Schema::Decimal(_) | Schema::BigDecimal => {
                value.is_kind_of(self.decimal) || integer(value) || float(value)
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

    fn admits(&self, schema: &Schema, ns: NamespaceRef<'_>, value: Value) -> Result<bool, Error> {
        Ok(match self.resolved(schema, ns)? {
            Schema::Null => value.is_nil(),
            Schema::Boolean => boolean(value),
            Schema::Int => {
                integer(value)
                    && i64::try_convert(value).is_ok_and(|value| i32::try_from(value).is_ok())
            }
            Schema::Long => integer(value) && i64::try_convert(value).is_ok(),
            Schema::Float | Schema::Double => {
                float(value) || integer(value) || value.is_kind_of(self.decimal)
            }
            Schema::String | Schema::Bytes => RString::from_value(value).is_some(),
            Schema::Fixed(fixed) => {
                RString::from_value(value).is_some_and(|value| value.len() == fixed.size)
            }
            Schema::Array(_) => RArray::from_value(value).is_some(),
            Schema::Map(_) | Schema::Record(_) => RHash::from_value(value).is_some(),
            _ => true,
        })
    }

    fn adapt(&mut self, mapping: Mapping, value: Value) -> Result<Value, Error> {
        if mapping.identity() {
            return Ok(value);
        }
        if self.trusted(value) && self.stock_adapter(mapping, value)? {
            return self.adapter_value(mapping, value);
        }
        self.callback(|this| mapping.convert(this.encode, value))
    }

    fn union_candidate(
        &mut self,
        variants: &[Schema],
        ns: NamespaceRef<'_>,
        value: Value,
        mapping: Mapping,
    ) -> Result<Option<(usize, Value)>, Error> {
        for (index, schema) in variants.iter().enumerate() {
            self.charge_work()?;
            if matches!(self.resolved(schema, ns)?, Schema::Null) {
                if value.is_nil() {
                    return Ok(Some((index, value)));
                }
                continue;
            }
            let child = mapping.child(index)?;
            let datum = if child.identity() {
                value
            } else if self.stock_adapter(child, value)? {
                match self.adapter_value(child, value) {
                    Ok(datum) => datum,
                    Err(error) if error.is_kind_of(self.ruby.exception_standard_error()) => {
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            } else {
                return Err(self.abort());
            };
            if self.admits(schema, ns, datum)? {
                return Ok(Some((index, datum)));
            }
        }
        Ok(None)
    }

    fn union_attempt(
        &mut self,
        variants: &[Schema],
        ns: NamespaceRef<'_>,
        value: Value,
        depth: usize,
        mapping: Mapping,
    ) -> Result<bool, Error> {
        let Some((index, datum)) = self.union_candidate(variants, ns, value, mapping)? else {
            return Ok(false);
        };
        self.emit_long(index as i64);
        let child = Some(mapping.child(index)?);
        self.adapted_value(&variants[index], ns, datum, depth, child)?;
        Ok(true)
    }

    fn validated_union_index(
        &mut self,
        value: Value,
        mapping: Mapping,
        depth: usize,
    ) -> Result<usize, Error> {
        let budget = crate::allocate(|| self.ruby.ary_new_capa(2))?;
        budget.push(self.limits.max_items.saturating_sub(self.work))?;
        budget.push(self.limits.max_depth.saturating_sub(depth) + 1)?;
        let index = self.callback(|_| mapping.union_index(value, budget));
        let remaining: i64 = budget.entry(0)?;
        self.work = self
            .limits
            .max_items
            .saturating_add_signed(-(remaining as isize));
        index
    }

    fn mapped_union(
        &mut self,
        variants: &[Schema],
        ns: NamespaceRef<'_>,
        value: Value,
        depth: usize,
        mapping: Mapping,
    ) -> Result<(), Error> {
        let outer = self.speculating;
        if self.trusted(value) {
            let (items, bytes, path_len, out_len) =
                (self.items, self.bytes, self.path.len(), self.out.len());
            self.speculating = true;
            let attempt = self.union_attempt(variants, ns, value, depth, mapping);
            self.speculating = outer;
            match attempt {
                Ok(true) => return Ok(()),
                Err(error)
                    if self.work > self.limits.max_items
                        || self.overflowed()
                        || !error.is_kind_of(self.ruby.exception_standard_error()) =>
                {
                    return Err(error);
                }
                _ => {}
            }
            self.items = items;
            self.bytes = bytes;
            self.path.truncate(path_len);
            self.out.truncate(out_len);
        }
        if outer {
            return Err(self.abort());
        }
        let index = self.validated_union_index(value, mapping, depth)?;
        let schema = variants
            .get(index)
            .ok_or_else(|| self.fail("union branch index out of range"))?;
        self.emit_long(index as i64);
        self.value(schema, ns, value, depth, Some(mapping.child(index)?))
    }

    fn value(
        &mut self,
        schema: &Schema,
        ns: NamespaceRef<'_>,
        value: Value,
        depth: usize,
        mapping: Option<Mapping>,
    ) -> Result<(), Error> {
        if self.speculating && !self.trusted(value) {
            return Err(self.abort());
        }
        let value = match mapping {
            Some(mapping) => self.adapt(mapping, value)?,
            None => value,
        };
        self.adapted_value(schema, ns, value, depth, mapping)
    }

    fn adapted_value(
        &mut self,
        schema: &Schema,
        ns: NamespaceRef<'_>,
        value: Value,
        depth: usize,
        mapping: Option<Mapping>,
    ) -> Result<(), Error> {
        if let Schema::Ref { name } = schema {
            let full = name.fully_qualified_name(ns);
            let schema = self
                .names
                .get(&full)
                .copied()
                .ok_or_else(|| self.fail("unresolved named schema"))?;
            return self.adapted_value(schema, full.namespace(), value, depth, mapping);
        }
        match self.physical(schema, ns, value, depth, mapping) {
            Err(error)
                if error.is_kind_of(self.error_class) && mapping.is_some() && !self.speculating =>
            {
                Err(mapping.unwrap().encoding_error(value)?)
            }
            result => result,
        }
    }

    fn element(
        &mut self,
        schema: &Schema,
        ns: NamespaceRef<'_>,
        value: Value,
        depth: usize,
        mapping: Option<Mapping>,
        index: usize,
    ) -> Result<(), Error> {
        let n = self.path.len();
        crate::push_index(&mut self.path, index);
        self.value(schema, ns, value, depth + 1, mapping)?;
        self.path.truncate(n);
        Ok(())
    }

    fn entry(
        &mut self,
        schema: &Schema,
        ns: NamespaceRef<'_>,
        (key, value): (Value, Value),
        depth: usize,
        mapping: Option<Mapping>,
    ) -> Result<(), Error> {
        let start = self.write_text(key)?;
        let n = self.path.len();
        let key = unsafe { std::str::from_utf8_unchecked(&self.out[start..]) };
        if key
            .bytes()
            .all(|byte| (byte.is_ascii_graphic() && byte != b'"' && byte != b'\\') || byte == b' ')
        {
            self.path.push_str("[\"");
            self.path.push_str(key);
            self.path.push_str("\"]");
        } else {
            let _ = write!(self.path, "[{key:?}]");
        }
        self.value(schema, ns, value, depth + 1, mapping)?;
        self.path.truncate(n);
        Ok(())
    }

    fn block(&mut self, header: usize, declared: usize, written: usize) {
        if written != declared {
            let mut count = Vec::new();
            if written > 0 {
                write_long(written as i64, &mut count);
            }
            let mut declared_header = Vec::new();
            write_long(declared as i64, &mut declared_header);
            self.out
                .splice(header..header + declared_header.len(), count);
        }
        self.emit(&[0]);
    }

    fn physical(
        &mut self,
        schema: &Schema,
        ns: NamespaceRef<'_>,
        value: Value,
        depth: usize,
        mapping: Option<Mapping>,
    ) -> Result<(), Error> {
        if depth > self.limits.max_depth {
            return Err(self.fail("value exceeds maximum depth"));
        }
        if self.overflowed() {
            return Err(self.fail("encoded datum exceeds max_bytes"));
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
            Schema::Null if value.is_nil() => Ok(()),
            Schema::Boolean if boolean(value) => {
                self.emit(&[u8::from(bool::try_convert(value)?)]);
                Ok(())
            }
            Schema::Int => {
                let value = self.int32(value)?;
                self.emit_long(value.into());
                Ok(())
            }
            Schema::Long => {
                let value = self.integer(value)?;
                self.emit_long(value);
                Ok(())
            }
            Schema::Float => {
                let value = self.float(value)? as f32;
                self.emit(&value.to_le_bytes());
                Ok(())
            }
            Schema::Double => {
                let value = self.float(value)?;
                self.emit(&value.to_le_bytes());
                Ok(())
            }
            Schema::String => self.write_text(value).map(drop),
            Schema::Bytes => {
                let bytes = self.binary(value, "expected binary String")?;
                self.emit_bytes(unsafe { bytes.as_slice() });
                Ok(())
            }
            Schema::Fixed(fixed) => {
                let bytes = self.binary(value, "expected binary String")?;
                if bytes.len() != fixed.size {
                    return Err(self.fail(format!("fixed requires {} bytes", fixed.size)));
                }
                self.emit(unsafe { bytes.as_slice() });
                Ok(())
            }
            Schema::Enum(enumeration) => {
                let index = match mapping {
                    Some(mapping) if RString::from_value(value).is_some() && self.core() => {
                        mapping.enum_index(value)?
                    }
                    Some(mapping) => self.callback(|_| mapping.enum_index(value))?,
                    None => {
                        let symbol = RString::from_value(value)
                            .ok_or_else(|| self.fail("expected enum String"))?;
                        if symbol.enc_coderange_scan() != Coderange::SevenBit {
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
                self.emit_long(index as i64);
                Ok(())
            }
            Schema::Array(array) => {
                let values =
                    RArray::from_value(value).ok_or_else(|| self.fail("expected Array"))?;
                let plain = self.plain(value, Op::Sequence);
                let length: usize = if plain {
                    values.len()
                } else {
                    self.callback(|_| values.funcall_public("size", ()))?
                };
                if length > self.limits.max_items - self.items {
                    return Err(self.fail("array exceeds maximum item count"));
                }
                let child = mapping::child(mapping, 0)?;
                if length > 0 {
                    let header = self.out.len();
                    self.emit_long(length as i64);
                    let mut written = 0;
                    if plain {
                        while written < values.len() {
                            let item = values.entry(written as isize)?;
                            self.element(&array.items, ns, item, depth, child, written)?;
                            written += 1;
                        }
                    } else {
                        self.callback(|this| {
                            crate::callback::each(this.ruby, value, |args| {
                                this.refresh();
                                let nil = this.ruby.qnil().as_value();
                                let item = args.first().copied().unwrap_or(nil);
                                this.element(&array.items, ns, item, depth, child, written)?;
                                written += 1;
                                Ok(())
                            })
                        })?;
                    }
                    self.block(header, length, written);
                } else {
                    self.emit(&[0]);
                }
                Ok(())
            }
            Schema::Map(map) => {
                let values = RHash::from_value(value).ok_or_else(|| self.fail("expected Hash"))?;
                let plain = self.plain(value, Op::Map);
                let length: usize = if plain {
                    values.len()
                } else {
                    self.callback(|_| values.funcall_public("size", ()))?
                };
                if length > self.limits.max_items - self.items {
                    return Err(self.fail("map exceeds maximum item count"));
                }
                let child = mapping::child(mapping, 0)?;
                if length > 0 {
                    let header = self.out.len();
                    self.emit_long(length as i64);
                    let mut written = 0;
                    if plain {
                        values.foreach(|key: Value, item: Value| {
                            self.entry(&map.types, ns, (key, item), depth, child)
                                .map_err(|error| crate::materialize(self.ruby, error))?;
                            written += 1;
                            Ok(ForEach::Continue)
                        })?;
                    } else {
                        self.callback(|this| {
                            crate::callback::each(this.ruby, value, |args| {
                                this.refresh();
                                let pair;
                                let args = if args.len() == 1 {
                                    pair = RArray::try_convert(args[0])?;
                                    &[pair.entry(0)?, pair.entry(1)?]
                                } else {
                                    args
                                };
                                let nil = this.ruby.qnil().as_value();
                                let key = args.first().copied().unwrap_or(nil);
                                let item = args.get(1).copied().unwrap_or(nil);
                                this.entry(&map.types, ns, (key, item), depth, child)?;
                                written += 1;
                                Ok(())
                            })
                        })?;
                    }
                    self.block(header, length, written);
                } else {
                    self.emit(&[0]);
                }
                Ok(())
            }
            Schema::Record(record) => {
                let data =
                    RHash::from_value(value).ok_or_else(|| self.fail("expected record Hash"))?;
                let full = record.name.fully_qualified_name(ns);
                if record.fields.len() > self.limits.max_items - self.items {
                    return Err(self.fail("record exceeds maximum item count"));
                }
                let records = self.records;
                let indices = records
                    .get(&(record as *const RecordSchema as usize))
                    .ok_or_else(|| self.fail("missing prepared field"))?;
                for ((field_index, field), &index) in record.fields.iter().enumerate().zip(indices)
                {
                    self.count_bytes(field.name.len())?;
                    let n = self.path.len();
                    self.path.push('.');
                    self.path.push_str(&field.name);
                    let key = match mapping.and_then(|mapping| mapping.field_name(field_index)) {
                        Some(name) => name,
                        None => self.keys.entry((index * 2) as isize)?,
                    };
                    let symbol: Value = self.keys.entry((index * 2 + 1) as isize)?;
                    let value = self.field_value(data, key, symbol)?;
                    self.value(
                        &field.schema,
                        full.namespace(),
                        value,
                        depth + 1,
                        mapping::child(mapping, field_index)?,
                    )?;
                    self.path.truncate(n);
                }
                Ok(())
            }
            Schema::Union(union) => {
                let datum;
                let index = if value.is_kind_of(self.union) {
                    let (branch, wrapped): (Value, Value) =
                        if class_of(value) == Some(self.classes.union) {
                            (value.funcall("branch", ())?, value.funcall("value", ())?)
                        } else {
                            self.callback(|_| {
                                Ok((value.funcall("branch", ())?, value.funcall("value", ())?))
                            })?
                        };
                    datum = wrapped;
                    if integer(branch) {
                        self.ruby_result(usize::try_convert(branch))?
                    } else {
                        let branch: String = if RString::from_value(branch).is_some()
                            && self.plain(branch, Op::Text)
                        {
                            self.ruby_result(String::try_convert(branch))?
                        } else {
                            let label = self.callback(|_| branch.funcall("to_s", ()));
                            self.ruby_result(label)?
                        };
                        union
                            .variants()
                            .iter()
                            .position(|s| self.label(s, ns) == branch)
                            .ok_or_else(|| self.fail(format!("unknown union branch {branch}")))?
                    }
                } else if let Some(mapping) = mapping {
                    return self.mapped_union(union.variants(), ns, value, depth, mapping);
                } else {
                    let (items, bytes, path_len, out_len) =
                        (self.items, self.bytes, self.path.len(), self.out.len());
                    let mut failure = None;
                    for (i, schema) in union.variants().iter().enumerate() {
                        if self.accepts(schema, ns, value)? {
                            self.emit_long(i as i64);
                            match self.value(schema, ns, value, depth, None) {
                                Ok(()) => return Ok(()),
                                Err(error) => {
                                    if self.work > self.limits.max_items || self.overflowed() {
                                        return Err(error);
                                    }
                                    failure = Some(error.to_string());
                                }
                            }
                            self.items = items;
                            self.bytes = bytes;
                            self.path.truncate(path_len);
                            self.out.truncate(out_len);
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
                self.emit_long(index as i64);
                self.value(schema, ns, datum, depth, mapping::child(mapping, index)?)
            }
            Schema::Decimal(decimal) => {
                let unscaled: String = self.logical(
                    "decimal_unscaled",
                    (value, decimal.precision, decimal.scale),
                )?;
                let integer = BigInt::parse_bytes(unscaled.as_bytes(), 10)
                    .ok_or_else(|| self.fail("invalid decimal"))?;
                let bytes = integer.to_signed_bytes_be();
                if let InnerDecimalSchema::Fixed(fixed) = &decimal.inner {
                    if bytes.len() > fixed.size {
                        return Err(self.fail("decimal overflows fixed size"));
                    }
                    self.count_bytes(fixed.size)?;
                    let sign = if integer.sign() == num_bigint::Sign::Minus {
                        255
                    } else {
                        0
                    };
                    self.out
                        .extend(std::iter::repeat_n(sign, fixed.size - bytes.len()));
                    self.emit(&bytes);
                } else {
                    self.count_bytes(bytes.len())?;
                    self.emit_bytes(&bytes);
                }
                Ok(())
            }
            Schema::BigDecimal => {
                let parts: RArray = self.logical("big_decimal_parts", (value,))?;
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
                let bytes =
                    crate::big_decimal::encode(&apache_avro::BigDecimal::new(coefficient, scale))
                        .map_err(|e| self.fail(e))?;
                self.emit_bytes(&bytes);
                Ok(())
            }
            Schema::Date => {
                let days: Value = self.logical("date_days", (value,))?;
                let days = self.int32(days)?;
                self.emit_long(days.into());
                Ok(())
            }
            Schema::TimeMillis => {
                let ticks = self.int32(value)?;
                if !(0..86_400_000).contains(&ticks) {
                    return Err(self.fail("time-millis must be within a day"));
                }
                self.emit_long(ticks.into());
                Ok(())
            }
            Schema::TimeMicros => {
                let ticks = self.integer(value)?;
                if !(0..86_400_000_000).contains(&ticks) {
                    return Err(self.fail("time-micros must be within a day"));
                }
                self.emit_long(ticks);
                Ok(())
            }
            Schema::TimestampMillis | Schema::TimestampMicros | Schema::TimestampNanos => {
                let units = match schema {
                    Schema::TimestampMillis => 1000,
                    Schema::TimestampMicros => 1_000_000,
                    _ => 1_000_000_000,
                };
                let ticks: Value = self.logical("timestamp_ticks", (value, units))?;
                let ticks = self.integer(ticks)?;
                self.emit_long(ticks);
                Ok(())
            }
            Schema::LocalTimestampMillis
            | Schema::LocalTimestampMicros
            | Schema::LocalTimestampNanos => {
                let ticks = self.integer(value)?;
                self.emit_long(ticks);
                Ok(())
            }
            Schema::Uuid(inner) => {
                let mark = self.out.len();
                let start = self.write_text(value)?;
                let text = unsafe { std::str::from_utf8_unchecked(&self.out[start..]) };
                let uuid = uuid::Uuid::parse_str(text).map_err(|_| self.fail("invalid UUID"))?;
                self.out.truncate(mark);
                match inner {
                    UuidSchema::String => self.emit_bytes(uuid.to_string().as_bytes()),
                    UuidSchema::Bytes => self.emit_bytes(uuid.as_bytes()),
                    UuidSchema::Fixed(_) => self.emit(uuid.as_bytes()),
                }
                Ok(())
            }
            Schema::Duration(_) => {
                if !value.is_kind_of(self.duration) {
                    return Err(self.fail("expected Avrocadabra::Duration"));
                }
                let exact = class_of(value) == Some(self.classes.duration);
                let mut parts = [0u32; 3];
                for (i, name) in ["months", "days", "milliseconds"].iter().enumerate() {
                    let part = if exact {
                        value.funcall(*name, ())
                    } else {
                        self.callback(|_| value.funcall(*name, ()))
                    };
                    let part: Value = self.ruby_result(part)?;
                    parts[i] = u32::try_from(self.integer(part)?)
                        .map_err(|_| self.fail("duration components must be uint32"))?;
                }
                for part in parts {
                    self.emit(&part.to_le_bytes());
                }
                Ok(())
            }
            _ => Err(self.fail("value does not match schema")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortest_digits_round_halfway_ties_to_even_like_ruby_dtoa() {
        for (value, digits, exponent) in [
            (920_013_567_207_072.2, "9200135672070722", 15),
            (97_404_494_744_092.62, "9740449474409262", 14),
            (0.9524, "9524", 0),
            (2.5, "25", 1),
            (1e23, "1", 24),
            (5e-324, "5", -323),
        ] {
            assert_eq!(
                shortest_digits(value),
                Some((digits.into(), exponent)),
                "{value}"
            );
        }
    }
}
