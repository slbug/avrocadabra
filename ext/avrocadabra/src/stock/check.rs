use magnus::{
    Error, RArray, Ruby, Value,
    prelude::*,
    rb_sys::{AsRawId, AsRawValue, FromRawValue, protect},
    value::LazyId,
};
use rb_sys::VALUE;
use std::sync::{
    OnceLock,
    atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
};

static HOOKS: OnceLock<VALUE> = OnceLock::new();
static TAMPERED: AtomicBool = AtomicBool::new(false);
static EPOCH: AtomicU64 = AtomicU64::new(1);
static VERIFIED_EPOCH: AtomicU64 = AtomicU64::new(0);
static VERIFIED_CVAR: AtomicU64 = AtomicU64::new(u64::MAX);
static VERIFIED_CONSTANTS: AtomicU64 = AtomicU64::new(u64::MAX);
static VERIFIED_GROUPS: AtomicU8 = AtomicU8::new(0);

pub fn bump() {
    EPOCH.fetch_add(1, Ordering::SeqCst);
}

macro_rules! ids {
    ($($name:ident = $text:literal),* $(,)?) => {
        $(pub static $name: LazyId = LazyId::new($text);)*
    };
}

ids! {
    IS_A = "is_a?", KIND_OF = "kind_of?", INSTANCE_OF = "instance_of?", NIL = "nil?", CLASS = "class",
    INSPECT = "inspect", RESPOND_TO = "respond_to?", RESPOND_TO_MISSING = "respond_to_missing?",
    EQ = "==", NEQ = "!=", EQUAL = "equal?", NOT = "!", EQL = "eql?", HASH = "hash", TO_S = "to_s",
    NAME = "name", CMP = "<=>", LSHIFT = "<<", RSHIFT = ">>", XOR = "^", AND = "&", OR = "|", INVERT = "~",
    CHR = "chr", GT = ">", LT = "<", TO_I = "to_i", TO_F = "to_f", TIMES = "*", PLUS = "+", DIVIDE = "/",
    MINUS = "-", ENCODE = "encode", BYTESIZE = "bytesize", LENGTH = "length", SIZE = "size",
    TO_SYM = "to_sym", FREEZE = "freeze", UPLUS = "+@", SQUEEZE = "squeeze!", KEY = "key?", AREF = "[]",
    FETCH = "fetch", DEFAULT = "default", DEFAULT_PROC = "default_proc", EACH = "each",
    EACH_PAIR = "each_pair", EACH_WITH_INDEX = "each_with_index", KEYS = "keys", ANY = "any?",
    FIRST = "first", COVER = "cover?", INCLUDE = "include?", TRIPLE = "===", FIND_INDEX = "find_index",
    INDEX = "index", FIND = "find", DETECT = "detect", MAP = "map", JOIN = "join", PACK = "pack",
    UNSHIFT = "unshift", MERGE = "merge", RAISE = "raise", FAIL = "fail", LOOP = "loop", TAP = "tap",
    NEW = "new", EXCEPTION = "exception", MESSAGE = "message", INSTANCE_METHOD = "instance_method",
    SUPER_METHOD = "super_method", OWNER = "owner", ANCESTORS = "ancestors", STAT = "stat",
    TO_STR = "to_str", LIMIT = "limit", SPLIT = "split", WRITE = "write", CLOSED_WRITE = "closed_write?",
    STRING = "string", EXTERNAL_ENCODING = "external_encoding",
    IV_FACTOR = "@factor", IV_FIELDS = "@fields", IV_ITEMS = "@items", IV_LOGICAL_TYPE = "@logical_type",
    IV_NAME = "@name", IV_PLANS = "@plans", IV_PRECISION = "@precision", IV_SCALE = "@scale",
    IV_SCHEMAS = "@schemas", IV_SIZE = "@size", IV_SYMBOLS = "@symbols", IV_TYPE = "@type",
    IV_TYPE_ADAPTER = "@type_adapter", IV_TYPE_SYM = "@type_sym", IV_VALUES = "@values", IV_WRITER = "@writer",
    IV_WRITERS_SCHEMA = "@writers_schema",
}

pub fn ivar(object: VALUE, name: &LazyId) -> VALUE {
    unsafe { rb_sys::rb_ivar_get(object, id(name)) }
}

pub fn id(name: &LazyId) -> rb_sys::ID {
    let ruby = unsafe { Ruby::get_unchecked() };
    LazyId::get_inner_with(name, &ruby).as_raw()
}

pub fn class_of(value: Value) -> VALUE {
    class_raw(value.as_raw())
}

pub fn class_raw(value: VALUE) -> VALUE {
    if rb_sys::SPECIAL_CONST_P(value) {
        unsafe { Value::from_raw(value) }.class().as_raw()
    } else {
        unsafe { (*(value as *const rb_sys::RBasic)).klass }
    }
}

pub fn basic(class: VALUE, names: &[&LazyId]) -> bool {
    names
        .iter()
        .all(|name| unsafe { rb_sys::rb_method_basic_definition_p(class, id(name)) != 0 })
}

pub fn public(class: VALUE, names: &[&LazyId]) -> bool {
    names.iter().all(|name| unsafe {
        rb_sys::rb_method_basic_definition_p(class, id(name)) != 0
            && rb_sys::rb_method_boundp(class, id(name), 3) != 0
    })
}

pub fn absent(class: VALUE, name: &LazyId) -> bool {
    unsafe { rb_sys::rb_method_boundp(class, id(name), 0) == 0 }
}

fn constant(ruby: &Ruby, scope: VALUE, path: &[&str]) -> Result<VALUE, Error> {
    let mut value = scope;
    for name in path {
        let name = ruby.intern(name).as_raw();
        value = protect(|| unsafe { rb_sys::rb_const_get(value, name) })?;
    }
    Ok(value)
}

fn integer(value: VALUE) -> u64 {
    let value = unsafe { Value::from_raw(value) };
    u64::try_convert(value).unwrap_or(u64::MAX)
}

fn stat(ruby: &Ruby, key: &str) -> Result<u64, Error> {
    let vm = constant(ruby, ruby.class_object().as_raw(), &["RubyVM"])?;
    if !public(class_raw(vm), &[&STAT]) {
        return Ok(u64::MAX);
    }
    let key = ruby.to_symbol(key).as_raw();
    let value = protect(|| unsafe { rb_sys::rb_funcallv(vm, id(&STAT), 1, &key) })?;
    Ok(integer(value))
}

fn boot(ruby: &Ruby) -> Result<bool, Error> {
    let object = ruby.class_object().as_raw();
    let set = constant(ruby, object, &["Set"])?;
    let vm = constant(ruby, object, &["RubyVM"])?;
    let validator = class_raw(constant(ruby, object, &["Avro", "SchemaValidator"])?);
    let writer = constant(ruby, object, &["Avro", "IO", "DatumWriter"])?;
    // Implicit-self calls resolve on Ruby Avro's own receivers, not Object.
    let private: [(VALUE, &[&LazyId]); 3] = [
        (object, &[&RAISE, &FAIL, &LOOP, &TAP, &RESPOND_TO_MISSING]),
        (writer, &[&RAISE]),
        (validator, &[&FAIL]),
    ];
    if !private.iter().all(|(class, names)| basic(*class, names))
        || !absent(ruby.class_symbol().as_raw(), &TO_STR)
    {
        return Ok(false);
    }
    let checks: [(VALUE, &[&LazyId]); 17] = [
        (ruby.class_range().as_raw(), &[&COVER]),
        (set, &[&INCLUDE]),
        (
            ruby.class_symbol().as_raw(),
            &[&EQ, &TRIPLE, &TO_S, &EQL, &HASH, &INSPECT],
        ),
        (
            ruby.class_string().as_raw(),
            &[
                &TO_SYM, &UPLUS, &SQUEEZE, &FREEZE, &EQ, &EQL, &HASH, &TO_S, &LENGTH, &BYTESIZE,
                &INSPECT,
            ],
        ),
        (
            ruby.class_array().as_raw(),
            &[
                &EACH,
                &FIND_INDEX,
                &AREF,
                &INDEX,
                &INCLUDE,
                &SIZE,
                &FIND,
                &DETECT,
                &LSHIFT,
                &ANY,
                &EACH_WITH_INDEX,
                &MAP,
                &JOIN,
                &FIRST,
                &PACK,
                &UNSHIFT,
                &LENGTH,
                &INSPECT,
                &TAP,
            ],
        ),
        (
            ruby.class_hash().as_raw(),
            &[&FETCH, &AREF, &KEY, &KEYS, &EACH, &ANY, &MERGE, &INSPECT],
        ),
        (
            ruby.class_integer().as_raw(),
            &[
                &EQ, &NEQ, &GT, &LT, &CMP, &TO_S, &CHR, &LSHIFT, &RSHIFT, &XOR, &AND, &OR, &INVERT,
                &TIMES, &PLUS, &MINUS, &DIVIDE, &TO_I, &INSPECT,
            ],
        ),
        (
            object,
            &[
                &IS_A,
                &KIND_OF,
                &INSTANCE_OF,
                &NIL,
                &CLASS,
                &INSPECT,
                &RESPOND_TO,
                &EQ,
                &NEQ,
                &EQUAL,
                &NOT,
                &EQL,
                &HASH,
                &TO_S,
            ],
        ),
        (
            ruby.class_class().as_raw(),
            &[
                &NEW,
                &TRIPLE,
                &TO_S,
                &NAME,
                &INSPECT,
                &HASH,
                &EQ,
                &EQL,
                &INSTANCE_METHOD,
                &ANCESTORS,
            ],
        ),
        (
            ruby.class_module().as_raw(),
            &[
                &TRIPLE,
                &TO_S,
                &NAME,
                &INSPECT,
                &HASH,
                &EQ,
                &EQL,
                &INSTANCE_METHOD,
                &ANCESTORS,
            ],
        ),
        (
            ruby.exception_standard_error().as_raw(),
            &[&EXCEPTION, &MESSAGE, &TO_S],
        ),
        (
            ruby.class_true_class().as_raw(),
            &[&EQ, &NOT, &INSPECT, &TO_S],
        ),
        (
            ruby.class_false_class().as_raw(),
            &[&EQ, &NOT, &INSPECT, &TO_S],
        ),
        (
            ruby.class_nil_class().as_raw(),
            &[&NIL, &EQ, &INSPECT, &TO_S],
        ),
        (ruby.class_float().as_raw(), &[&TO_F, &TO_I, &INSPECT]),
        (
            constant(ruby, object, &["UnboundMethod"])?,
            &[&EQ, &SUPER_METHOD, &OWNER],
        ),
        (class_raw(vm), &[&STAT]),
    ];
    Ok(checks.iter().all(|(class, names)| public(*class, names)))
}

fn unbound_eq(left: VALUE, right: VALUE) -> Result<bool, Error> {
    let value = protect(|| unsafe { rb_sys::rb_funcallv(left, id(&EQ), 1, &right) })?;
    Ok(rb_sys::TEST(value))
}

fn fresh(receiver: VALUE, name: VALUE) -> Option<VALUE> {
    if !public(class_raw(receiver), &[&INSTANCE_METHOD]) {
        return None;
    }
    protect(|| unsafe { rb_sys::rb_funcallv(receiver, id(&INSTANCE_METHOD), 1, &name) }).ok()
}

const TABLES: [&str; 4] = ["METHODS", "SUPERS", "CONSTANTS", "HOOKED"];
static PINNED: OnceLock<[VALUE; 4]> = OnceLock::new();

/// Pinned on first use: reassigning a `Stock` constant later cannot widen what native code trusts.
fn entries(ruby: &Ruby, name: &str) -> Result<RArray, Error> {
    let tables = match PINNED.get() {
        Some(tables) => *tables,
        None => {
            let mut tables = [0; 4];
            for (slot, table) in tables.iter_mut().zip(TABLES) {
                let path = ["Avrocadabra", "AvroTurf", "Stock", table];
                *slot = constant(ruby, ruby.class_object().as_raw(), &path)?;
                unsafe { rb_sys::rb_gc_register_mark_object(*slot) };
            }
            *PINNED.get_or_init(|| tables)
        }
    };
    let index = TABLES.iter().position(|table| *table == name).unwrap_or(0);
    RArray::try_convert(unsafe { Value::from_raw(tables[index]) })
}

fn entry(row: RArray, index: usize) -> VALUE {
    unsafe { rb_sys::rb_ary_entry(row.as_raw(), index as _) }
}

pub const CORE: u8 = 1;
pub const DECIMAL: u8 = 2;
pub const DECIMAL_VALUE: u8 = 4;
pub const TIME: u8 = 8;
pub const LOGICAL: u8 = 16;
const GROUPS: u8 = CORE | DECIMAL | DECIMAL_VALUE | TIME | LOGICAL;

fn group(symbol: VALUE) -> u8 {
    let name = unsafe { Value::from_raw(rb_sys::rb_sym2str(symbol)) };
    match String::try_convert(name).as_deref() {
        Ok("decimal") => DECIMAL,
        Ok("decimal_value") => DECIMAL_VALUE,
        Ok("time") => TIME,
        Ok("logical") => LOGICAL,
        _ => CORE,
    }
}

fn current(
    receiver: VALUE,
    name: VALUE,
    original: VALUE,
    public_only: bool,
) -> Result<bool, Error> {
    if !rb_sys::TEST(original) {
        return Ok(false);
    }
    let Some(current) = fresh(receiver, name) else {
        return Ok(false);
    };
    let id = unsafe { rb_sys::rb_sym2id(name) };
    Ok(unbound_eq(original, current)?
        && (!public_only || unsafe { rb_sys::rb_method_boundp(receiver, id, 3) } != 0))
}

fn methods(ruby: &Ruby) -> Result<u8, Error> {
    let mut groups = GROUPS;
    for row in entries(ruby, "METHODS")? {
        let row = RArray::try_convert(row)?;
        let group = group(entry(row, 4));
        if groups & group != 0
            && !current(
                entry(row, 0),
                entry(row, 1),
                entry(row, 2),
                rb_sys::TEST(entry(row, 3)),
            )?
        {
            groups &= !group;
        }
    }
    for row in entries(ruby, "SUPERS")? {
        let row = RArray::try_convert(row)?;
        let (receiver, name, original) = (entry(row, 0), entry(row, 1), entry(row, 2));
        let Some(current) = fresh(receiver, name).filter(|_| rb_sys::TEST(original)) else {
            return Ok(0);
        };
        let parent = protect(|| unsafe {
            rb_sys::rb_funcallv(current, id(&SUPER_METHOD), 0, std::ptr::null())
        })?;
        if !rb_sys::TEST(parent) || !unbound_eq(original, parent)? {
            return Ok(0);
        }
    }
    Ok(if groups & CORE == 0 { 0 } else { groups })
}

pub fn lexical(cref: &[VALUE], name: rb_sys::ID) -> Result<VALUE, Error> {
    for &scope in cref {
        if unsafe { rb_sys::rb_const_defined_at(scope, name) } != 0 {
            return protect(|| unsafe { rb_sys::rb_const_get_at(scope, name) });
        }
    }
    protect(|| unsafe { rb_sys::rb_const_get(cref[0], name) })
}

fn constants(ruby: &Ruby) -> Result<bool, Error> {
    for row in entries(ruby, "CONSTANTS")? {
        let row = RArray::try_convert(row)?;
        let cref: Vec<VALUE> = RArray::try_convert(unsafe { Value::from_raw(entry(row, 0)) })?
            .into_iter()
            .map(|scope| scope.as_raw())
            .collect();
        let name = unsafe { rb_sys::rb_sym2id(entry(row, 1)) };
        let Ok(value) = lexical(&cref, name) else {
            return Ok(false);
        };
        let object_id = protect(|| unsafe { rb_sys::rb_obj_id(value) })?;
        if integer(object_id) != integer(entry(row, 2)) || integer(object_id) == u64::MAX {
            return Ok(false);
        }
    }
    Ok(true)
}

pub fn set_hooks(hooks: VALUE) {
    unsafe { rb_sys::rb_gc_register_mark_object(hooks) };
    let _ = HOOKS.set(hooks);
}

fn hooks() -> VALUE {
    HOOKS.get().copied().unwrap_or(rb_sys::Qnil as VALUE)
}

fn guard(module: VALUE) -> Result<(bool, bool), Error> {
    let hooks = hooks();
    let ancestors = protect(|| unsafe { rb_sys::rb_mod_ancestors(module) })?;
    for (index, entry) in RArray::try_convert(unsafe { Value::from_raw(ancestors) })?
        .into_iter()
        .enumerate()
    {
        match entry.as_raw() {
            entry if entry == hooks => return Ok((true, index == 0)),
            entry if entry == module => break,
            _ => {}
        }
    }
    Ok((false, false))
}

/// A mixed-in module's own `method_added` may skip `super`, so the hooks go ahead of it. Nothing can
/// be re-prepended ahead of an intruder, so one revokes trust for good.
pub fn mixed(module: VALUE, target: VALUE) -> Result<(), Error> {
    let ruby = unsafe { Ruby::get_unchecked() };
    let hooks = hooks();
    let scope = unsafe { rb_sys::rb_obj_is_kind_of(target, ruby.class_module().as_raw()) }
        == rb_sys::Qtrue as VALUE;
    let (guarded, first) = if scope {
        guard(target)?
    } else {
        (false, false)
    };
    if guarded && !first {
        TAMPERED.store(true, Ordering::SeqCst);
    }
    if !guarded && !guard(class_raw(target))?.0 {
        return Ok(());
    }
    let ancestors = protect(|| unsafe { rb_sys::rb_mod_ancestors(module) })?;
    for entry in RArray::try_convert(unsafe { Value::from_raw(ancestors) })? {
        let entry = entry.as_raw();
        if unsafe { rb_sys::rb_obj_frozen_p(entry) } == rb_sys::Qtrue as VALUE {
            continue;
        }
        let singleton = protect(|| unsafe { rb_sys::rb_singleton_class(entry) })?;
        if !guard(singleton)?.0 {
            protect(|| {
                unsafe { rb_sys::rb_prepend_module(singleton, hooks) };
                rb_sys::Qnil as VALUE
            })?;
        }
    }
    Ok(())
}

fn hooked(ruby: &Ruby) -> Result<bool, Error> {
    if TAMPERED.load(Ordering::SeqCst) || guard(ruby.class_module().as_raw())? != (true, true) {
        return Ok(false);
    }
    for klass in entries(ruby, "HOOKED")? {
        let singleton = protect(|| unsafe { rb_sys::rb_singleton_class(klass.as_raw()) })?;
        if guard(singleton)? != (true, true) {
            return Ok(false);
        }
    }
    Ok(true)
}

pub fn stock(ruby: &Ruby) -> Result<u8, Error> {
    let epoch = EPOCH.load(Ordering::SeqCst);
    let cvar = stat(ruby, "global_cvar_state")?;
    let constants_state = stat(ruby, "constant_cache_invalidations")?;
    if VERIFIED_EPOCH.load(Ordering::SeqCst) == epoch
        && VERIFIED_CVAR.load(Ordering::SeqCst) == cvar
        && VERIFIED_CONSTANTS.load(Ordering::SeqCst) == constants_state
        && cvar != u64::MAX
    {
        return Ok(VERIFIED_GROUPS.load(Ordering::SeqCst));
    }
    let groups = if boot(ruby)? && constants(ruby)? {
        methods(ruby)?
    } else {
        0
    };
    let trusted = groups != 0 && hooked(ruby)?;
    VERIFIED_GROUPS.store(groups, Ordering::SeqCst);
    VERIFIED_EPOCH.store(if trusted { epoch } else { 0 }, Ordering::SeqCst);
    VERIFIED_CVAR.store(cvar, Ordering::SeqCst);
    VERIFIED_CONSTANTS.store(constants_state, Ordering::SeqCst);
    Ok(groups)
}

pub const VALUE_CAP: u16 = 1;
pub const INTEGER_CAP: u16 = 2;
pub const STRING_CAP: u16 = 4;
pub const MAP_CAP: u16 = 8;
pub const LOOKUP_CAP: u16 = 16;
pub const ARRAY_CAP: u16 = 32;
pub const WALK_CAP: u16 = 64;
pub const INERT_CAP: u16 = 128;

const SLOTS: usize = 64;

#[derive(Clone, Copy, Default)]
struct Slot {
    class: VALUE,
    known: u16,
    held: u16,
    hooked: bool,
}

pub struct Caps {
    slots: [Slot; SLOTS],
    groups: u8,
    epoch: u64,
    hooked: Vec<VALUE>,
}

impl Default for Caps {
    fn default() -> Self {
        Self {
            slots: [Slot::default(); SLOTS],
            groups: 0,
            epoch: 0,
            hooked: Vec::new(),
        }
    }
}

impl Caps {
    pub fn reset(&mut self, groups: u8) {
        let epoch = EPOCH.load(Ordering::SeqCst);
        if groups != self.groups || epoch != self.epoch || self.hooked.is_empty() {
            self.slots = [Slot::default(); SLOTS];
            self.hooked = hooked_classes();
        } else {
            for slot in &mut self.slots {
                if !slot.hooked {
                    *slot = Slot::default();
                }
            }
        }
        self.groups = groups;
        self.epoch = epoch;
    }

    #[inline]
    pub fn has(&mut self, class: VALUE, cap: u16) -> bool {
        let index = ((class >> 3) ^ (class >> 11)) as usize & (SLOTS - 1);
        let slot = &self.slots[index];
        if slot.class == class && slot.known & cap != 0 {
            return slot.held & cap != 0;
        }
        self.miss(index, class, cap)
    }

    #[cold]
    fn miss(&mut self, index: usize, class: VALUE, cap: u16) -> bool {
        if self.slots[index].class != class {
            let hooked = self.hooked.contains(&class);
            self.slots[index] = Slot {
                class,
                known: 0,
                held: 0,
                hooked,
            };
        }
        let ok = compute(class, cap, self.groups);
        let slot = &mut self.slots[index];
        slot.known |= cap;
        if ok {
            slot.held |= cap;
        }
        ok
    }
}

fn hooked_classes() -> Vec<VALUE> {
    let ruby = unsafe { Ruby::get_unchecked() };
    entries(&ruby, "HOOKED").map_or_else(
        |_| Vec::new(),
        |classes| classes.into_iter().map(|class| class.as_raw()).collect(),
    )
}

fn compute(class: VALUE, cap: u16, groups: u8) -> bool {
    let ruby = unsafe { Ruby::get_unchecked() };
    match cap {
        VALUE_CAP => {
            let decimal =
                constant(&ruby, ruby.class_object().as_raw(), &["BigDecimal"]).ok() == Some(class);
            public(
                class,
                &[
                    &IS_A,
                    &KIND_OF,
                    &INSTANCE_OF,
                    &NIL,
                    &CLASS,
                    &RESPOND_TO,
                    &NEQ,
                    &EQUAL,
                    &NOT,
                ],
            ) && (if decimal {
                groups & DECIMAL_VALUE != 0
            } else {
                public(class, &[&INSPECT, &EQ])
            }) && basic(class, &[&RESPOND_TO_MISSING])
                && (unsafe { rb_sys::rb_class_inherited_p(class, ruby.class_string().as_raw()) }
                    == rb_sys::Qtrue as VALUE
                    || absent(class, &TO_STR))
        }
        INTEGER_CAP => {
            class == ruby.class_integer().as_raw()
                && public(
                    class,
                    &[
                        &CMP, &LSHIFT, &RSHIFT, &XOR, &AND, &OR, &INVERT, &NEQ, &EQ, &CHR, &TO_S,
                        &GT, &LT, &TO_I, &TIMES, &PLUS, &DIVIDE, &MINUS,
                    ],
                )
        }
        STRING_CAP => public(
            class,
            &[
                &ENCODE, &BYTESIZE, &LENGTH, &SIZE, &EQ, &EQL, &HASH, &TO_SYM, &TO_S, &FREEZE,
            ],
        ),
        MAP_CAP | ARRAY_CAP => public(class, &[&SIZE, &EACH]),
        LOOKUP_CAP => public(class, &[&KEY, &AREF, &DEFAULT, &DEFAULT_PROC]),
        WALK_CAP => {
            let hash = unsafe { rb_sys::rb_class_inherited_p(class, ruby.class_hash().as_raw()) }
                == rb_sys::Qtrue as VALUE;
            public(class, &[&EACH, &SIZE, &INSPECT, &AREF])
                && if hash {
                    public(class, &[&KEY, &KEYS, &DEFAULT, &DEFAULT_PROC, &EACH_PAIR])
                } else {
                    public(class, &[&EACH_WITH_INDEX])
                }
        }
        INERT_CAP => {
            let meta = class_raw(unsafe { rb_sys::rb_class_real(class) });
            public(meta, &[&TO_S, &INSPECT, &NAME, &HASH, &EQL, &EQ])
        }
        _ => false,
    }
}
