//! Convert Python scalar callback arguments and return values under the GIL.
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyInt, PyTuple};
use triplelight::rdf::{BlankNode, Literal, NamedNode, Term};

use crate::{Classes, to_py};

pub(super) fn call(callback: &Py<PyAny>, args: &[Term]) -> Option<Term> {
    Python::with_gil(|py| {
        let classes = Classes::get(py).ok()?;
        let args = args
            .iter()
            .map(|term| to_py(py, &classes, term))
            .collect::<PyResult<Vec<_>>>()
            .ok()?;
        let args = PyTuple::new(py, args).ok()?;
        let value = callback.bind(py).call1(args).ok()?;
        from_py(&classes, &value, 0)
    })
}

pub(super) fn from_py(classes: &Classes<'_>, value: &Bound<'_, PyAny>, depth: usize) -> Option<Term> {
    if value.is_instance(&classes.iri).ok()? {
        return Some(NamedNode::new(value.extract::<String>().ok()?).ok()?.into());
    }
    if value.is_instance(&classes.bnode).ok()? {
        return Some(BlankNode::new(value.extract::<String>().ok()?).ok()?.into());
    }
    if value.is_instance(&classes.literal).ok()? {
        return Some(literal(value)?.into());
    }
    #[cfg(feature = "rdf-12")]
    if value.is_instance(&classes.triple).ok()? {
        use triplelight::rdf::Triple;

        // Match the engine's RDF 1.2 term depth limit, including cyclic Python objects.
        const MAX_TRIPLE_TERM_DEPTH: usize = 63;
        if depth >= MAX_TRIPLE_TERM_DEPTH {
            return None;
        }
        let subject = from_py(classes, &value.getattr("subject").ok()?, depth + 1)?;
        let predicate = from_py(classes, &value.getattr("predicate").ok()?, depth + 1)?;
        let object = from_py(classes, &value.getattr("object").ok()?, depth + 1)?;
        return Some(Triple::from_terms(subject, predicate, object).ok()?.into());
    }
    #[cfg(not(feature = "rdf-12"))]
    let _ = depth;

    if let Ok(value) = value.downcast::<PyBool>() {
        return Some(Literal::from(value.is_true()).into());
    }
    if let Ok(value) = value.downcast::<PyInt>() {
        return Some(Literal::from(value.extract::<i64>().ok()?).into());
    }
    value
        .extract::<String>()
        .ok()
        .map(Literal::new_simple_literal)
        .map(Into::into)
}

fn literal(value: &Bound<'_, PyAny>) -> Option<Literal> {
    let lexical: String = value.getattr("value").ok()?.extract().ok()?;
    let datatype: Option<String> = value.getattr("datatype").ok()?.extract().ok()?;
    let language: Option<String> = value.getattr("lang").ok()?.extract().ok()?;
    let direction: Option<String> = value.getattr("direction").ok()?.extract().ok()?;
    match (datatype, language, direction.as_deref()) {
        (None, None, None) => Some(Literal::new_simple_literal(lexical)),
        (Some(datatype), None, None) => {
            let datatype = NamedNode::new(datatype).ok()?;
            Some(Literal::new_typed_literal(lexical, datatype))
        }
        (None, Some(language), None) => {
            Literal::new_language_tagged_literal(lexical, language).ok()
        }
        #[cfg(feature = "rdf-12")]
        (None, Some(language), Some(direction)) => {
            use triplelight::rdf::BaseDirection;

            let direction = match direction {
                "ltr" => BaseDirection::Ltr,
                "rtl" => BaseDirection::Rtl,
                _ => return None,
            };
            Literal::new_directional_language_tagged_literal(lexical, language, direction).ok()
        }
        _ => None,
    }
}
