use crate::BridgeError;
use std::{
    any::Any,
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
};

pub type CallbackEntry = unsafe extern "C" fn(*mut c_void, u64) -> u64;
pub type CallbackFunction = unsafe extern "C" fn(u64, CallbackEntry, *mut c_void) -> u64;

#[derive(Clone, Copy)]
pub struct NativeCallback(CallbackFunction);

impl NativeCallback {
    /// Binds a native function that makes synchronous unary guest callbacks.
    ///
    /// # Safety
    ///
    /// The native function may call the trampoline on the current thread during this call only.
    /// It must pass the supplied context pointer unchanged, make calls sequentially, retain
    /// neither pointer, and avoid unwinding across the C boundary.
    pub unsafe fn new(function: CallbackFunction) -> Self {
        Self(function)
    }

    pub(crate) fn call(
        self,
        value: u64,
        invoke: &mut dyn FnMut(u64) -> Result<u64, BridgeError>,
    ) -> Result<u64, BridgeError> {
        let mut scope = Scope {
            invoke,
            failure: None,
        };
        // SAFETY: Construction guarantees the native function obeys the synchronous callback
        // contract. The scope and the borrowed closure stay live until the native call returns.
        let result = unsafe { (self.0)(value, trampoline, (&mut scope as *mut Scope<'_>).cast()) };
        match scope.failure {
            None => Ok(result),
            Some(Failure::Bridge(error)) => Err(error),
            Some(Failure::Panic(payload)) => resume_unwind(payload),
        }
    }
}

struct Scope<'a> {
    invoke: &'a mut dyn FnMut(u64) -> Result<u64, BridgeError>,
    failure: Option<Failure>,
}

enum Failure {
    Bridge(BridgeError),
    Panic(Box<dyn Any + Send>),
}

unsafe extern "C" fn trampoline(context: *mut c_void, value: u64) -> u64 {
    // SAFETY: The bound native function passes exactly this live scope, sequentially on the
    // calling thread. Nested bridge calls receive a different scope.
    let scope = unsafe { &mut *context.cast::<Scope<'_>>() };
    if scope.failure.is_some() {
        return 0;
    }
    match catch_unwind(AssertUnwindSafe(|| (scope.invoke)(value))) {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => {
            scope.failure = Some(Failure::Bridge(error));
            0
        }
        Err(payload) => {
            scope.failure = Some(Failure::Panic(payload));
            0
        }
    }
}
