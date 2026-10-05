use magnus::{
    Error, Exception, RArray, RString, Value,
    prelude::*,
    rb_sys::{AsRawValue, FromRawValue},
    value::IntoId,
};

#[derive(Clone, Copy)]
pub struct Mapping {
    context: Value,
    node: RArray,
}

fn entry(array: RArray, index: usize) -> Value {
    unsafe { Value::from_raw(rb_sys::rb_ary_entry(array.as_raw(), index as _)) }
}

impl Mapping {
    pub fn new(graph: RArray) -> Result<Self, Error> {
        Ok(Self {
            context: graph.entry(0)?,
            node: graph.entry(1)?,
        })
    }

    pub fn child(self, index: usize) -> Result<Self, Error> {
        let node = RArray::from_value(entry(self.node, 4))
            .and_then(|children| RArray::from_value(entry(children, index)))
            .ok_or_else(|| {
                Error::new(
                    magnus::Ruby::get_with(self.node).exception_index_error(),
                    "missing mapping node",
                )
            })?;
        Ok(Self {
            context: self.context,
            node,
        })
    }

    pub fn identity(self) -> bool {
        entry(self.node, 1).is_nil()
    }

    pub fn builtin(self) -> bool {
        let adapter = entry(self.node, 1);
        let class = entry(self.node, 2);
        !adapter.is_nil()
            && !class.is_nil()
            && !rb_sys::SPECIAL_CONST_P(adapter.as_raw())
            && unsafe { (*(adapter.as_raw() as *const rb_sys::RBasic)).klass } == class.as_raw()
    }

    pub fn adapter(self) -> Value {
        entry(self.node, 1)
    }

    pub fn native_decimal(self) -> Result<bool, Error> {
        self.context.funcall("native_decimal?", ())
    }

    pub fn stock_modules(self, value_class: Value) -> Result<bool, Error> {
        self.context.funcall("stock_modules?", (value_class,))
    }

    pub fn field_name(self, index: usize) -> Option<Value> {
        RArray::from_value(entry(self.node, 3)).map(|names| entry(names, index))
    }

    pub fn convert(self, method: impl IntoId, value: Value) -> Result<Value, Error> {
        let adapter = entry(self.node, 1);
        if adapter.is_nil() {
            Ok(value)
        } else {
            adapter.funcall(method, (value,))
        }
    }

    pub fn union_index(self, value: Value, budget: RArray) -> Result<usize, Error> {
        let schema: Value = self.node.entry(0)?;
        self.context.funcall("union_index", (schema, value, budget))
    }

    pub fn enum_index(self, value: Value) -> Result<Option<usize>, Error> {
        let schema: Value = self.node.entry(0)?;
        let symbols: RArray = schema.funcall_public("symbols", ())?;
        symbols.funcall_public("index", (value,))
    }

    pub fn default_value(self, name: RString) -> Result<Value, Error> {
        let schema: Value = self.node.entry(0)?;
        self.context.funcall("default_value", (schema, name))
    }

    pub fn encoding_error(self, value: Value) -> Result<Error, Error> {
        let schema: Value = self.node.entry(0)?;
        let exception: Exception = self.context.funcall("encoding_error", (schema, value))?;
        Ok(exception.into())
    }
}

pub fn child(mapping: Option<Mapping>, index: usize) -> Result<Option<Mapping>, Error> {
    mapping.map(|mapping| mapping.child(index)).transpose()
}
