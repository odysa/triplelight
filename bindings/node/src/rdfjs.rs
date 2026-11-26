use napi::{Env, Error, JsObject, JsString, JsUnknown, Result, Status};
use triplelight::rdf::{BlankNode, Literal, NamedNode, Term};

const LANG_STRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString";
const DIR_LANG_STRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#dirLangString";

pub(super) fn named(env: &Env, iri: &str) -> Result<JsObject> {
    let mut o = env.create_object()?;
    o.set_named_property("termType", env.create_string("NamedNode")?)?;
    o.set_named_property("value", env.create_string(iri)?)?;
    Ok(o)
}

fn literal(env: &Env, l: &Literal) -> Result<JsObject> {
    let mut o = env.create_object()?;
    o.set_named_property("termType", env.create_string("Literal")?)?;
    o.set_named_property("value", env.create_string(l.value())?)?;
    o.set_named_property("language", env.create_string(l.language().unwrap_or(""))?)?;
    o.set_named_property("datatype", named(env, l.datatype().as_str())?)?;
    // RDF 1.2 base direction: "ltr" or "rtl"
    // set only when present, so a build with the feature on keeps the exact 1.1 shape for every other literal
    #[cfg(feature = "rdf-12")]
    if let Some(dir) = l.direction() {
        let dir = match dir {
            triplelight::rdf::BaseDirection::Ltr => "ltr",
            triplelight::rdf::BaseDirection::Rtl => "rtl",
        };
        o.set_named_property("direction", env.create_string(dir)?)?;
    }
    Ok(o)
}

pub(super) fn term(env: &Env, t: &Term) -> Result<JsObject> {
    match t {
        Term::NamedNode(n) => named(env, n.as_str()),
        Term::BlankNode(b) => {
            let mut o = env.create_object()?;
            o.set_named_property("termType", env.create_string("BlankNode")?)?;
            o.set_named_property("value", env.create_string(b.as_str())?)?;
            Ok(o)
        }
        Term::Literal(l) => literal(env, l),
        // RDF 1.2 triple term: an RDF/JS Quad term in the default graph, parts converted recursively
        #[cfg(feature = "rdf-12")]
        Term::Triple(t) => {
            let mut o = env.create_object()?;
            o.set_named_property("termType", env.create_string("Quad")?)?;
            o.set_named_property("value", env.create_string("")?)?;
            let subject: Term = t.subject.clone().into();
            let predicate: Term = t.predicate.clone().into();
            o.set_named_property("subject", term(env, &subject)?)?;
            o.set_named_property("predicate", term(env, &predicate)?)?;
            o.set_named_property("object", term(env, &t.object)?)?;
            let mut g = env.create_object()?;
            g.set_named_property("termType", env.create_string("DefaultGraph")?)?;
            g.set_named_property("value", env.create_string("")?)?;
            o.set_named_property("graph", g)?;
            Ok(o)
        }
        #[allow(unreachable_patterns)]
        _ => Err(Error::new(
            Status::GenericFailure,
            "UnsupportedError: unsupported term kind".to_string(),
        )),
    }
}

fn string(object: &JsObject, field: &str) -> Option<String> {
    object
        .get_named_property::<JsString>(field)
        .ok()?
        .into_utf16()
        .ok()?
        .as_str()
        .ok()
}

fn object(input: JsUnknown) -> Option<JsObject> {
    if input.get_type().ok()? != napi::ValueType::Object || input.is_promise().ok()? {
        return None;
    }
    Some(unsafe { input.cast() })
}

pub(super) fn parse_term(input: JsUnknown) -> Option<Term> {
    decode(input, 0)
}

fn decode(input: JsUnknown, depth: usize) -> Option<Term> {
    let _ = depth; // The minimal build has no recursive terms.
    let input = object(input)?;
    let value = string(&input, "value")?;
    match string(&input, "termType")?.as_str() {
        "NamedNode" => NamedNode::new(value).ok().map(Term::from),
        "BlankNode" => BlankNode::new(value).ok().map(Term::from),
        "Literal" => parse_literal(&input, value).map(Term::from),
        #[cfg(feature = "rdf-12")]
        "Quad" if value.is_empty() => parse_quad(&input, depth),
        _ => None,
    }
}

fn datatype(input: &JsObject) -> Option<NamedNode> {
    let datatype = object(input.get_named_property::<JsUnknown>("datatype").ok()?)?;
    if string(&datatype, "termType")?.as_str() != "NamedNode" {
        return None;
    }
    NamedNode::new(string(&datatype, "value")?).ok()
}

fn parse_literal(input: &JsObject, value: String) -> Option<Literal> {
    let datatype = datatype(input)?;
    let language = string(input, "language")?;
    let direction = input
        .get_named_property::<Option<JsString>>("direction")
        .ok()?
        .map(|dir| dir.into_utf16().ok()?.as_str().ok())
        .unwrap_or_else(|| Some(String::new()))?;
    if !direction.is_empty() {
        #[cfg(feature = "rdf-12")]
        {
            use triplelight::rdf::BaseDirection;
            let direction = match direction.as_str() {
                "ltr" => BaseDirection::Ltr,
                "rtl" => BaseDirection::Rtl,
                _ => return None,
            };
            return if language.is_empty() || datatype.as_str() != DIR_LANG_STRING {
                None
            } else {
                Literal::new_directional_language_tagged_literal(value, language, direction).ok()
            };
        }
        #[cfg(not(feature = "rdf-12"))]
        return None;
    }
    if !language.is_empty() {
        return if datatype.as_str() == LANG_STRING {
            Literal::new_language_tagged_literal(value, language).ok()
        } else {
            None
        };
    }
    if datatype.as_str() == LANG_STRING || datatype.as_str() == DIR_LANG_STRING {
        return None;
    }
    Some(Literal::new_typed_literal(value, datatype))
}

#[cfg(feature = "rdf-12")]
fn parse_quad(input: &JsObject, depth: usize) -> Option<Term> {
    // Match the engine's term codec bound before following a recursive object.
    const MAX_TRIPLE_TERM_DEPTH: usize = 63;
    if depth >= MAX_TRIPLE_TERM_DEPTH {
        return None;
    }
    let graph = object(input.get_named_property::<JsUnknown>("graph").ok()?)?;
    if string(&graph, "termType")? != "DefaultGraph" || !string(&graph, "value")?.is_empty() {
        return None;
    }
    let component = |name| decode(input.get_named_property::<JsUnknown>(name).ok()?, depth + 1);
    let triple = triplelight::rdf::Triple::from_terms(
        component("subject")?,
        component("predicate")?,
        component("object")?,
    )
    .ok()?;
    Some(Term::Triple(Box::new(triple)))
}
