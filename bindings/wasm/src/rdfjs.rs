use js_sys::{Object, Reflect};
use triplelight::rdf::{BlankNode, Literal, NamedNode, Term};
use wasm_bindgen::JsValue;

const LANG_STRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString";
const DIR_LANG_STRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#dirLangString";

pub(super) fn set(o: &Object, k: &str, v: impl Into<JsValue>) {
    let _ = Reflect::set(o, &k.into(), &v.into());
}

pub(super) fn named(iri: &str) -> Object {
    let o = Object::new();
    set(&o, "termType", "NamedNode");
    set(&o, "value", iri);
    o
}

fn literal(l: &Literal) -> Object {
    let o = Object::new();
    set(&o, "termType", "Literal");
    set(&o, "value", l.value());
    set(&o, "language", l.language().unwrap_or(""));
    set(&o, "datatype", named(l.datatype().as_str()));
    o
}

pub(super) fn term(t: &Term) -> Object {
    match t {
        Term::NamedNode(n) => named(n.as_str()),
        Term::BlankNode(b) => {
            let o = Object::new();
            set(&o, "termType", "BlankNode");
            set(&o, "value", b.as_str());
            o
        }
        Term::Literal(l) => literal(l),
        #[allow(unreachable_patterns)]
        _ => Object::new(),
    }
}

fn property(term: &JsValue, key: &str) -> Option<JsValue> {
    Reflect::get(term, &JsValue::from_str(key)).ok()
}

fn parse_named(term: &JsValue) -> Option<NamedNode> {
    if property(term, "termType")?.as_string()?.as_str() != "NamedNode" {
        return None;
    }
    NamedNode::new(property(term, "value")?.as_string()?).ok()
}

pub(super) fn parse_term(term: &JsValue) -> Option<Term> {
    let value = property(term, "value")?.as_string()?;
    match property(term, "termType")?.as_string()?.as_str() {
        "NamedNode" => NamedNode::new(value).ok().map(Term::from),
        "BlankNode" => BlankNode::new(value).ok().map(Term::from),
        "Literal" => {
            let language = property(term, "language")?.as_string()?;
            let direction = property(term, "direction")?;
            if !direction.is_undefined() && direction.as_string()?.as_str() != "" {
                return None;
            }
            let datatype = parse_named(&property(term, "datatype")?)?;
            if !language.is_empty() {
                return if datatype.as_str() == LANG_STRING {
                    Literal::new_language_tagged_literal(value, language)
                        .ok()
                        .map(Term::from)
                } else {
                    None
                };
            }
            if datatype.as_str() == LANG_STRING || datatype.as_str() == DIR_LANG_STRING {
                return None;
            }
            Some(Literal::new_typed_literal(value, datatype).into())
        }
        _ => None,
    }
}
