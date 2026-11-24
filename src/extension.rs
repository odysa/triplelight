//! Registration of linked TripleLight extensions.
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};
use triplelight_core::rdf::Triple;
pub use triplelight_core::storage::EntailmentProvider;

/// An extension could not be registered or used.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExtensionError {
    Unsupported(String),
    InvalidRegistration(String),
    InvalidShapes(String),
}

impl std::fmt::Display for ExtensionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(message)
            | Self::InvalidRegistration(message)
            | Self::InvalidShapes(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ExtensionError {}

/// A registered validator returns conformance and a report graph.
pub struct Validation {
    pub conforms: bool,
    pub report: Vec<Triple>,
}

/// A validator over a data graph and a shapes graph.
pub trait Validator: Send + Sync {
    fn validate(&self, data: &[Triple], shapes: &[Triple]) -> Result<Validation, ExtensionError>;
}

/// A linked extension declares its capabilities. Registration publishes them together.
pub trait Extension: Send + Sync {
    fn name(&self) -> &str;
    fn register(&self, registrar: &mut Registrar) -> Result<(), ExtensionError>;
}

/// Stages the capabilities of one extension before registration.
#[derive(Clone, Default)]
pub struct Registrar {
    entailment: Option<Arc<dyn EntailmentProvider>>,
    validator: Option<Arc<dyn Validator>>,
}

impl Registrar {
    pub fn entailment(
        &mut self,
        provider: impl EntailmentProvider + 'static,
    ) -> Result<(), ExtensionError> {
        if self.entailment.is_some() {
            return Err(ExtensionError::InvalidRegistration(
                "extension declared two entailment providers".into(),
            ));
        }
        self.entailment = Some(Arc::new(provider));
        Ok(())
    }

    pub fn validator(&mut self, provider: impl Validator + 'static) -> Result<(), ExtensionError> {
        if self.validator.is_some() {
            return Err(ExtensionError::InvalidRegistration(
                "extension declared two validators".into(),
            ));
        }
        self.validator = Some(Arc::new(provider));
        Ok(())
    }
}

/// Availability and registration state of an extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionInfo {
    pub name: String,
    pub compiled_in: bool,
    pub loaded: bool,
}

const OFFICIAL: [(&str, bool); 2] = [
    ("rdfs", cfg!(feature = "rdfs")),
    ("shacl", cfg!(feature = "shacl")),
];

#[derive(Clone, Default)]
pub(crate) struct Registry(BTreeMap<String, Registrar>);

impl Registry {
    pub(crate) fn list(&self) -> Vec<ExtensionInfo> {
        OFFICIAL
            .iter()
            .map(|&(name, compiled_in)| ExtensionInfo {
                name: name.into(),
                compiled_in,
                loaded: self.0.contains_key(name),
            })
            .chain(
                self.0
                    .keys()
                    .filter(|name| !is_official(name))
                    .map(|name| ExtensionInfo {
                        name: name.clone(),
                        compiled_in: true,
                        loaded: true,
                    }),
            )
            .collect()
    }

    pub(crate) fn entailment(&self, name: &str) -> Option<&dyn EntailmentProvider> {
        self.0.get(name)?.entailment.as_deref()
    }

    pub(crate) fn validate(
        &self,
        name: &str,
        data: &[Triple],
        shapes: &[Triple],
    ) -> Result<Validation, ExtensionError> {
        self.0
            .get(name)
            .and_then(|entry| entry.validator.as_deref())
            .ok_or_else(|| {
                ExtensionError::Unsupported(format!("extension {name:?} has no loaded validator"))
            })?
            .validate(data, shapes)
    }
}

/// Loaded extensions for a database. Embedded frontends can share this registry.
#[derive(Clone, Default)]
pub struct ExtensionRegistry(Arc<RwLock<Arc<Registry>>>);

impl ExtensionRegistry {
    /// Lists compiled-in official extensions and current registrations.
    pub fn list(&self) -> Vec<ExtensionInfo> {
        self.snapshot().list()
    }

    pub(crate) fn snapshot(&self) -> Arc<Registry> {
        Arc::clone(&self.0.read().unwrap())
    }

    /// Loads a linked official extension by name.
    pub fn load(&self, name: &str) -> Result<(), ExtensionError> {
        if self.0.read().unwrap().0.contains_key(name) {
            return Ok(());
        }
        #[cfg(feature = "rdfs")]
        if name == "rdfs" {
            return self.publish(&RdfsExtension, true);
        }
        #[cfg(feature = "shacl")]
        if name == "shacl" {
            return self.publish(&ShaclExtension, true);
        }
        let message = if is_official(name) {
            format!("extension {name:?} requires the {name} Cargo feature")
        } else {
            format!("unknown extension {name:?}")
        };
        Err(ExtensionError::Unsupported(message))
    }

    pub(crate) fn register(&self, extension: &dyn Extension) -> Result<(), ExtensionError> {
        if is_official(extension.name()) {
            return Err(ExtensionError::InvalidRegistration(format!(
                "official extension name {:?} is reserved",
                extension.name()
            )));
        }
        self.publish(extension, false)
    }

    fn publish(&self, extension: &dyn Extension, idempotent: bool) -> Result<(), ExtensionError> {
        let name = extension.name();
        if name.is_empty() {
            return Err(ExtensionError::InvalidRegistration(
                "extension name is empty".into(),
            ));
        }
        let duplicate = || {
            if idempotent {
                Ok(())
            } else {
                Err(ExtensionError::InvalidRegistration(format!(
                    "extension {name:?} is already registered"
                )))
            }
        };
        if self.0.read().unwrap().0.contains_key(name) {
            return duplicate();
        }
        let mut stage = Registrar::default();
        extension.register(&mut stage)?;
        if stage.entailment.is_none() && stage.validator.is_none() {
            return Err(ExtensionError::InvalidRegistration(format!(
                "extension {name:?} declared no capabilities"
            )));
        }
        let mut registry = self.0.write().unwrap();
        if registry.0.contains_key(name) {
            return duplicate();
        }
        Arc::make_mut(&mut registry).0.insert(name.into(), stage);
        Ok(())
    }
}

fn is_official(name: &str) -> bool {
    OFFICIAL.iter().any(|&(official, _)| official == name)
}

#[cfg(feature = "rdfs")]
struct RdfsExtension;
#[cfg(feature = "rdfs")]
impl Extension for RdfsExtension {
    fn name(&self) -> &str {
        "rdfs"
    }
    fn register(&self, registrar: &mut Registrar) -> Result<(), ExtensionError> {
        registrar.entailment(triplelight_rdfs::Rdfs)
    }
}

#[cfg(feature = "shacl")]
struct ShaclExtension;
#[cfg(feature = "shacl")]
impl Extension for ShaclExtension {
    fn name(&self) -> &str {
        "shacl"
    }
    fn register(&self, registrar: &mut Registrar) -> Result<(), ExtensionError> {
        registrar.validator(ShaclExtension)
    }
}

#[cfg(feature = "shacl")]
impl Validator for ShaclExtension {
    fn validate(&self, data: &[Triple], shapes: &[Triple]) -> Result<Validation, ExtensionError> {
        let report = triplelight_shacl::validate(data, shapes).map_err(|error| match error {
            triplelight_shacl::ValidationError::Unsupported(message) => {
                ExtensionError::Unsupported(message)
            }
            error => ExtensionError::InvalidShapes(error.to_string()),
        })?;
        Ok(Validation {
            conforms: report.conforms,
            report: report.to_triples(),
        })
    }
}
