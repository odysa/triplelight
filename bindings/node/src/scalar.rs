use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::{CStr, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use napi::{Env, Error, JsFunction, JsUnknown, NapiRaw, NapiValue, Result, Status, ValueType, sys};
use triplelight::exec::Interrupt;
use triplelight::extension::{AggregateFunction, AggregateState, ScalarFunction};
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
    groups: Mutex<GroupReferences>,
}

#[derive(Default)]
struct GroupReferences {
    live: HashSet<usize>,
    pending_release: Vec<usize>,
}

struct GroupReference {
    endpoint: Arc<Endpoint>,
    reference: usize,
}

impl Drop for GroupReference {
    fn drop(&mut self) {
        self.endpoint.schedule_group_release(self.reference);
    }
}

enum Operation<'a> {
    Scalar(Cow<'a, [Term]>),
    Create,
    Step(Arc<GroupReference>, Cow<'a, Term>),
    Finish(Arc<GroupReference>),
}

impl Operation<'_> {
    fn into_owned(self) -> Operation<'static> {
        match self {
            Self::Scalar(args) => Operation::Scalar(Cow::Owned(args.into_owned())),
            Self::Create => Operation::Create,
            Self::Step(group, term) => Operation::Step(group, Cow::Owned(term.into_owned())),
            Self::Finish(group) => Operation::Finish(group),
        }
    }
}

enum Reply {
    Term(Term),
    Created(Arc<GroupReference>),
    Stepped,
}

pub(super) struct Callback(Arc<Endpoint>);

impl Callback {
    pub(super) fn new(env: &Env, function: JsFunction) -> Result<Self> {
        let name = env.create_string("TripleLight SPARQL callback")?;
        let raw_env = env.raw();
        let mut reference = ptr::null_mut();
        status(unsafe { sys::napi_create_reference(raw_env, function.raw(), 1, &mut reference) })?;
        let endpoint = Arc::new(Endpoint {
            tsfn: Mutex::new(None),
            reference: reference as usize,
            environment: raw_env as usize,
            cleanup_installed: Mutex::new(false),
            groups: Mutex::new(GroupReferences::default()),
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
                native_context,
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

    fn call_sync(&self, environment: usize, operation: Operation<'_>) -> Option<Reply> {
        if environment != self.0.environment || self.0.tsfn.lock().unwrap().is_none() {
            return None;
        }
        // The TLS guard came from the live native entry point. We are on the
        // environment's owning thread, so teardown cannot race this reference.
        invoke(
            environment as sys::napi_env,
            FunctionHandle::Reference(self.0.reference as sys::napi_ref),
            operation,
            &self.0,
        )
    }

    fn call_worker(
        &self,
        operation: Operation<'_>,
        interrupt: &Option<Arc<Interrupt>>,
    ) -> Option<Reply> {
        let (reply, result) = mpsc::channel();
        let mut request = Box::new(Request {
            operation: operation.into_owned(),
            endpoint: Arc::clone(&self.0),
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

    fn dispatch(&self, operation: Operation<'_>) -> Option<Reply> {
        let thread = QUERY_THREAD.with(|current| current.borrow().clone())?;
        match thread {
            QueryThread::JavaScript(env) => self.call_sync(env, operation),
            QueryThread::Worker(interrupt) => self.call_worker(operation, &interrupt),
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
        match self.dispatch(Operation::Scalar(Cow::Borrowed(args)))? {
            Reply::Term(term) => Some(term),
            _ => None,
        }
    }
}

pub(super) struct AggregateFactory(Arc<Callback>);

impl AggregateFactory {
    pub(super) fn new(env: &Env, function: JsFunction) -> Result<Self> {
        Callback::new(env, function).map(|callback| Self(Arc::new(callback)))
    }
}

impl AggregateFunction for AggregateFactory {
    fn create(&self) -> Option<Box<dyn AggregateState>> {
        let Reply::Created(group) = self.0.dispatch(Operation::Create)? else {
            return None;
        };
        Some(Box::new(AggregateGroup {
            group,
            callback: Arc::clone(&self.0),
        }))
    }
}

struct AggregateGroup {
    group: Arc<GroupReference>,
    callback: Arc<Callback>,
}

impl AggregateState for AggregateGroup {
    fn step(&mut self, term: &Term) -> Option<()> {
        let reply = self.callback.dispatch(Operation::Step(
            Arc::clone(&self.group),
            Cow::Borrowed(term),
        ))?;
        matches!(reply, Reply::Stepped).then_some(())
    }

    fn finish(&mut self) -> Option<Term> {
        match self
            .callback
            .dispatch(Operation::Finish(Arc::clone(&self.group)))?
        {
            Reply::Term(term) => Some(term),
            _ => None,
        }
    }
}

impl Endpoint {
    fn schedule_group_release(&self, reference: usize) {
        let mut handle = self.tsfn.lock().unwrap();
        let Some(tsfn) = *handle else {
            // The finalizer releases all remaining references.
            return;
        };
        let on_owner = QUERY_THREAD.with(|current| {
            matches!(current.borrow().as_ref(), Some(QueryThread::JavaScript(env)) if *env == self.environment)
        });
        if on_owner {
            self.groups.lock().unwrap().live.remove(&reference);
            drop(handle);
            unsafe {
                sys::napi_delete_reference(
                    self.environment as sys::napi_env,
                    reference as sys::napi_ref,
                )
            };
            return;
        }

        self.groups.lock().unwrap().pending_release.push(reference);
        // Null data requests cleanup without creating a reply or blocking the worker. If the
        // queue is full, the next queued call drains pending releases before executing.
        let status = unsafe {
            sys::napi_call_threadsafe_function(
                tsfn as sys::napi_threadsafe_function,
                ptr::null_mut(),
                sys::ThreadsafeFunctionCallMode::nonblocking,
            )
        };
        if status == sys::Status::napi_closing {
            *handle = None;
        }
    }

    fn drain_group_releases(&self, env: sys::napi_env) {
        let mut groups = self.groups.lock().unwrap();
        let pending = std::mem::take(&mut groups.pending_release);
        for reference in &pending {
            groups.live.remove(reference);
        }
        drop(groups);
        for reference in pending {
            unsafe { sys::napi_delete_reference(env, reference as sys::napi_ref) };
        }
    }

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
        let groups = std::mem::take(&mut *endpoint.groups.lock().unwrap());
        for reference in groups.live {
            unsafe { sys::napi_delete_reference(env, reference as sys::napi_ref) };
        }
        unsafe {
            if *endpoint.cleanup_installed.lock().unwrap() {
                sys::napi_remove_env_cleanup_hook(env, Some(cleanup), data);
            }
            sys::napi_delete_reference(env, endpoint.reference as sys::napi_ref);
        }
    }
}

struct Request {
    operation: Operation<'static>,
    endpoint: Arc<Endpoint>,
    interrupt: Option<Arc<Interrupt>>,
    reply: Sender<Option<Reply>>,
}

unsafe extern "C" fn call_js(
    env: sys::napi_env,
    function: sys::napi_value,
    context: *mut c_void,
    data: *mut c_void,
) {
    if !env.is_null() {
        let endpoint = unsafe { &*context.cast::<Endpoint>() };
        endpoint.drain_group_releases(env);
    }
    if data.is_null() {
        return;
    }
    let request = unsafe { Box::from_raw(data.cast::<Request>()) };
    // Node also calls this with null handles to dispose of queued calls during
    // shutdown. In that case dropping the sender wakes the waiting workers.
    if env.is_null() || function.is_null() || interrupted(&request.interrupt) {
        return;
    }
    let reply = invoke(
        env,
        FunctionHandle::Local(function),
        request.operation,
        &request.endpoint,
    );
    let _ = request.reply.send(reply);
}

enum FunctionHandle {
    Local(sys::napi_value),
    Reference(sys::napi_ref),
}

fn invoke(
    env: sys::napi_env,
    function: FunctionHandle,
    operation: Operation<'_>,
    endpoint: &Arc<Endpoint>,
) -> Option<Reply> {
    // Local N-API handles may only be accessed on the JS thread in a scope.
    let cx = unsafe { Env::from_raw(env) };
    cx.run_in_scope(|| {
        let result = catch_unwind(AssertUnwindSafe(|| {
            let function = match function {
                FunctionHandle::Local(function) => function,
                FunctionHandle::Reference(reference) => reference_value(env, reference)?,
            };
            if function.is_null() {
                return None;
            }
            execute(&cx, function, operation, endpoint)
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

fn execute(
    cx: &Env,
    function: sys::napi_value,
    operation: Operation<'_>,
    endpoint: &Arc<Endpoint>,
) -> Option<Reply> {
    let env = cx.raw();
    match operation {
        Operation::Scalar(args) => {
            let args = args
                .iter()
                .map(|arg| term(cx, arg).map(|value| unsafe { value.raw() }))
                .collect::<Result<Vec<_>>>()
                .ok()?;
            let receiver = unsafe { cx.get_undefined().ok()?.raw() };
            parse_term(call(cx, receiver, function, &args)?).map(Reply::Term)
        }
        Operation::Create => {
            let undefined = unsafe { cx.get_undefined().ok()?.raw() };
            let value = call(cx, undefined, function, &[])?;
            if value.get_type().ok()? != ValueType::Object {
                return None;
            }
            let receiver = unsafe { value.raw() };
            method(cx, receiver, c"step")?;
            method(cx, receiver, c"finish")?;
            let mut reference = ptr::null_mut();
            status(unsafe { sys::napi_create_reference(env, receiver, 1, &mut reference) }).ok()?;
            let reference = reference as usize;
            endpoint.groups.lock().unwrap().live.insert(reference);
            Some(Reply::Created(Arc::new(GroupReference {
                endpoint: Arc::clone(endpoint),
                reference,
            })))
        }
        Operation::Step(group, arg) => {
            let receiver = reference_value(env, group.reference as sys::napi_ref)?;
            let method = method(cx, receiver, c"step")?;
            let arg = unsafe { term(cx, &arg).ok()?.raw() };
            let result = call(cx, receiver, method, &[arg])?;
            (result.get_type().ok()? == ValueType::Undefined).then_some(Reply::Stepped)
        }
        Operation::Finish(group) => {
            let receiver = reference_value(env, group.reference as sys::napi_ref)?;
            let method = method(cx, receiver, c"finish")?;
            parse_term(call(cx, receiver, method, &[])?).map(Reply::Term)
        }
    }
}

fn reference_value(env: sys::napi_env, reference: sys::napi_ref) -> Option<sys::napi_value> {
    let mut value = ptr::null_mut();
    status(unsafe { sys::napi_get_reference_value(env, reference, &mut value) }).ok()?;
    (!value.is_null()).then_some(value)
}

fn method(cx: &Env, object: sys::napi_value, name: &CStr) -> Option<sys::napi_value> {
    let env = cx.raw();
    let mut property = ptr::null_mut();
    status(unsafe { sys::napi_get_named_property(env, object, name.as_ptr(), &mut property) })
        .ok()?;
    let value = unsafe { JsUnknown::from_raw_unchecked(env, property) };
    (value.get_type().ok()? == ValueType::Function).then_some(property)
}

fn call(
    cx: &Env,
    receiver: sys::napi_value,
    function: sys::napi_value,
    args: &[sys::napi_value],
) -> Option<JsUnknown> {
    let env = cx.raw();
    let mut result = ptr::null_mut();
    status(unsafe {
        sys::napi_call_function(
            env,
            receiver,
            function,
            args.len(),
            args.as_ptr(),
            &mut result,
        )
    })
    .ok()?;
    Some(unsafe { JsUnknown::from_raw_unchecked(env, result) })
}

fn status(value: sys::napi_status) -> Result<()> {
    if value == sys::Status::napi_ok {
        Ok(())
    } else {
        Err(Error::new(
            Status::from(value),
            "could not dispatch SPARQL callback".to_owned(),
        ))
    }
}
