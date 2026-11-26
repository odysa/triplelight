//! Python aggregate callbacks use the same RDF term conversions as scalar callbacks.
use pyo3::prelude::*;
use triplelight::extension::AggregateState;
use triplelight::rdf::Term;

use crate::{Classes, scalar, to_py};

struct State {
    step: Py<PyAny>,
    finish: Py<PyAny>,
}

pub(super) fn create(factory: &Py<PyAny>) -> Option<Box<dyn AggregateState>> {
    Python::with_gil(|py| {
        let state = factory.bind(py).call0().ok()?;
        let step = state.getattr("step").ok()?;
        let finish = state.getattr("finish").ok()?;
        if !step.is_callable() || !finish.is_callable() {
            return None;
        }
        Some(Box::new(State {
            step: step.unbind(),
            finish: finish.unbind(),
        }) as Box<dyn AggregateState>)
    })
}

impl AggregateState for State {
    fn step(&mut self, term: &Term) -> Option<()> {
        Python::with_gil(|py| {
            let classes = Classes::get(py).ok()?;
            let arg = to_py(py, &classes, term).ok()?;
            let result = self.step.bind(py).call1((arg,)).ok()?;
            result.is_none().then_some(())
        })
    }

    fn finish(&mut self) -> Option<Term> {
        Python::with_gil(|py| {
            let classes = Classes::get(py).ok()?;
            let value = self.finish.bind(py).call0().ok()?;
            scalar::from_py(&classes, &value, 0)
        })
    }
}
