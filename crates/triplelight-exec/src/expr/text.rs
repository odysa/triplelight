//! SPARQL literal matching over the shared text analyzer.
use crate::model::{TermId, id_tag};
use crate::sparql::{Expression, Function};
use crate::storage::Snapshot;
use crate::{Ctx, UNBOUND};
use std::collections::{HashMap, HashSet};
use triplelight_core::text::AnalyzedText;
use triplelight_rdf::Term;
use triplelight_rdf::vocab::{rdf, xsd};

pub(crate) const MATCHES_IRI: &str = "urn:triplelight:text:matches";

fn eligible(literal: &triplelight_rdf::Literal) -> bool {
    literal.datatype() == xsd::STRING || literal.datatype() == rdf::LANG_STRING
}

pub(super) fn matches(text: &str, query: &str) -> bool {
    let normalized_query = AnalyzedText::new(query);
    matches_tokens(text, normalized_query.tokens().collect())
}

fn matches_tokens(text: &str, mut needed: HashSet<&str>) -> bool {
    if needed.is_empty() { return false; }
    let normalized_text = AnalyzedText::new(text);
    normalized_text.tokens().any(|token| {
        needed.remove(token);
        needed.is_empty()
    })
}

impl<S: Snapshot + ?Sized> Ctx<'_, S> {
    /// A positive single-variable BGP conjunct has the same row acceptance
    /// rule for false and expression errors. Composed filters stay general.
    pub(crate) fn filter_text_literals(&self, expr: &Expression, rows: &[TermId], width: usize) -> Option<Vec<TermId>> {
        let Expression::FunctionCall(Function::Custom(name), args) = expr else { return None; };
        if name.as_str() != MATCHES_IRI { return None; }
        let [Expression::Variable(variable), Expression::Literal(query)] = args.as_slice() else { return None; };
        if !eligible(query) { return None; }
        let col = *self.vars.get(variable.as_str())?;
        let analyzed = AnalyzedText::new(query.value());
        let tokens: HashSet<_> = analyzed.tokens().collect();
        if tokens.is_empty() || rows.is_empty() { return Some(Vec::new()); }

        // Up to two ID arrays (8 bytes per ID) and a varint buffer (10 bytes per ID).
        const MAX_POSTINGS: usize = 65536;
        const BYTES_PER_ID: u64 = 26;
        let limit = self.budget.map_or(MAX_POSTINGS, |budget| {
            (budget.available() / BYTES_PER_ID).min(MAX_POSTINGS as u64) as usize
        });
        let candidates = if limit == 0 {
            None
        } else {
            let _charge = match self.budget {
                Some(budget) => Some(budget.try_charge(limit as u64 * BYTES_PER_ID).ok()?),
                None => None,
            };
            let terms: Vec<_> = tokens.iter().copied().collect();
            self.snap.text_candidates(&terms, limit)
        };
        let _ids = match (self.budget, &candidates) {
            (Some(budget), Some(ids)) => Some(budget.try_charge(ids.heap_bytes()).ok()?),
            _ => None,
        };
        let mut memo = HashMap::new();
        let mut result = Vec::with_capacity(rows.len());
        for (n, row) in rows.chunks_exact(width).enumerate() {
            if n.is_multiple_of(4096) { self.check(); }
            let id = row[col];
            if id == UNBOUND { continue; }
            let indexed = candidates.as_ref().filter(|_| id_tag::of(id) == id_tag::LITERAL);
            let keep = if let Some(matches) = indexed.and_then(|ids| ids.matches(id)) {
                matches
            } else {
                *memo.entry(id).or_insert_with(|| {
                    matches!(self.resolve(id), Term::Literal(literal) if eligible(&literal)
                        && matches_tokens(literal.value(), tokens.clone()))
                })
            };
            if keep { result.extend_from_slice(row); }
        }
        Some(result)
    }
}
