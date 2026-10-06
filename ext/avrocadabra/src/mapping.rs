use magnus::{
    Error, RArray, RString, Value,
    prelude::*,
    rb_sys::{AsRawValue, FromRawValue},
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
        let node = RArray::from_value(entry(self.node, 2))
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

    pub fn convert(self, method: &str, value: Value) -> Result<Value, Error> {
        let adapter = entry(self.node, 1);
        if adapter.is_nil() {
            Ok(value)
        } else {
            adapter.funcall(method, (value,))
        }
    }

    pub fn default_value(self, name: RString) -> Result<Value, Error> {
        let schema: Value = self.node.entry(0)?;
        self.context.funcall("default_value", (schema, name))
    }
}

pub fn child(mapping: Option<Mapping>, index: usize) -> Result<Option<Mapping>, Error> {
    mapping.map(|mapping| mapping.child(index)).transpose()
}
