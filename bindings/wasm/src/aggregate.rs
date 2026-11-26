use js_sys::{Function, Reflect};
use triplelight::extension::{AggregateFunction, AggregateState, ExtensionRegistry};
use triplelight::rdf::Term;
use wasm_bindgen::{JsCast, JsValue};

use super::rdfjs::term;
use super::scalar::{Callback, callback_term, handle_invalid_promise};

struct Factory(Callback);

impl AggregateFunction for Factory {
    fn create(&self) -> Option<Box<dyn AggregateState>> {
        let receiver = self.0.function()?.call0(&JsValue::UNDEFINED).ok()?;
        if !receiver.is_object() {
            return None;
        }
        let (Some(step), Some(finish)) = (method(&receiver, "step"), method(&receiver, "finish"))
        else {
            let _ = handle_invalid_promise(&receiver);
            return None;
        };
        Some(Box::new(State {
            receiver,
            step,
            finish,
        }))
    }
}

fn method(receiver: &JsValue, name: &str) -> Option<Function> {
    Reflect::get(receiver, &name.into()).ok()?.dyn_into().ok()
}

struct State {
    receiver: JsValue,
    step: Function,
    finish: Function,
}

impl AggregateState for State {
    fn step(&mut self, arg: &Term) -> Option<()> {
        let arg = term(arg);
        let result = self.step.call1(&self.receiver, &arg).ok()?;
        if result.is_undefined() {
            return Some(());
        }
        if result.is_object() {
            let _ = handle_invalid_promise(&result);
        }
        None
    }

    fn finish(&mut self) -> Option<Term> {
        callback_term(&self.finish.call0(&self.receiver).ok()?)
    }
}

pub(super) fn register(
    registry: &ExtensionRegistry,
    iri: &str,
    factory: Function,
) -> Result<(), String> {
    if !factory.is_function() {
        return Err("aggregate factory must be a function".into());
    }
    registry
        .register_aggregate_function(iri, Factory(Callback::new(factory)?))
        .map_err(|err| err.to_string())
}
