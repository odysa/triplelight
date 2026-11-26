//! C aggregate factories retain their registration context separately from each query group's state.
use std::ffi::{c_char, c_void};
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use triplelight::extension::{AggregateFunction, AggregateState};
use triplelight::rdf::Term;

use crate::{
    TL_OK, TlDb, TlError, TlScalarArgs, TlScalarOutput, TlScalarRelease, extension_error, guard,
    handle, invalid, text,
};

pub type TlAggregateCreate = unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> i32;
pub type TlAggregateStep = unsafe extern "C" fn(*mut c_void, *const TlScalarArgs) -> i32;
pub type TlAggregateFinish = unsafe extern "C" fn(*mut c_void, *mut TlScalarOutput) -> i32;

#[repr(C)]
pub struct TlAggregateCallbacks {
    pub create: Option<TlAggregateCreate>,
    pub step: Option<TlAggregateStep>,
    pub finish: Option<TlAggregateFinish>,
    pub drop_state: Option<TlScalarRelease>,
    pub release_context: Option<TlScalarRelease>,
}

struct Callbacks {
    context: *mut c_void,
    create: TlAggregateCreate,
    step: TlAggregateStep,
    finish: TlAggregateFinish,
    drop_state: Option<TlScalarRelease>,
    release_context: Option<TlScalarRelease>,
    adopted: AtomicBool,
}

// SAFETY: the C registration contract permits concurrent factory calls and context release on
// any thread. Rust never dereferences the context. Each successful state is query-thread local.
unsafe impl Send for Callbacks {}
unsafe impl Sync for Callbacks {}

impl Drop for Callbacks {
    fn drop(&mut self) {
        if self.adopted.load(Ordering::Acquire)
            && let Some(release) = self.release_context
        {
            unsafe { release(self.context) };
        }
    }
}

struct Factory(Arc<Callbacks>);

impl AggregateFunction for Factory {
    fn create(&self) -> Option<Box<dyn AggregateState>> {
        let mut state = ptr::null_mut();
        let status = unsafe { (self.0.create)(self.0.context, &mut state) };
        if status != TL_OK {
            return None;
        }
        Some(Box::new(Group {
            state,
            callbacks: Arc::clone(&self.0),
        }))
    }
}

struct Group {
    state: *mut c_void,
    callbacks: Arc<Callbacks>,
}

impl Drop for Group {
    fn drop(&mut self) {
        if let Some(drop_state) = self.callbacks.drop_state {
            unsafe { drop_state(self.state) };
        }
    }
}

impl AggregateState for Group {
    fn step(&mut self, term: &Term) -> Option<()> {
        TlScalarArgs::with_terms(std::slice::from_ref(term), |arg| {
            let status = unsafe { (self.callbacks.step)(self.state, arg) };
            (status == TL_OK).then_some(())
        })
    }

    fn finish(&mut self) -> Option<Term> {
        TlScalarOutput::capture(|output| unsafe { (self.callbacks.finish)(self.state, output) })
    }
}

/// Registers a unary aggregate. On success the registry adopts the callback context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tl_register_aggregate_function(
    db: *const TlDb,
    iri: *const c_char,
    len: usize,
    callbacks: *const TlAggregateCallbacks,
    context: *mut c_void,
    err: *mut *mut TlError,
) -> i32 {
    unsafe {
        guard(err, || {
            let db = handle(db)?;
            let iri = text(iri, len)?;
            let callbacks = callbacks
                .as_ref()
                .ok_or_else(|| invalid("null aggregate callbacks"))?;
            let create = callbacks
                .create
                .ok_or_else(|| invalid("null aggregate create callback"))?;
            let step = callbacks
                .step
                .ok_or_else(|| invalid("null aggregate step callback"))?;
            let finish = callbacks
                .finish
                .ok_or_else(|| invalid("null aggregate finish callback"))?;
            let callbacks = Arc::new(Callbacks {
                context,
                create,
                step,
                finish,
                drop_state: callbacks.drop_state,
                release_context: callbacks.release_context,
                adopted: AtomicBool::new(false),
            });
            db.register_aggregate_function(iri, Factory(Arc::clone(&callbacks)))
                .map_err(extension_error)?;
            callbacks.adopted.store(true, Ordering::Release);
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        TL_ERR_INVALID_ARGUMENT, TL_TERM_LITERAL, tl_close, tl_error_free, tl_scalar_set_term,
    };
    use std::sync::atomic::AtomicUsize;
    use triplelight::embedded::Database;

    #[derive(Default)]
    struct Counts {
        created: AtomicUsize,
        dropped: AtomicUsize,
        released: AtomicUsize,
    }

    struct CountState {
        total: usize,
        counts: *const Counts,
    }

    unsafe extern "C" fn create(context: *mut c_void, state: *mut *mut c_void) -> i32 {
        let counts = context.cast::<Counts>();
        unsafe { &*counts }.created.fetch_add(1, Ordering::Relaxed);
        let owned = Box::new(CountState { total: 0, counts });
        unsafe { *state = Box::into_raw(owned).cast() };
        TL_OK
    }

    unsafe extern "C" fn step(state: *mut c_void, arg: *const TlScalarArgs) -> i32 {
        let arg = unsafe { &*arg };
        if arg.len != 1 {
            return TL_ERR_INVALID_ARGUMENT;
        }
        let cell = unsafe { *arg.cells } as usize;
        if cell == 0 || cell > arg.n_terms {
            return TL_ERR_INVALID_ARGUMENT;
        }
        let term = unsafe { &*arg.terms.add(cell - 1) };
        if term.kind != TL_TERM_LITERAL {
            return TL_ERR_INVALID_ARGUMENT;
        }
        unsafe { &mut *state.cast::<CountState>() }.total += 1;
        TL_OK
    }

    unsafe extern "C" fn finish(state: *mut c_void, output: *mut TlScalarOutput) -> i32 {
        let total = unsafe { &*state.cast::<CountState>() }.total;
        let term = format!("\"{total}\"^^<http://www.w3.org/2001/XMLSchema#integer>");
        unsafe { tl_scalar_set_term(output, term.as_ptr().cast(), term.len()) }
    }

    unsafe extern "C" fn drop_state(state: *mut c_void) {
        let state = unsafe { Box::from_raw(state.cast::<CountState>()) };
        unsafe { &*state.counts }
            .dropped
            .fetch_add(1, Ordering::Relaxed);
    }

    unsafe extern "C" fn release_context(context: *mut c_void) {
        unsafe { &*context.cast::<Counts>() }
            .released
            .fetch_add(1, Ordering::Relaxed);
    }

    #[test]
    fn aggregate_callbacks_use_independent_group_state_and_release_after_the_snapshot() {
        let counts = Counts::default();
        let duplicate = Counts::default();
        let db = Box::into_raw(Box::new(TlDb(Database::in_memory())));
        let inner = unsafe { handle(db) }.unwrap_or_else(|_| panic!("database handle"));
        let old = inner.read();
        let iri = c"urn:count-literals";
        let register = |counts: &Counts, err| unsafe {
            let callbacks = TlAggregateCallbacks {
                create: Some(create),
                step: Some(step),
                finish: Some(finish),
                drop_state: Some(drop_state),
                release_context: Some(release_context),
            };
            tl_register_aggregate_function(
                db,
                iri.as_ptr(),
                iri.to_bytes().len(),
                &callbacks,
                ptr::from_ref(counts).cast_mut().cast(),
                err,
            )
        };
        let mut err = ptr::null_mut();
        assert_eq!(register(&counts, &mut err), TL_OK);
        assert_ne!(register(&duplicate, &mut err), TL_OK);
        unsafe { tl_error_free(err) };
        assert_eq!(duplicate.released.load(Ordering::Relaxed), 0);
        let grouped = r#"SELECT ?g (<urn:count-literals>(DISTINCT ?v) AS ?n) WHERE {
            VALUES (?g ?v) { (1 "x") (1 "x") (1 UNDEF) (2 "y") (2 "z") }
        } GROUP BY ?g ORDER BY ?g"#;
        assert!(old.query_json(grouped).is_err());
        let new = inner.read();
        unsafe { tl_close(db) };
        let builtin = grouped.replace("<urn:count-literals>", "COUNT");
        assert_eq!(
            new.query_json(grouped).expect("aggregate"),
            new.query_json(&builtin).expect("COUNT")
        );
        let empty = "SELECT (<urn:count-literals>(?v) AS ?n) WHERE { FILTER(false) }";
        let builtin = empty.replace("<urn:count-literals>", "COUNT");
        assert_eq!(
            new.query_json(empty).expect("empty group"),
            new.query_json(&builtin).expect("empty COUNT")
        );

        let failed = "SELECT (<urn:count-literals>(?v) AS ?n) WHERE { VALUES ?v { <urn:bad> } }";
        let unbound = "SELECT ?n WHERE {}";
        assert_eq!(
            new.query_json(failed).expect("failed group"),
            new.query_json(unbound).expect("unbound")
        );
        assert_eq!(counts.created.load(Ordering::Relaxed), 4);
        assert_eq!(counts.dropped.load(Ordering::Relaxed), 4);
        assert_eq!(counts.released.load(Ordering::Relaxed), 0);
        drop(new);
        assert_eq!(counts.released.load(Ordering::Relaxed), 1);
    }
}
