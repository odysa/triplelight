//! Bind one value into each solution, preserving compatible outer bindings.
use std::collections::HashMap;
use triplelight_core::model::{TermId, id_tag};

pub const UNBOUND: TermId = TermId(0);
pub type Row = Vec<TermId>;

pub(crate) fn extend(
    rows: Vec<Row>,
    target: usize,
    source: Option<usize>,
    evaluate: impl Fn(&Row) -> Option<TermId>,
    canonicalize: impl Fn(TermId) -> TermId,
    check: impl Fn(),
) -> Vec<Row> {
    let mut alias_ids = HashMap::new();
    rows.into_iter()
        .enumerate()
        .filter_map(|(i, mut row)| {
            if i.is_multiple_of(4096) {
                check();
            }
            let value = match source {
                Some(column) => row
                    .get(column)
                    .copied()
                    .filter(|&id| id != UNBOUND)
                    .map(|id| {
                        // Scratch IDs are already interned. For other IDs, keep the resolve-and-intern
                        // check and memoize repeated values up to a bounded number of entries.
                        if id.0 >> 60 == id_tag::SCRATCH {
                            return id;
                        }
                        if let Some(&value) = alias_ids.get(&id) {
                            return value;
                        }
                        let value = canonicalize(id);
                        if alias_ids.len() < 256 {
                            alias_ids.insert(id, value);
                        }
                        value
                    }),
                None => evaluate(&row),
            };
            // A new binding must agree with the target if it already has a value.
            // An expression error leaves the previous binding in place.
            if let Some(value) = value {
                if row[target] == UNBOUND {
                    row[target] = value;
                } else if row[target] != value {
                    return None;
                }
            }
            Some(row)
        })
        .collect()
}
