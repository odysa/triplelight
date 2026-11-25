//! C scalar callbacks borrow the same term table and UTF-8 arena as query results.
use std::ffi::{c_char, c_void};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use triplelight::extension::{FunctionOptions, ScalarFunction};
use triplelight::rdf::Term;

use crate::{
    Builder, TL_ERR_INVALID_ARGUMENT, TL_OK, TlDb, TlError, TlTerm, extension_error, guard, handle,
    invalid, text,
};

#[repr(C)]
pub struct TlScalarArgs {
    pub len: usize,
    pub cells: *const u32,
    pub n_terms: usize,
    pub terms: *const TlTerm,
    pub n_bytes: usize,
    pub bytes: *const u8,
}

pub struct TlScalarOutput(Option<Term>);

pub type TlScalarCallback =
    unsafe extern "C" fn(*mut c_void, *const TlScalarArgs, *mut TlScalarOutput) -> i32;
pub type TlScalarRelease = unsafe extern "C" fn(*mut c_void);

struct Callback {
    context: *mut c_void,
    call: TlScalarCallback,
    release: Option<TlScalarRelease>,
    adopted: AtomicBool,
}

// SAFETY: Registration requires the host callback and context to support concurrent calls
// and final release on any thread. Rust never dereferences the opaque context.
unsafe impl Send for Callback {}
unsafe impl Sync for Callback {}

impl Drop for Callback {
    fn drop(&mut self) {
        if self.adopted.load(Ordering::Acquire)
            && let Some(release) = self.release
        {
            unsafe { release(self.context) };
        }
    }
}

struct Scalar(Arc<Callback>);

impl ScalarFunction for Scalar {
    fn call(&self, args: &[Term]) -> Option<Term> {
        let mut arena = Builder::default();
        let cells = args
            .iter()
            .map(|term| arena.term(term))
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        let args = TlScalarArgs {
            len: cells.len(),
            cells: cells.as_ptr(),
            n_terms: arena.terms.len(),
            terms: arena.terms.as_ptr(),
            n_bytes: arena.bytes.len(),
            bytes: arena.bytes.as_ptr(),
        };
        let mut output = TlScalarOutput(None);
        let status = unsafe { (self.0.call)(self.0.context, &args, &mut output) };
        if status == TL_OK { output.0 } else { None }
    }
}

/// Stores one owned RDF term in the callback's output slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tl_scalar_set_term(
    output: *mut TlScalarOutput,
    term: *const c_char,
    len: usize,
) -> i32 {
    unsafe {
        guard(std::ptr::null_mut(), || {
            let output = output
                .as_mut()
                .ok_or_else(|| invalid("null scalar output"))?;
            output.0 = None;
            let term = text(term, len)?.parse::<Term>().map_err(|e| TlError {
                kind: TL_ERR_INVALID_ARGUMENT,
                message: e.to_string(),
            })?;
            output.0 = Some(term);
            Ok(())
        })
    }
}

/// Registers a scalar function. On success the registry owns the callback context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tl_register_function(
    db: *const TlDb,
    iri: *const c_char,
    len: usize,
    arity: usize,
    pure: i32,
    call: Option<TlScalarCallback>,
    context: *mut c_void,
    release: Option<TlScalarRelease>,
    err: *mut *mut TlError,
) -> i32 {
    unsafe {
        guard(err, || {
            let db = handle(db)?;
            let iri = text(iri, len)?;
            let call = call.ok_or_else(|| invalid("null scalar callback"))?;
            if !matches!(pure, 0 | 1) {
                return Err(invalid("scalar purity must be 0 or 1"));
            }
            let options = FunctionOptions {
                arity: (arity != usize::MAX).then_some(arity),
                pure: pure == 1,
            };
            let callback = Arc::new(Callback {
                context,
                call,
                release,
                adopted: AtomicBool::new(false),
            });
            db.register_function(iri, options, Scalar(Arc::clone(&callback)))
                .map_err(extension_error)?;
            callback.adopted.store(true, Ordering::Release);
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TL_TERM_LITERAL, tl_close, tl_error_free};
    use std::sync::atomic::AtomicUsize;
    use triplelight::embedded::Database;

    unsafe extern "C" fn echo(
        _: *mut c_void,
        args: *const TlScalarArgs,
        output: *mut TlScalarOutput,
    ) -> i32 {
        let args = unsafe { &*args };
        if args.len != 1 {
            return TL_ERR_INVALID_ARGUMENT;
        }
        let entry = unsafe { &*args.terms.add(*args.cells as usize - 1) };
        let value = unsafe {
            std::slice::from_raw_parts(
                args.bytes.add(entry.value.off as usize),
                entry.value.len as usize,
            )
        };
        if entry.kind != TL_TERM_LITERAL || value != b"hello" {
            return TL_ERR_INVALID_ARGUMENT;
        }
        let result = c"\"bonjour\"@fr";
        unsafe { tl_scalar_set_term(output, result.as_ptr(), result.to_bytes().len()) }
    }

    unsafe extern "C" fn release(context: *mut c_void) {
        unsafe { &*context.cast::<AtomicUsize>() }.fetch_add(1, Ordering::AcqRel);
    }

    #[test]
    fn registration_is_snapshot_local_and_releases_only_after_last_reference() {
        let released = AtomicUsize::new(0);
        let rejected = AtomicUsize::new(0);
        let db = Box::into_raw(Box::new(TlDb(Database::in_memory())));
        let inner = unsafe { handle(db) }.unwrap_or_else(|_| panic!("database handle"));
        let old = inner.read();
        let iri = c"urn:echo";
        let mut err = std::ptr::null_mut();
        let register = |flag: &AtomicUsize, err: *mut *mut TlError| unsafe {
            tl_register_function(
                db,
                iri.as_ptr(),
                iri.to_bytes().len(),
                1,
                1,
                Some(echo),
                std::ptr::from_ref(flag).cast_mut().cast(),
                Some(release),
                err,
            )
        };
        assert_eq!(register(&released, &mut err), TL_OK);
        assert_ne!(register(&rejected, &mut err), TL_OK);
        unsafe { tl_error_free(err) };
        assert_eq!(rejected.load(Ordering::Acquire), 0);
        let q = r#"SELECT (<urn:echo>("hello") AS ?value) WHERE {}"#;
        assert!(old.query_json(q).is_err());
        let new = inner.read();
        unsafe { tl_close(db) };
        assert_eq!(released.load(Ordering::Acquire), 0);
        let json = new.query_json(q).expect("registered callback");
        assert!(json.contains("bonjour") && json.contains("xml:lang") && json.contains("fr"));
        drop(new);
        assert_eq!(released.load(Ordering::Acquire), 1);
    }
}
