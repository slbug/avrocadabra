mod check;
mod plan;
mod write;

pub use plan::Plans;

use magnus::{
    Error, ExceptionClass, RArray, Ruby, TryConvert, Value,
    rb_sys::{AsRawId, AsRawValue, FromRawValue, protect},
    typed_data::Obj,
    value::LazyId,
};
use rb_sys::VALUE;
use std::sync::OnceLock;
use write::{Fail, Writer};

pub struct World {
    schema: VALUE,
    native_schema: VALUE,
    validation_options: VALUE,
    avro_type_error: VALUE,
    encode_error: VALUE,
    time: VALUE,
    decimal: VALUE,
    string_io: VALUE,
    datum_writer: VALUE,
    binary_encoder: VALUE,
    io: VALUE,
    avro: VALUE,
    env: plan::Env,
}

unsafe impl Send for World {}
unsafe impl Sync for World {}

impl World {
    fn cref(&self) -> [VALUE; 3] {
        [self.datum_writer, self.io, self.avro]
    }

    fn encode_error(&self) -> ExceptionClass {
        ExceptionClass::from_value(unsafe { Value::from_raw(self.encode_error) })
            .expect("EncodeError")
    }

    fn decimal_class(&self) -> VALUE {
        self.decimal
    }
}

static WORLD: OnceLock<World> = OnceLock::new();

fn constant(ruby: &Ruby, path: &[&str]) -> Result<VALUE, Error> {
    let mut value = ruby.class_object().as_raw();
    for name in path {
        let name = ruby.intern(name).as_raw();
        value = protect(|| unsafe { rb_sys::rb_const_get(value, name) })?;
    }
    Ok(value)
}

/// Registration also pins against compaction.
fn pinned(value: VALUE) -> VALUE {
    unsafe { rb_sys::rb_gc_register_mark_object(value) };
    value
}

fn world(ruby: &Ruby) -> Result<&'static World, Error> {
    if let Some(world) = WORLD.get() {
        return Ok(world);
    }
    let world = World {
        schema: pinned(constant(ruby, &["Avro", "Schema"])?),
        native_schema: pinned(constant(ruby, &["Avrocadabra", "Schema"])?),
        validation_options: pinned(constant(
            ruby,
            &["Avro", "IO", "DatumWriter", "VALIDATION_OPTIONS"],
        )?),
        avro_type_error: pinned(constant(ruby, &["Avro", "IO", "AvroTypeError"])?),
        encode_error: pinned(constant(ruby, &["Avrocadabra", "EncodeError"])?),
        time: pinned(ruby.class_time().as_raw()),
        decimal: pinned(constant(ruby, &["BigDecimal"])?),
        string_io: pinned(constant(ruby, &["StringIO"])?),
        datum_writer: pinned(constant(ruby, &["Avro", "IO", "DatumWriter"])?),
        binary_encoder: pinned(constant(ruby, &["Avro", "IO", "BinaryEncoder"])?),
        io: pinned(constant(ruby, &["Avro", "IO"])?),
        avro: pinned(constant(ruby, &["Avro"])?),
        env: plan::Env::new(ruby, pinned)?,
    };
    Ok(WORLD.get_or_init(|| world))
}

use check::{
    CLOSED_WRITE, EXTERNAL_ENCODING, IV_PLANS, IV_TYPE_SYM, IV_WRITER, IV_WRITERS_SCHEMA, STRING,
    id, ivar,
};

static CODECS: LazyId = LazyId::new("avrocadabra_codecs");
static BUDGET: LazyId = LazyId::new("__avrocadabra_budget");
static UNION: LazyId = LazyId::new("union");
static MAX_ITEMS: LazyId = LazyId::new("MAX_ITEMS");
static MAX_DEPTH: LazyId = LazyId::new("MAX_DEPTH");

fn call0(recv: VALUE, name: &LazyId) -> Result<VALUE, Error> {
    protect(|| unsafe { rb_sys::rb_funcallv(recv, id(name), 0, std::ptr::null()) })
}

fn io_ready(_ruby: &Ruby, world: &World, io: VALUE) -> Result<Option<i32>, Error> {
    if check::class_raw(io) != world.string_io || rb_sys::TEST(call0(io, &CLOSED_WRITE)?) {
        return Ok(None);
    }
    let string = call0(io, &STRING)?;
    if rb_sys::SPECIAL_CONST_P(string)
        || unsafe { rb_sys::rb_obj_frozen_p(string) } != rb_sys::Qfalse as VALUE
    {
        return Ok(None);
    }
    io_encoding(io)
}

fn io_encoding(io: VALUE) -> Result<Option<i32>, Error> {
    let encoding = call0(io, &EXTERNAL_ENCODING)?;
    let index = unsafe { rb_sys::rb_to_encoding_index(encoding) };
    Ok((index >= 0).then_some(index))
}

fn thread_local(name: &LazyId) -> VALUE {
    unsafe { rb_sys::rb_thread_local_aref(rb_sys::rb_thread_current(), id(name)) }
}

fn set_thread_local(name: &LazyId, value: VALUE) {
    unsafe { rb_sys::rb_thread_local_aset(rb_sys::rb_thread_current(), id(name), value) };
}

/// Bumps around `super`: a `method_added` further down may encode against the new definition,
/// and a visibility change lands only inside `super`. A C frame keeps argument-free `private`
/// scoped to its caller.
pub fn hook(_rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    check::bump();
    let result = super_call(args);
    check::bump();
    result
}

pub fn mixin(rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    check::bump();
    let result = super_call(args);
    let adopted = match (&result, args.first()) {
        (Ok(_), Some(target)) => check::mixed(rb_self.as_raw(), target.as_raw()),
        _ => Ok(()),
    };
    check::bump();
    adopted.and(result)
}

pub fn set_hooks(hooks: Value) {
    check::set_hooks(hooks.as_raw());
    unsafe { rb_sys::rb_obj_freeze(hooks.as_raw()) };
}

pub fn new_plans(ruby: &Ruby) -> Result<Obj<Plans>, Error> {
    crate::wrap(ruby, Plans::default())
}

pub fn codecs() -> Value {
    unsafe { Value::from_raw(thread_local(&CODECS)) }
}

pub fn set_codecs(cache: Value) -> Value {
    set_thread_local(&CODECS, cache.as_raw());
    cache
}

/// Read per encode: callers may lower them.
fn limits(ruby: &Ruby) -> Result<(i64, i64), Error> {
    let schema = world(ruby)?.native_schema;
    let items = protect(|| unsafe { rb_sys::rb_const_get(schema, id(&MAX_ITEMS)) })?;
    let depth = protect(|| unsafe { rb_sys::rb_const_get(schema, id(&MAX_DEPTH)) })?;
    Ok((
        i64::try_convert(unsafe { Value::from_raw(items) })?,
        i64::try_convert(unsafe { Value::from_raw(depth) })?,
    ))
}

fn new_budget(ruby: &Ruby, (items, depth): (i64, i64)) -> Result<RArray, Error> {
    let budget = crate::allocate(|| ruby.ary_new_capa(2))?;
    budget.push(items)?;
    budget.push(depth + 1)?;
    Ok(budget)
}

fn super_call(args: &[Value]) -> Result<Value, Error> {
    let argv: Vec<VALUE> = args.iter().map(|value| value.as_raw()).collect();
    protect(|| unsafe { rb_sys::rb_call_super(argv.len() as _, argv.as_ptr()) })
        .map(|value| unsafe { Value::from_raw(value) })
}

fn raise_with_cause(ruby: &Ruby, exception: VALUE, cause: Error) -> Error {
    let cause = crate::materialize(ruby, cause);
    if let magnus::error::ErrorType::Exception(cause) = cause.error_type() {
        unsafe { rb_sys::rb_set_errinfo(cause.as_raw()) };
    }
    magnus::Exception::from_value(unsafe { Value::from_raw(exception) }).map_or_else(
        || Error::new(ruby.exception_type_error(), "invalid AvroTypeError"),
        Error::from,
    )
}

pub fn validate_bang(ruby: &Ruby, _rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let fresh = thread_local(&CODECS) != rb_sys::Qnil as VALUE
        && thread_local(&BUDGET) == rb_sys::Qnil as VALUE;
    if fresh {
        set_thread_local(&BUDGET, new_budget(ruby, limits(ruby)?)?.as_raw());
    }
    let result = super_call(args);
    if fresh {
        set_thread_local(&BUDGET, rb_sys::Qnil as VALUE);
    }
    match result {
        Err(error) if error.is_kind_of(world(ruby)?.encode_error()) => {
            let argv = [
                args.first().map_or(rb_sys::Qnil as VALUE, |v| v.as_raw()),
                args.get(1).map_or(rb_sys::Qnil as VALUE, |v| v.as_raw()),
            ];
            let class = world(ruby)?.avro_type_error;
            let exception =
                protect(|| unsafe { rb_sys::rb_class_new_instance(2, argv.as_ptr(), class) })?;
            Err(raise_with_cause(ruby, exception, error))
        }
        result => result,
    }
}

pub fn validate_recursive(ruby: &Ruby, _rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let Some(budget) = RArray::from_value(unsafe { Value::from_raw(thread_local(&BUDGET)) }) else {
        return super_call(args);
    };
    let schema = args
        .first()
        .map_or(rb_sys::Qnil as VALUE, |schema| schema.as_raw());
    let cost = i64::from(ivar(schema, &IV_TYPE_SYM) != unsafe { rb_sys::rb_id2sym(id(&UNION)) });
    let items: i64 = budget.entry(0)?;
    let depth: i64 = budget.entry(1)?;
    budget.store(0, items - 1)?;
    budget.store(1, depth - cost)?;
    let result = if items - 1 < 0 || depth - cost < 0 {
        let path = args.get(2).map_or_else(String::new, |path| {
            String::try_convert(*path).unwrap_or_default()
        });
        let limit = if items - 1 < 0 { "item count" } else { "depth" };
        Err(Error::new(
            world(ruby)?.encode_error(),
            format!("{path}: union search exceeds maximum {limit}"),
        ))
    } else {
        super_call(args)
    };
    let depth: i64 = budget.entry(1)?;
    budget.store(1, depth + cost)?;
    result
}

fn fallback(datum: Value, encoder: Value) -> Result<Value, Error> {
    let argv = [datum.as_raw(), encoder.as_raw()];
    protect(|| unsafe { rb_sys::rb_call_super(2, argv.as_ptr()) })
        .map(|value| unsafe { Value::from_raw(value) })
}

pub fn write(ruby: &Ruby, rb_self: Value, datum: Value, encoder: Value) -> Result<Value, Error> {
    let cache = thread_local(&CODECS);
    if cache == rb_sys::Qnil as VALUE {
        return fallback(datum, encoder);
    }
    let world = world(ruby)?;
    let class_of = |value: Value| check::class_raw(value.as_raw());
    if class_of(rb_self) != world.datum_writer || class_of(encoder) != world.binary_encoder {
        return fallback(datum, encoder);
    }
    let groups = check::stock(ruby)?;
    if groups & check::CORE == 0 {
        return fallback(datum, encoder);
    }
    let io = ivar(encoder.as_raw(), &IV_WRITER);
    let Some(encoding) = io_ready(ruby, world, io)? else {
        return fallback(datum, encoder);
    };
    let Ok(plans) = Obj::<Plans>::try_convert(unsafe { Value::from_raw(ivar(cache, &IV_PLANS)) })
    else {
        return fallback(datum, encoder);
    };
    let schema = ivar(rb_self.as_raw(), &IV_WRITERS_SCHEMA);
    let Some(plan) = plans.fetch(&world.env, schema)? else {
        return fallback(datum, encoder);
    };
    let (items, depth) = limits(ruby)?;
    let budget = new_budget(ruby, (items, depth))?;
    let previous = thread_local(&BUDGET);
    set_thread_local(&BUDGET, budget.as_raw());
    let bounds = write::Bounds {
        items: usize::try_from(items).unwrap_or(0),
        depth: usize::try_from(depth).unwrap_or(0),
        budget,
        groups,
    };
    let mut writer = Writer::new(
        ruby,
        world,
        &plan,
        (rb_self.as_raw(), encoder.as_raw(), io, encoding),
        bounds,
    );
    let result = writer.write(0, datum);
    let flushed = writer.flush();
    drop(writer);
    plans.release(plan);
    set_thread_local(&BUDGET, previous);
    match result {
        Ok(value) => flushed.map(|()| value),
        Err(Fail::Raise(error)) => Err(error),
        Err(Fail::Limit(message)) => {
            let class = world.avro_type_error;
            let argv = [schema, datum.as_raw()];
            let exception =
                protect(|| unsafe { rb_sys::rb_class_new_instance(2, argv.as_ptr(), class) })?;
            let error = Error::new(world.encode_error(), message);
            Err(raise_with_cause(ruby, exception, error))
        }
        Err(Fail::Invalid | Fail::Abort) => Err(Error::new(
            world.encode_error(),
            "union selection escaped its attempt",
        )),
    }
}
