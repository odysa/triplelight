//! The dataset views a query reads besides its store, including graph-local entailment when enabled.
pub(crate) mod dataset;
#[cfg(feature = "rdfs")]
mod entail;

use crate::model::TermId;
use crate::storage::{DatasetView, Snapshot, StorageError};
#[cfg(feature = "rdfs")]
use entail::Entailed;
use std::collections::HashMap;

/// The active default graph and any graph-local RDFS views for one query.
pub(crate) struct Views<'a> {
    plain: Option<&'a dyn Snapshot>,
    #[cfg(feature = "rdfs")]
    default: Option<Entailed<'a>>,
    #[cfg(feature = "rdfs")]
    named: Vec<(TermId, Entailed<'a>)>,
}

impl<'a> Views<'a> {
    pub(crate) fn new(
        rdfs: bool,
        store: &'a dyn Snapshot,
        default: Option<&'a dyn Snapshot>,
        dataset: Option<&'a dyn DatasetView>,
        named: &[TermId],
    ) -> Result<Self, StorageError> {
        #[cfg(feature = "rdfs")]
        if rdfs {
            let mut views = Vec::new();
            if let Some(dataset) = dataset {
                for &graph in named {
                    if let Some(view) = dataset.graph(graph)? {
                        views.push((graph, Entailed::new(view)?));
                    }
                }
            }
            return Ok(Self {
                plain: default,
                default: Some(Entailed::new(default.unwrap_or(store))?),
                named: views,
            });
        }
        #[cfg(not(feature = "rdfs"))]
        let _ = (rdfs, store, dataset, named);
        Ok(Self {
            plain: default,
            #[cfg(feature = "rdfs")]
            default: None,
            #[cfg(feature = "rdfs")]
            named: Vec::new(),
        })
    }

    /// The active default graph if it is not the store's own.
    pub(crate) fn default_graph(&self) -> Option<&dyn Snapshot> {
        #[cfg(feature = "rdfs")]
        if let Some(view) = &self.default {
            return Some(view);
        }
        self.plain
    }

    pub(crate) fn named(&self) -> HashMap<TermId, &dyn Snapshot> {
        #[cfg(feature = "rdfs")]
        {
            self.named
                .iter()
                .map(|(graph, view)| (*graph, view as &dyn Snapshot))
                .collect()
        }
        #[cfg(not(feature = "rdfs"))]
        {
            HashMap::new()
        }
    }

    pub(crate) fn entailing(&self) -> bool {
        #[cfg(feature = "rdfs")]
        {
            self.default.is_some()
        }
        #[cfg(not(feature = "rdfs"))]
        {
            false
        }
    }

    /// Names `rdf:type` in the entailed views when the store has no such ID.
    pub(crate) fn set_rdf_type(&self, id: TermId) {
        #[cfg(feature = "rdfs")]
        for view in self
            .default
            .iter()
            .chain(self.named.iter().map(|(_, view)| view))
        {
            view.set_rdf_type(id);
        }
        #[cfg(not(feature = "rdfs"))]
        let _ = id;
    }
}
