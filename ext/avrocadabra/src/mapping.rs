use magnus::{Error, Exception, RArray, RString, Value, prelude::*};

#[derive(Clone, Copy)]
pub struct Mapping {
    context: Value,
    node: RArray,
}

impl Mapping {
    pub fn new(graph: RArray) -> Result<Self, Error> {
        Ok(Self {
            context: graph.entry(0)?,
            node: graph.entry(1)?,
        })
    }

    pub fn child(self, index: usize) -> Result<Self, Error> {
        let children: RArray = self.node.entry(2)?;
        Ok(Self {
            context: self.context,
            node: children.entry(index as isize)?,
        })
    }

    pub fn convert(self, method: &str, value: Value) -> Result<Value, Error> {
        let adapter: Value = self.node.entry(1)?;
        if adapter.is_nil() {
            Ok(value)
        } else {
            adapter.funcall(method, (value,))
        }
    }

    pub fn identity(self) -> Result<bool, Error> {
        Ok(self.node.entry::<Value>(1)?.is_nil())
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
