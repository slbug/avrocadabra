mod big_decimal;
mod callback;
mod convert;
mod decode_value;
mod guard;
mod mapping;
mod memory;
mod prepare;
mod resolution;
mod schema_state;
mod validation;
mod wire;

use apache_avro::{
    Schema,
    reader::datum::GenericDatumReader,
    schema::{RecordSchema, ResolvedSchema},
};
use guard::Limits;
use magnus::{
    DataTypeFunctions, Error, RArray, RModule, RString, Ruby, TryConvert, TypedData, Value,
    function, gc, method,
    prelude::*,
    rb_sys::{AsRawValue, FromRawValue},
    typed_data::Obj,
    value::{Opaque, ReprValue},
};
use memory::HeapSize;
use resolution::Resolution;
use std::{
    collections::HashMap,
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn push_index(path: &mut String, index: usize) {
    path.push('[');
    path.push_str(index.format_into(&mut std::fmt::NumBuffer::new()));
    path.push(']');
}

struct Codec<'a> {
    schema: &'a Schema,
    resolved: ResolvedSchema<'a>,
    reader: GenericDatumReader<'a>,
}

#[ouroboros::self_referencing]
struct Prepared {
    schemas: Vec<Schema>,
    wire_schemas: Vec<Schema>,
    #[borrows(schemas, wire_schemas)]
    #[covariant]
    codec: Codec<'this>,
}

struct Core {
    prepared: Prepared,
    id: u64,
    limits: Limits,
    fields: HashMap<String, usize>,
    records: HashMap<usize, Vec<usize>>,
    encoded_size: AtomicUsize,
    memory_size: usize,
    // Plans own no schemas, so reciprocal reader pairs cannot form Arc cycles.
    resolutions: [ResolutionSlot; 8],
}

#[derive(Default)]
struct ResolutionSlot {
    reader: AtomicU64,
    plan: OnceLock<Arc<Resolution>>,
}

#[derive(TypedData)]
#[magnus(class = "Avrocadabra::NativeSchema", mark, size, frozen_shareable)]
struct NativeSchema {
    core: Arc<Core>,
    keys: Opaque<RArray>,
    accounted: AtomicUsize,
}

impl DataTypeFunctions for NativeSchema {
    fn free(self: Box<Self>) {
        let bytes = self.accounted.load(Ordering::Relaxed);
        drop(self);
        // Deferred dfree runs under Ruby's GVL; native worker drops never call Ruby.
        unsafe { rb_sys::rb_gc_adjust_memory_usage(-(bytes as isize) as _) };
    }

    fn size(&self) -> usize {
        size_of::<Self>()
            + self.core.memory_size
            + self
                .core
                .resolutions
                .iter()
                .filter_map(|slot| slot.plan.get())
                .map(|plan| plan.memory_size())
                .sum::<usize>()
    }

    fn mark(&self, marker: &gc::Marker) {
        // Pin the array; Ruby traces and relocates its string/symbol entries itself.
        marker.mark(self.keys);
    }
}

fn namespace(ruby: &Ruby) -> Result<RModule, Error> {
    ruby.class_object().const_get("Avrocadabra")
}

pub(crate) fn error(ruby: &Ruby, class: &str, message: impl AsRef<str>) -> Error {
    match namespace(ruby).and_then(|m| m.const_get(class)) {
        Ok(class) => Error::new(class, message.as_ref().to_owned()),
        Err(error) => error,
    }
}

pub(crate) fn allocate<T: ReprValue + TryConvert>(make: impl FnOnce() -> T) -> Result<T, Error> {
    let raw = magnus::rb_sys::protect(|| make().as_raw())?;
    T::try_convert(unsafe { Value::from_raw(raw) })
}

pub(crate) fn wrap<T: TypedData>(ruby: &Ruby, data: T) -> Result<Obj<T>, Error> {
    let class = T::class(ruby).as_raw();
    let data_type = (T::data_type() as *const magnus::DataType).cast::<rb_sys::rb_data_type_t>();
    let object = allocate(|| unsafe {
        Value::from_raw(rb_sys::rb_data_typed_object_wrap(
            class,
            std::ptr::null_mut(),
            data_type,
        ))
    })?;
    #[allow(deprecated)]
    unsafe {
        (*(object.as_raw() as *mut rb_sys::RTypedData)).data = Box::into_raw(Box::new(data)).cast();
    }
    Obj::try_convert(object)
}

pub(crate) fn materialize(ruby: &Ruby, error: Error) -> Error {
    let magnus::error::ErrorType::Error(class, message) = error.error_type() else {
        return error;
    };
    let class = class.as_raw();
    let exception = magnus::rb_sys::protect(|| unsafe {
        let message = ruby.str_new(message).as_raw();
        rb_sys::rb_class_new_instance(1, &message, class)
    });
    match exception.map(|raw| magnus::Exception::from_value(unsafe { Value::from_raw(raw) })) {
        Ok(Some(exception)) => exception.into(),
        Ok(None) => error,
        Err(error) => error,
    }
}

fn boundary<T>(
    ruby: &Ruby,
    class: &str,
    work: impl FnOnce() -> Result<T, Error>,
) -> Result<T, Error> {
    catch_unwind(AssertUnwindSafe(work))
        .unwrap_or_else(|_| {
            Err(error(
                ruby,
                class,
                "native codec rejected input after an internal panic",
            ))
        })
        .map_err(|error| materialize(ruby, error))
}

fn collect_fields(
    schema: &Schema,
    fields: &mut HashMap<String, usize>,
    limits: Limits,
) -> Result<(), String> {
    use apache_avro::schema::{InnerDecimalSchema, UuidSchema};
    match schema {
        Schema::Record(record) => {
            for field in &record.fields {
                let next = fields.len();
                fields.entry(field.name.clone()).or_insert(next);
                collect_fields(&field.schema, fields, limits)?;
            }
        }
        Schema::Array(array) => collect_fields(&array.items, fields, limits)?,
        Schema::Map(map) => collect_fields(&map.types, fields, limits)?,
        Schema::Union(union) => {
            for schema in union.variants() {
                collect_fields(schema, fields, limits)?;
            }
        }
        Schema::Fixed(fixed) | Schema::Duration(fixed) | Schema::Uuid(UuidSchema::Fixed(fixed)) => {
            if fixed.size > limits.max_bytes {
                return Err("fixed size exceeds max_bytes".into());
            }
        }
        Schema::Decimal(decimal) => {
            if let InnerDecimalSchema::Fixed(fixed) = &decimal.inner
                && fixed.size > limits.max_bytes
            {
                return Err("decimal fixed size exceeds max_bytes".into());
            }
        }
        _ => {}
    }
    Ok(())
}

fn collect_records(
    schema: &Schema,
    fields: &HashMap<String, usize>,
    records: &mut HashMap<usize, Vec<usize>>,
) {
    match schema {
        Schema::Record(record) => {
            records
                .entry(record as *const RecordSchema as usize)
                .or_insert_with(|| {
                    record
                        .fields
                        .iter()
                        .map(|field| fields[&field.name])
                        .collect()
                });
            for field in &record.fields {
                collect_records(&field.schema, fields, records);
            }
        }
        Schema::Array(array) => collect_records(&array.items, fields, records),
        Schema::Map(map) => collect_records(&map.types, fields, records),
        Schema::Union(union) => {
            for schema in union.variants() {
                collect_records(schema, fields, records);
            }
        }
        _ => {}
    }
}

impl Core {
    fn prepare(json: String, references: Vec<String>, limits: Limits) -> Result<Self, String> {
        let schemas = prepare::schemas(&json, &references, limits)?;
        let wire_schemas = schemas
            .iter()
            .map(wire::schema)
            .collect::<Result<Vec<_>, _>>()?;
        let mut fields = HashMap::new();
        for schema in &schemas {
            collect_fields(schema, &mut fields, limits)?;
        }
        let prepared = PreparedTryBuilder {
            schemas,
            wire_schemas,
            codec_builder: move |schemas, wire_schemas| {
                let schema = schemas.last().ok_or("missing root schema")?;
                let resolved = ResolvedSchema::new_with_schemata(schemas.iter().collect())
                    .map_err(|e| e.to_string())?;
                let wire_schema = wire_schemas.last().ok_or("missing root wire schema")?;
                let wire_resolved =
                    ResolvedSchema::new_with_schemata(wire_schemas.iter().collect())
                        .map_err(|error| error.to_string())?;
                let reader = GenericDatumReader::builder(wire_schema)
                    .resolved_writer_schemata(wire_resolved)
                    .build()
                    .map_err(|e| e.to_string())?;
                Ok::<_, String>(Codec {
                    schema,
                    resolved,
                    reader,
                })
            },
        }
        .try_build()?;
        let mut records = HashMap::new();
        for schema in prepared.borrow_schemas() {
            collect_records(schema, &fields, &mut records);
        }
        let resolved = &prepared.borrow_codec().resolved;
        let memory_size = size_of::<Self>()
            + 2 * size_of::<usize>()
            + 2 * size_of::<Vec<Schema>>()
            + size_of::<Codec<'_>>()
            + prepared.borrow_schemas().heap_size()
            + prepared.borrow_wire_schemas().heap_size()
            + fields.heap_size()
            + records.heap_size()
            + 3 * (resolved.get_names().heap_size() + size_of_val(resolved.get_schemata()));
        Ok(Self {
            prepared,
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            limits,
            fields,
            records,
            encoded_size: AtomicUsize::new(0),
            memory_size,
            resolutions: std::array::from_fn(|_| ResolutionSlot::default()),
        })
    }

    fn resolution(&self, reader: &Core) -> Result<Arc<Resolution>, String> {
        for slot in &self.resolutions {
            if slot.reader.load(Ordering::Relaxed) == reader.id
                && let Some(plan) = slot.plan.get()
            {
                return Ok(Arc::clone(plan));
            }
        }
        let writer = self.prepared.borrow_codec();
        let target = reader.prepared.borrow_codec();
        let limits = self.limits.intersect(reader.limits);
        let plan = Arc::new(Resolution::new(
            writer.schema,
            writer.resolved.get_names(),
            target.schema,
            target.resolved.get_names(),
            limits.max_depth,
            limits.max_items,
            limits.max_bytes,
        )?);
        for slot in &self.resolutions {
            match slot
                .reader
                .compare_exchange(0, reader.id, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => {
                    // A forked child may have lost the thread initializing this slot.
                    let _ = slot.plan.set(Arc::clone(&plan));
                    break;
                }
                Err(id) if id == reader.id => {
                    return Ok(slot.plan.get().cloned().unwrap_or(plan));
                }
                Err(_) => {}
            }
        }
        Ok(plan)
    }
}

impl NativeSchema {
    fn account_memory(&self, ruby: &Ruby) {
        let bytes = self.size();
        let previous = self.accounted.fetch_max(bytes, Ordering::Relaxed);
        if bytes > previous {
            let _ = magnus::rb_sys::protect(|| {
                ruby.gc_adjust_memory_usage((bytes - previous) as isize);
                ruby.qnil().as_raw()
            });
        }
    }

    fn new(
        ruby: &Ruby,
        json: RString,
        references: RArray,
        depth: usize,
        bytes: usize,
        items: usize,
    ) -> Result<Obj<Self>, Error> {
        boundary(ruby, "SchemaError", || {
            if !(1..=128).contains(&depth)
                || !(1..=67_108_864).contains(&bytes)
                || !(1..=1_000_000).contains(&items)
            {
                return Err(error(
                    ruby,
                    "SchemaError",
                    "limits require max_depth in 1..128, max_bytes in 1..67108864, max_items in 1..1000000",
                ));
            }
            let limits = Limits {
                max_depth: depth,
                max_bytes: bytes,
                max_items: items,
            };
            if references.len() > 65_536 {
                return Err(error(
                    ruby,
                    "SchemaError",
                    "schema reference count exceeds node limit",
                ));
            }
            let mut total = 0;
            let json = schema_text(ruby, json, &mut total)?;
            let mut owned_references = Vec::with_capacity(references.len());
            for index in 0..references.len() {
                owned_references.push(schema_text(
                    ruby,
                    references.entry(index as isize)?,
                    &mut total,
                )?);
            }
            let core = Core::prepare(json, owned_references, limits)
                .map_err(|e| error(ruby, "SchemaError", e))?;
            let keys = allocate(|| ruby.ary_new_capa(core.fields.len() * 2))?;
            for (name, &index) in &core.fields {
                let key = allocate(|| {
                    let key = ruby.str_new(name);
                    key.freeze();
                    key
                })?;
                keys.store((index * 2) as isize, key)?;
                let symbol: Value = key.funcall("to_sym", ())?;
                keys.store((index * 2 + 1) as isize, symbol)?;
            }
            keys.freeze();
            let ractor: magnus::RClass = ruby.class_object().const_get("Ractor")?;
            let keys: RArray = ractor.funcall("make_shareable", (keys,))?;
            let schema = wrap(
                ruby,
                Self {
                    core: Arc::new(core),
                    keys: keys.into(),
                    accounted: AtomicUsize::new(0),
                },
            )?;
            schema.account_memory(ruby);
            Ok(schema)
        })
    }

    fn encode(
        ruby: &Ruby,
        this: &Self,
        value: Value,
        _release_gvl: bool,
        graph: Option<RArray>,
    ) -> Result<RString, Error> {
        boundary(ruby, "EncodeError", || {
            let mapping = graph.map(mapping::Mapping::new).transpose()?;
            let bytes =
                convert::encode(ruby, &this.core, value, ruby.get_inner(this.keys), mapping)?;
            allocate(|| ruby.str_from_slice(&bytes))
        })
    }

    fn decode(
        ruby: &Ruby,
        this: &Self,
        input: Value,
        reader: Option<&Self>,
        release_gvl: bool,
        tagged_unions: bool,
        graph: Option<RArray>,
    ) -> Result<Value, Error> {
        boundary(ruby, "DecodeError", || {
            let target = reader.unwrap_or(this);
            let limits = this.core.limits.intersect(target.core.limits);
            let (bytes, offset, stream) = match RString::from_value(input) {
                Some(bytes) => (bytes, 0, None),
                None => {
                    let class: magnus::RClass = ruby.class_object().const_get("StringIO")?;
                    if !input.is_kind_of(class) {
                        return Err(error(
                            ruby,
                            "DecodeError",
                            "input must be a String or StringIO",
                        ));
                    }
                    let _: Value = input.funcall("read", (0,))?;
                    (
                        input.funcall("string", ())?,
                        input.funcall("pos", ())?,
                        Some(input),
                    )
                }
            };
            let codec = this.core.prepared.borrow_codec();
            let slice = unsafe { bytes.as_slice() }
                .get(offset..)
                .unwrap_or_default();
            let frame = guard::inspect(codec.schema, codec.resolved.get_names(), slice, limits)
                .map_err(|e| error(ruby, "DecodeError", e))?;
            let consumed = frame.consumed;
            // No Ruby-owned memory crosses the GVL boundary.
            let bytes = slice[..consumed].to_vec();
            let core = Arc::clone(&this.core);
            let plan = reader
                .filter(|reader| reader.core.id != core.id)
                .map(|reader| core.resolution(&reader.core))
                .transpose()
                .map_err(|e| error(ruby, "ResolutionError", e))?;
            if plan.is_some() {
                this.account_memory(ruby);
            }
            let value = pure(ruby, release_gvl, "DecodeError", move || {
                let codec = core.prepared.borrow_codec();
                let wire::Datum(value) = codec
                    .reader
                    .read_deser(&mut bytes.as_slice())
                    .map_err(|e| e.to_string())?;
                let value = wire::materialize(
                    value,
                    codec.schema,
                    codec.resolved.get_names(),
                    &mut frame.unions.into_iter(),
                )?;
                Ok(match plan {
                    Some(plan) => plan.apply(value),
                    None => Ok(resolution::Resolved {
                        value,
                        defaults: Vec::new(),
                        adapters: Vec::new(),
                        failure: None,
                    }),
                })
            })?
            .map_err(|e| error(ruby, "ResolutionError", e))?;
            let codec = target.core.prepared.borrow_codec();
            let value = decode_value::decode(
                ruby,
                codec.schema,
                codec.resolved.get_names(),
                value.value,
                ruby.get_inner(target.keys),
                &target.core.fields,
                decode_value::Options {
                    limits,
                    tagged_unions,
                    mapping: graph.map(mapping::Mapping::new).transpose()?,
                    defaults: value.defaults,
                    adapters: value.adapters,
                    failure: value.failure,
                },
            )?;
            if let Some(stream) = stream {
                let _: Value = stream.funcall("pos=", (offset + consumed,))?;
            }
            Ok(value)
        })
    }
}

fn schema_text(ruby: &Ruby, value: RString, total: &mut usize) -> Result<String, Error> {
    *total = total
        .checked_add(value.len())
        .filter(|&n| n <= 1024 * 1024)
        .ok_or_else(|| error(ruby, "SchemaError", "schemas exceed 1 MiB"))?;
    unsafe { value.as_str() }
        .map(str::to_owned)
        .map_err(|e| error(ruby, "SchemaError", e.to_string()))
}

fn pure<T: Send, F: FnOnce() -> Result<T, String> + Send>(
    ruby: &Ruby,
    release: bool,
    class: &str,
    work: F,
) -> Result<T, Error> {
    let work = || {
        catch_unwind(AssertUnwindSafe(work))
            .unwrap_or_else(|_| Err("native codec rejected input after an internal panic".into()))
    };
    if !release {
        return work().map_err(|e| error(ruby, class, e));
    }
    struct Job<F, T> {
        work: Option<F>,
        result: Option<Result<T, String>>,
    }
    unsafe extern "C" fn run<F: FnOnce() -> Result<T, String>, T>(
        data: *mut c_void,
    ) -> *mut c_void {
        // `protect` below keeps this stack frame alive through Ruby interrupts.
        let job = unsafe { &mut *data.cast::<Job<F, T>>() };
        if let Some(work) = job.work.take() {
            job.result = Some(work());
        }
        std::ptr::null_mut()
    }
    fn invoke<F: FnOnce() -> Result<T, String>, T>(job: &mut Job<F, T>) {
        unsafe {
            rb_sys::rb_thread_call_without_gvl(
                Some(run::<F, T>),
                (job as *mut Job<F, T>).cast(),
                None,
                std::ptr::null_mut(),
            );
        }
    }
    let mut job = Job {
        work: Some(work),
        result: None,
    };
    magnus::rb_sys::protect(|| {
        invoke(&mut job);
        rb_sys::Qnil as rb_sys::VALUE
    })?;
    job.result
        .ok_or_else(|| error(ruby, class, "native work was interrupted"))?
        .map_err(|e| error(ruby, class, e))
}

#[magnus::init]
fn init(ruby: &Ruby) -> Result<(), Error> {
    validation::initialize()
        .map_err(|message| Error::new(ruby.exception_runtime_error(), message))?;
    unsafe { rb_sys::rb_ext_ractor_safe(true) };
    let module = ruby.define_module("Avrocadabra")?;
    let class = module.define_class("NativeSchema", ruby.class_object())?;
    module.define_class("NativeCallback", ruby.class_object())?;
    callback::Callback::class(ruby);
    let _: Value = module.funcall("private_constant", ("NativeCallback",))?;
    // Magnus's lazy class lookup can deadlock with Ruby GC across Ractor GVLs.
    NativeSchema::class(ruby);
    class.define_singleton_method("new", function!(NativeSchema::new, 5))?;
    class.define_singleton_method("unchanged?", function!(schema_state::unchanged, 1))?;
    class.define_method("encode", method!(NativeSchema::encode, 3))?;
    class.define_method("decode", method!(NativeSchema::decode, 5))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use apache_avro::types::Value as AvroValue;

    fn core(schema: &str) -> Core {
        Core::prepare(
            schema.into(),
            Vec::new(),
            Limits {
                max_depth: 64,
                max_bytes: 1024,
                max_items: 1024,
            },
        )
        .unwrap()
    }

    #[test]
    fn resolution_skips_incomplete_slots_inherited_at_fork() {
        let writer = core(r#""int""#);
        let reader = core(r#""long""#);
        for slot in &writer.resolutions {
            slot.reader.store(reader.id, Ordering::Relaxed);
        }
        let plan = writer.resolution(&reader).unwrap();
        assert_eq!(
            plan.apply(AvroValue::Int(7)).unwrap().value,
            AvroValue::Long(7)
        );
        assert!(
            writer
                .resolutions
                .iter()
                .all(|slot| slot.plan.get().is_none())
        );
    }

    #[test]
    fn concurrent_resolution_reserves_one_slot_per_reader() {
        let writer = core(r#""int""#);
        let reader = core(r#""long""#);
        std::thread::scope(|scope| {
            for _ in 0..16 {
                scope.spawn(|| {
                    let plan = writer.resolution(&reader).unwrap();
                    assert_eq!(
                        plan.apply(AvroValue::Int(7)).unwrap().value,
                        AvroValue::Long(7)
                    );
                });
            }
        });
        assert_eq!(
            writer
                .resolutions
                .iter()
                .filter(|slot| slot.plan.get().is_some())
                .count(),
            1
        );
        assert!(Arc::ptr_eq(
            &writer.resolution(&reader).unwrap(),
            &writer.resolution(&reader).unwrap()
        ));
    }

    #[test]
    fn resolution_cache_is_bounded_without_replacing_published_plans() {
        let writer = core(r#""int""#);
        let first_reader = core(r#""long""#);
        let first_plan = writer.resolution(&first_reader).unwrap();
        for _ in 0..16 {
            let reader = core(r#""long""#);
            let plan = writer.resolution(&reader).unwrap();
            assert_eq!(
                plan.apply(AvroValue::Int(7)).unwrap().value,
                AvroValue::Long(7)
            );
        }
        assert_eq!(
            writer
                .resolutions
                .iter()
                .filter(|slot| slot.plan.get().is_some())
                .count(),
            8
        );
        assert!(Arc::ptr_eq(
            &first_plan,
            &writer.resolution(&first_reader).unwrap()
        ));
    }
}
