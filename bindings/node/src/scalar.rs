use std::cell::RefCell;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use napi::{Env, Error, JsFunction, JsUnknown, NapiRaw, NapiValue, Result, Status, sys};
use triplelight::exec::Interrupt;
use triplelight::extension::ScalarFunction;
use triplelight::rdf::Term;

use super::rdfjs::{parse_term, term};

const QUEUE_SIZE: usize = 32;
const RETRY: Duration = Duration::from_millis(1);
const WAIT: Duration = Duration::from_millis(5);

#[derive(Clone)]
pub(super) enum QueryThread {
    JavaScript(usize),
    Worker(Option<Arc<Interrupt>>),
}

thread_local! {
    static QUERY_THREAD: RefCell<Option<QueryThread>> = const { RefCell::new(None) };
}

impl QueryThread {
    pub(super) fn javascript(env: &Env) -> Self {
        Self::JavaScript(env.raw() as usize)
    }

    pub(super) fn run<T>(self, query: impl FnOnce() -> T) -> T {
        struct Restore(Option<QueryThread>);
        impl Drop for Restore {
            fn drop(&mut self) {
                QUERY_THREAD.with(|current| current.replace(self.0.take()));
            }
        }

        let _restore = Restore(QUERY_THREAD.with(|current| current.replace(Some(self))));
        query()
    }
}

// This state travels with the registry. Only the owning JS thread uses the
// function reference and environment. The mutex guards the TSFN pointer against
// environment cleanup and finalization, including calls arriving during teardown.
struct Endpoint {
    tsfn: Mutex<Option<usize>>,
    reference: usize,
    environment: usize,
    // Cleanup and finalization run on the same JS thread. The mutex also makes
    // this data accessible while the endpoint remains Send + Sync.
    cleanup_installed: Mutex<bool>,
}

pub(super) struct Callback(Arc<Endpoint>);

impl Callback {
    pub(super) fn new(env: &Env, function: JsFunction) -> Result<Self> {
        let name = env.create_string("TripleLight scalar callback")?;
        let raw_env = env.raw();
        let mut reference = ptr::null_mut();
        status(unsafe { sys::napi_create_reference(raw_env, function.raw(), 1, &mut reference) })?;
        let endpoint = Arc::new(Endpoint {
            tsfn: Mutex::new(None),
            reference: reference as usize,
            environment: raw_env as usize,
            cleanup_installed: Mutex::new(false),
        });
        let native_context = Arc::into_raw(Arc::clone(&endpoint))
            .cast_mut()
            .cast::<c_void>();
        let mut tsfn = ptr::null_mut();
        // The native finalizer owns native_context after success. No raw JS
        // values leave this callback registration call.
        let result = status(unsafe {
            sys::napi_create_threadsafe_function(
                raw_env,
                function.raw(),
                ptr::null_mut(),
                name.raw(),
                QUEUE_SIZE,
                1,
                native_context,
                Some(finalize),
                ptr::null_mut(),
                Some(call_js),
                &mut tsfn,
            )
        });
        if let Err(error) = result {
            unsafe {
                drop(Arc::from_raw(native_context.cast::<Endpoint>()));
                sys::napi_delete_reference(raw_env, reference);
            }
            return Err(error);
        }
        *endpoint.tsfn.lock().unwrap() = Some(tsfn as usize);
        let callback = Self(endpoint);
        // Install after the TSFN, so the endpoint closes before the TSFN's
        // cleanup runs. The finalizer removes this hook on normal release.
        status(unsafe { sys::napi_add_env_cleanup_hook(raw_env, Some(cleanup), native_context) })?;
        *callback.0.cleanup_installed.lock().unwrap() = true;
        status(unsafe { sys::napi_unref_threadsafe_function(raw_env, tsfn) })?;
        Ok(callback)
    }

    fn call_sync(&self, environment: usize, args: &[Term]) -> Option<Term> {
        if environment != self.0.environment || self.0.tsfn.lock().unwrap().is_none() {
            return None;
        }
        // The TLS guard came from the live native entry point. We are on the
        // environment's owning thread, so teardown cannot race this reference.
        invoke(
            environment as sys::napi_env,
            FunctionHandle::Reference(self.0.reference as sys::napi_ref),
            args,
        )
    }

    fn call_worker(&self, args: &[Term], interrupt: &Option<Arc<Interrupt>>) -> Option<Term> {
        let (reply, result) = mpsc::channel();
        let mut request = Box::new(Request {
            args: args.to_vec(),
            interrupt: interrupt.clone(),
            reply,
        });
        loop {
            if interrupted(interrupt) {
                return None;
            }
            let mut handle = self.0.tsfn.lock().unwrap();
            let tsfn = (*handle)? as sys::napi_threadsafe_function;
            let ptr = Box::into_raw(request);
            let status = unsafe {
                sys::napi_call_threadsafe_function(
                    tsfn,
                    ptr.cast(),
                    sys::ThreadsafeFunctionCallMode::nonblocking,
                )
            };
            if status == sys::Status::napi_ok {
                break;
            }
            // Ownership was handed to Node only on successful enqueue.
            request = unsafe { Box::from_raw(ptr) };
            if status != sys::Status::napi_queue_full {
                if status == sys::Status::napi_closing {
                    // Node considers our TSFN reference released on closing.
                    *handle = None;
                }
                return None;
            }
            drop(handle);
            std::thread::sleep(RETRY);
        }
        loop {
            match result.recv_timeout(WAIT) {
                Ok(term) => return term,
                Err(RecvTimeoutError::Disconnected) => return None,
                Err(RecvTimeoutError::Timeout) => {
                    if self.0.tsfn.lock().unwrap().is_none() || interrupted(interrupt) {
                        return None;
                    }
                }
            }
        }
    }
}

fn interrupted(interrupt: &Option<Arc<Interrupt>>) -> bool {
    interrupt
        .as_ref()
        .is_some_and(|interrupt| interrupt.stop_reason().is_some())
}

impl ScalarFunction for Callback {
    fn call(&self, args: &[Term]) -> Option<Term> {
        let thread = QUERY_THREAD.with(|current| current.borrow().clone())?;
        match thread {
            QueryThread::JavaScript(env) => self.call_sync(env, args),
            QueryThread::Worker(interrupt) => self.call_worker(args, &interrupt),
        }
    }
}

impl Endpoint {
    fn release(&self, mode: sys::napi_threadsafe_function_release_mode) {
        let mut handle = self.tsfn.lock().unwrap();
        if let Some(tsfn) = handle.take() {
            unsafe {
                sys::napi_release_threadsafe_function(tsfn as sys::napi_threadsafe_function, mode)
            };
        }
    }
}

impl Drop for Callback {
    fn drop(&mut self) {
        self.0.release(sys::ThreadsafeFunctionReleaseMode::release);
    }
}

unsafe extern "C" fn cleanup(data: *mut c_void) {
    let endpoint = unsafe { &*data.cast::<Endpoint>() };
    *endpoint.cleanup_installed.lock().unwrap() = false;
    endpoint.release(sys::ThreadsafeFunctionReleaseMode::abort);
}

unsafe extern "C" fn finalize(env: sys::napi_env, data: *mut c_void, _: *mut c_void) {
    let endpoint = unsafe { Arc::from_raw(data.cast::<Endpoint>()) };
    // Node cannot free the TSFN until this callback returns.
    endpoint.tsfn.lock().unwrap().take();
    if !env.is_null() {
        unsafe {
            if *endpoint.cleanup_installed.lock().unwrap() {
                sys::napi_remove_env_cleanup_hook(env, Some(cleanup), data);
            }
            sys::napi_delete_reference(env, endpoint.reference as sys::napi_ref);
        }
    }
}

struct Request {
    args: Vec<Term>,
    interrupt: Option<Arc<Interrupt>>,
    reply: Sender<Option<Term>>,
}

unsafe extern "C" fn call_js(
    env: sys::napi_env,
    function: sys::napi_value,
    _: *mut c_void,
    data: *mut c_void,
) {
    let request = unsafe { Box::from_raw(data.cast::<Request>()) };
    // Node also calls this with null handles to dispose of queued calls during
    // shutdown. In that case dropping the sender wakes the waiting workers.
    if env.is_null() || function.is_null() || interrupted(&request.interrupt) {
        return;
    }
    let term = invoke(env, FunctionHandle::Local(function), &request.args);
    let _ = request.reply.send(term);
}

enum FunctionHandle {
    Local(sys::napi_value),
    Reference(sys::napi_ref),
}

fn invoke(env: sys::napi_env, function: FunctionHandle, args: &[Term]) -> Option<Term> {
    // Local N-API handles may only be accessed on the JS thread in a scope.
    let cx = unsafe { Env::from_raw(env) };
    cx.run_in_scope(|| {
        let result = catch_unwind(AssertUnwindSafe(|| {
            let function = match function {
                FunctionHandle::Local(function) => function,
                FunctionHandle::Reference(reference) => {
                    let mut function = ptr::null_mut();
                    status(unsafe { sys::napi_get_reference_value(env, reference, &mut function) })
                        .ok()?;
                    function
                }
            };
            if function.is_null() {
                return None;
            }
            let args = args
                .iter()
                .map(|arg| term(&cx, arg).map(|value| unsafe { value.raw() }))
                .collect::<Result<Vec<_>>>()
                .ok()?;
            let undefined = cx.get_undefined().ok()?;
            let mut result = ptr::null_mut();
            let status = unsafe {
                sys::napi_call_function(
                    env,
                    undefined.raw(),
                    function,
                    args.len(),
                    args.as_ptr(),
                    &mut result,
                )
            };
            if status != sys::Status::napi_ok {
                return None;
            }
            parse_term(unsafe { JsUnknown::from_raw_unchecked(env, result) })
        }))
        .ok()
        .flatten();
        // Property reads can throw without going through JsFunction::call.
        // The caller must not see a pending exception after a SPARQL error.
        let mut pending = false;
        unsafe {
            if sys::napi_is_exception_pending(env, &mut pending) == sys::Status::napi_ok && pending
            {
                let mut exception = ptr::null_mut();
                sys::napi_get_and_clear_last_exception(env, &mut exception);
                return Ok(None);
            }
        }
        Ok(result)
    })
    .ok()
    .flatten()
}

fn status(value: sys::napi_status) -> Result<()> {
    if value == sys::Status::napi_ok {
        Ok(())
    } else {
        Err(Error::new(
            Status::from(value),
            "could not set up scalar callback dispatch".to_owned(),
        ))
    }
}
