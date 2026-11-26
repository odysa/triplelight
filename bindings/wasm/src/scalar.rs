use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use js_sys::{Array, Function, Reflect};
use triplelight::extension::{ExtensionRegistry, FunctionOptions, ScalarFunction};
use triplelight::rdf::Term;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};

use super::rdfjs::{parse_term, term};

#[cfg(target_feature = "atomics")]
compile_error!("Wasm scalar callbacks require a single JavaScript thread");

thread_local! {
    static NEXT_CALLBACK: Cell<u64> = const { Cell::new(0) };
    static CALLBACKS: RefCell<HashMap<u64, Function>> = RefCell::default();
    static IGNORE_REJECTION: Closure<dyn FnMut(JsValue)> = Closure::new(|_: JsValue| {});
}

// The executor requires Send + Sync. Only the ID crosses that interface. The
// JavaScript handle stays on its owning agent, including during final release.
struct Callback(u64);

impl ScalarFunction for Callback {
    fn call(&self, args: &[Term]) -> Option<Term> {
        let function = CALLBACKS.with(|callbacks| callbacks.borrow().get(&self.0).cloned())?;
        // A host callback can invoke JavaScript that drops other registrations.
        // Release the table borrow before invoking any host code.
        let args = args
            .iter()
            .map(|arg| JsValue::from(term(arg)))
            .collect::<Array>();
        let result = Reflect::apply(&function, &JsValue::UNDEFINED, &args).ok()?;
        let term = parse_term(&result);
        if term.is_none() && result.is_object() {
            let _ = handle_invalid_promise(&result);
        }
        term
    }
}

impl Drop for Callback {
    fn drop(&mut self) {
        let function = CALLBACKS.try_with(|callbacks| callbacks.borrow_mut().remove(&self.0));
        drop(function);
    }
}

// A synchronous callback may accidentally return a Promise. Use the native
// method so ordinary objects and thenables cannot invoke their own "then".
fn handle_invalid_promise(value: &JsValue) -> Option<()> {
    let constructor = Reflect::get(&js_sys::global(), &"Promise".into()).ok()?;
    let prototype = Reflect::get(&constructor, &"prototype".into()).ok()?;
    let then = Reflect::get(&prototype, &"then".into())
        .ok()?
        .dyn_into::<Function>()
        .ok()?;
    IGNORE_REJECTION.with(|handler| {
        let args = Array::of2(&JsValue::UNDEFINED, handler.as_ref());
        Reflect::apply(&then, value, &args).ok()?;
        Some(())
    })
}

pub(super) fn register(
    registry: &ExtensionRegistry,
    iri: &str,
    function: Function,
    arity: Option<f64>,
    pure: Option<bool>,
) -> Result<(), String> {
    if !function.is_function() {
        return Err("scalar callback must be a function".into());
    }
    if arity.is_some_and(|n| !(0.0..=f64::from(u32::MAX)).contains(&n) || n.fract() != 0.0) {
        return Err("scalar arity must be a non-negative integer".into());
    }
    let id = NEXT_CALLBACK
        .with(|next| {
            let id = next.get().checked_add(1)?;
            next.set(id);
            Some(id)
        })
        .ok_or("scalar callback IDs exhausted")?;
    CALLBACKS.with(|callbacks| callbacks.borrow_mut().insert(id, function));
    let options = FunctionOptions {
        arity: arity.map(|n| n as usize),
        pure: pure.unwrap_or(false),
    };
    registry
        .register_function(iri, options, Callback(id))
        .map_err(|err| err.to_string())
}
