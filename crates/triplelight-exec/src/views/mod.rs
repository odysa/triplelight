//! Graph views over the active RDF dataset, with optional query-time entailment.
pub(crate) mod dataset;

use crate::model::{TermId, id_tag};
use crate::storage::{DatasetView, EntailmentProvider, Snapshot, StorageError};
use std::collections::HashMap;
use triplelight_rdf::Term;

pub(crate) struct Views<'a> {
    plain: Option<&'a dyn Snapshot>,
    entailed: Option<Box<dyn Snapshot + 'a>>,
    named: Vec<(TermId, Box<dyn Snapshot + 'a>)>,
    virtual_terms: Vec<(Term, TermId)>,
}

impl<'a> Views<'a> {
    pub(crate) fn new(
        provider: Option<&dyn EntailmentProvider>,
        store: &'a dyn Snapshot,
        default: Option<&'a dyn Snapshot>,
        dataset: Option<&'a dyn DatasetView>,
        named: &[TermId],
    ) -> Result<Self, StorageError> {
        let mut views = Self {
            plain: default,
            entailed: None,
            named: Vec::new(),
            virtual_terms: Vec::new(),
        };
        let Some(provider) = provider else {
            return Ok(views);
        };

        let mut virtual_ids = Vec::new();
        for &predicate in provider.virtual_predicates() {
            let term = Term::NamedNode(predicate.into_owned());
            if store.lookup(&term)?.is_some() || virtual_ids.iter().any(|(iri, _)| *iri == predicate) {
                continue;
            }
            let id = TermId((id_tag::SCRATCH << 60) | virtual_ids.len() as u64);
            virtual_ids.push((predicate, id));
            views.virtual_terms.push((term, id));
        }

        views.entailed = Some(provider.graph(default.unwrap_or(store), &virtual_ids)?);
        if let Some(dataset) = dataset {
            for &name in named {
                if let Some(graph) = dataset.graph(name)? {
                    views.named.push((name, provider.graph(graph, &virtual_ids)?));
                }
            }
        }
        Ok(views)
    }

    pub(crate) fn default_graph(&self) -> Option<&dyn Snapshot> {
        self.entailed.as_deref().or(self.plain)
    }

    pub(crate) fn named(&self) -> HashMap<TermId, &dyn Snapshot> {
        self.named.iter().map(|(name, graph)| (*name, graph.as_ref())).collect()
    }

    /// A synthetic predicate can participate in a query even when absent from the store dictionary.
    pub(crate) fn virtual_id(&self, term: &Term) -> Option<TermId> {
        self.virtual_terms.iter().find(|(value, _)| value == term).map(|(_, id)| *id)
    }

    /// Preallocate the virtual predicate IDs before expression evaluation interns any terms.
    pub(crate) fn seeded_scratch(&self) -> (HashMap<Term, TermId>, Vec<Term>) {
        (
            self.virtual_terms.iter().cloned().collect(),
            self.virtual_terms.iter().map(|(term, _)| term.clone()).collect(),
        )
    }
}
