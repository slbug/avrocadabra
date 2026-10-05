use magnus::{
    DataTypeFunctions, Error, Ruby, TypedData, Value,
    block::Proc,
    fiber::Fiber,
    gc,
    prelude::*,
    rb_sys::{AsRawValue, FromRawValue, protect, resume_error},
    typed_data::Obj,
    value::Opaque,
};
use std::sync::Mutex;

#[derive(TypedData)]
#[magnus(class = "Avrocadabra::NativeCallback", free_immediately, mark)]
pub struct Callback {
    address: Mutex<Option<usize>>,
    invoke: unsafe fn(usize, &[Value]) -> Result<(), Error>,
    owner: std::thread::ThreadId,
    fiber: Opaque<Fiber>,
}

impl DataTypeFunctions for Callback {
    fn mark(&self, marker: &gc::Marker) {
        marker.mark(self.fiber);
    }
}

struct Scope(Obj<Callback>);

impl Drop for Scope {
    fn drop(&mut self) {
        *self
            .0
            .address
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
    }
}

unsafe extern "C" fn call(
    _: rb_sys::VALUE,
    data: rb_sys::VALUE,
    argc: std::ffi::c_int,
    argv: *const rb_sys::VALUE,
    _: rb_sys::VALUE,
) -> rb_sys::VALUE {
    let ruby = unsafe { Ruby::get_unchecked() };
    let result = crate::boundary(&ruby, "EncodeError", || {
        let callback = Obj::<Callback>::try_convert(unsafe { Value::from_raw(data) })?;
        if std::thread::current().id() != callback.owner
            || ruby.fiber_current().as_raw() != ruby.get_inner(callback.fiber).as_raw()
        {
            return Err(Error::new(
                ruby.exception_thread_error(),
                "encoding block called from another thread or fiber",
            ));
        }
        let address = callback
            .address
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
            .ok_or_else(|| {
                Error::new(ruby.exception_local_jump_error(), "inactive encoding block")
            })?;
        let args = if argc == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(argv.cast(), argc as usize) }
        };
        // Escaped and reentrant blocks cannot access the borrowed encoder.
        let result = unsafe { (callback.invoke)(address, args) };
        *callback
            .address
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(address);
        result
    });
    match result {
        Ok(()) => ruby.qnil().as_raw(),
        Err(error) => unsafe { resume_error(error) },
    }
}

pub fn each<F>(ruby: &Ruby, value: Value, mut callback: F) -> Result<Value, Error>
where
    F: FnMut(&[Value]) -> Result<(), Error>,
{
    unsafe fn invoke<F>(address: usize, args: &[Value]) -> Result<(), Error>
    where
        F: FnMut(&[Value]) -> Result<(), Error>,
    {
        unsafe { (&mut *(address as *mut F))(args) }
    }

    let fiber = crate::allocate(|| ruby.fiber_current())?;
    let scope = Scope(crate::wrap(
        ruby,
        Callback {
            address: Mutex::new(Some((&raw mut callback) as usize)),
            invoke: invoke::<F>,
            owner: std::thread::current().id(),
            fiber: fiber.into(),
        },
    )?);
    let block = protect(|| unsafe { rb_sys::rb_proc_new(Some(call), scope.0.as_raw()) })?;
    let block = Proc::try_convert(unsafe { Value::from_raw(block) })?;
    value.funcall_with_block("each", (), block)
}
