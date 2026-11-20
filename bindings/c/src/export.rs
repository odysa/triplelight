//! The C callbacks of the stream returned by `tl_query_arrow`.
//!
//! arrow-rs's `FFI_ArrowArrayStream::new` runs schema conversion, batch export and error-string construction (which
//! panics on an interior NUL) inside its own `extern "C"` callbacks, where a panic aborts the host process. These
//! callbacks run the same work under `catch_unwind`: every failure, a panic included, becomes a nonzero errno with
//! a message for `get_last_error`. After a failed `get_next` the stream is poisoned and every further `get_next`
//! returns the same error. The release callbacks of the exported batches and schemas are still arrow-rs's; they
//! only drop the buffers they own.
use std::ffi::{c_char, c_int, c_void, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

use arrow_array::ffi::{FFI_ArrowArray, FFI_ArrowSchema};
use arrow_array::ffi_stream::FFI_ArrowArrayStream;
use arrow_array::{Array, RecordBatchReader, StructArray};
use arrow_schema::ArrowError;

const EIO: c_int = 5;
const ENOMEM: c_int = 12;
const EINVAL: c_int = 22;
const ENOSYS: c_int = 38;

/// The C layout of `struct ArrowArrayStream` (identical to `FFI_ArrowArrayStream`, whose fields are private).
#[repr(C)]
struct RawStream {
    get_schema: Option<unsafe extern "C" fn(*mut RawStream, *mut FFI_ArrowSchema) -> c_int>,
    get_next: Option<unsafe extern "C" fn(*mut RawStream, *mut FFI_ArrowArray) -> c_int>,
    get_last_error: Option<unsafe extern "C" fn(*mut RawStream) -> *const c_char>,
    release: Option<unsafe extern "C" fn(*mut RawStream)>,
    private_data: *mut c_void,
}

const _: () = assert!(size_of::<RawStream>() == size_of::<FFI_ArrowArrayStream>());

struct Private {
    reader: Box<dyn RecordBatchReader + Send>,
    last_error: Option<CString>,
    /// Set by a failed `get_next`: (errno, message) returned by every later pull.
    poisoned: Option<(c_int, String)>,
}

/// Writes a stream reading from `reader` into `out`. The caller owns the stream and must release it.
///
/// # Safety
/// `out` must be valid for a write of one `struct ArrowArrayStream`; its previous contents are not read or released.
pub unsafe fn export(reader: Box<dyn RecordBatchReader + Send>, out: *mut FFI_ArrowArrayStream) {
    let private = Box::new(Private { reader, last_error: None, poisoned: None });
    let raw = RawStream {
        get_schema: Some(get_schema),
        get_next: Some(get_next),
        get_last_error: Some(get_last_error),
        release: Some(release),
        private_data: Box::into_raw(private) as *mut c_void,
    };
    unsafe { ptr::write(out as *mut RawStream, raw) }
}

fn errno(e: &ArrowError) -> c_int {
    match e {
        ArrowError::NotYetImplemented(_) => ENOSYS,
        ArrowError::MemoryError(_) => ENOMEM,
        ArrowError::IoError(..) => EIO,
        _ => EINVAL,
    }
}

fn message(text: String) -> CString {
    CString::new(text.replace('\0', "\\0")).unwrap_or_default()
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    let what = payload.downcast_ref::<&str>().copied().or_else(|| payload.downcast_ref::<String>().map(String::as_str));
    format!("internal panic in the TripleLight Arrow stream: {}", what.unwrap_or("(no message)"))
}

/// Runs one callback body on the stream's private data. Errors and panics are recorded for `get_last_error`.
unsafe fn guarded(stream: *mut RawStream, body: impl FnOnce(&mut Private) -> Result<(), (c_int, String)>) -> c_int {
    if stream.is_null() { return EINVAL }
    let private = unsafe { (*stream).private_data as *mut Private };
    if private.is_null() { return EINVAL } // released
    let private = unsafe { &mut *private };
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        private.last_error = None;
        match body(private) {
            Ok(()) => 0,
            Err((code, text)) => { private.last_error = Some(message(text)); code }
        }
    }));
    match outcome {
        Ok(code) => code,
        Err(payload) => {
            // Building the message cannot unwind out of here: a second panic is caught and leaves no message.
            let text = catch_unwind(AssertUnwindSafe(|| panic_text(payload.as_ref()))).unwrap_or_default();
            let _ = catch_unwind(AssertUnwindSafe(|| {
                private.poisoned = Some((EIO, text.clone()));
                private.last_error = Some(message(text));
            }));
            std::mem::forget(payload); // dropping a foreign payload could panic again
            EIO
        }
    }
}

unsafe extern "C" fn get_schema(stream: *mut RawStream, out: *mut FFI_ArrowSchema) -> c_int {
    unsafe {
        guarded(stream, |p| {
            if out.is_null() { return Err((EINVAL, "null output schema".into())) }
            let schema = FFI_ArrowSchema::try_from(p.reader.schema().as_ref()).map_err(|e| (errno(&e), e.to_string()))?;
            ptr::write(out, schema);
            Ok(())
        })
    }
}

unsafe extern "C" fn get_next(stream: *mut RawStream, out: *mut FFI_ArrowArray) -> c_int {
    unsafe {
        guarded(stream, |p| {
            if out.is_null() { return Err((EINVAL, "null output array".into())) }
            if let Some(e) = &p.poisoned { return Err(e.clone()) }
            let next = p.reader.next(); // a panic here is caught by `guarded`, which poisons the stream
            match next {
                None => ptr::write(out, FFI_ArrowArray::empty()), // end of stream: a released array
                Some(Ok(batch)) => {
                    let array = StructArray::from(batch);
                    ptr::write(out, FFI_ArrowArray::new(&array.to_data()));
                }
                Some(Err(e)) => {
                    let failure = (errno(&e), e.to_string());
                    p.poisoned = Some(failure.clone());
                    return Err(failure);
                }
            }
            Ok(())
        })
    }
}

unsafe extern "C" fn get_last_error(stream: *mut RawStream) -> *const c_char {
    if stream.is_null() { return ptr::null() }
    let private = unsafe { (*stream).private_data as *const Private };
    if private.is_null() { return ptr::null() }
    unsafe { (*private).last_error.as_ref().map_or(ptr::null(), |m| m.as_ptr()) }
}

unsafe extern "C" fn release(stream: *mut RawStream) {
    if stream.is_null() { return }
    let s = unsafe { &mut *stream };
    let private = std::mem::replace(&mut s.private_data, ptr::null_mut()) as *mut Private;
    s.release = None;
    if private.is_null() { return }
    // Dropping the reader drops its snapshot; a panic there leaks what is left rather than unwinding into C.
    let _ = catch_unwind(AssertUnwindSafe(|| drop(unsafe { Box::from_raw(private) })));
}
