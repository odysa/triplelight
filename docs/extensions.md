# TripleLight extensions

TripleLight ships RDFS entailment and SHACL Core validation as official extensions. Cargo features `rdfs` and `shacl` control which are available in a build. Both are enabled by default. With `--no-default-features`, enable either feature individually.

## Load and inspect

`Database::load_extension("rdfs")` loads an extension for one database. Loading it again succeeds. `list_extensions()` reports its name, build availability, and load state. A read snapshot retains the custom registrations and load state it saw when created. It can use compiled-in official providers directly. Its status list reports their availability as well as the captured load state. For the async Rust API, use `as_sync()` to load an extension or inspect its status. Loading an unavailable official extension returns `ExtensionError::Unsupported` and names the Cargo feature.

An explicit RDFS query through `query_with_entailment` loads RDFS automatically. The ordinary query method continues to match stored triples. `validate_with_extension("shacl", &data, &shapes)` loads SHACL automatically and returns conformance and its RDF report. The CLI and standalone SHACL API select the relevant linked extension when invoked.

## Register a Rust extension

Implement `Extension::register` to declare an entailment provider, a graph validator, or both. Call `Database::register_extension` with that implementation. Registrations become visible together after the callback succeeds. Registered providers are selected by extension name through `query_with_extension` or `validate_with_extension`. Official names are reserved.

An entailment provider wraps the active graph with a read-only `Snapshot` view. The executor supplies IDs for virtual predicates that are absent from the store dictionary. Named graphs and an explicit `FROM` merge each receive their own view. A validator receives the data and shapes graphs as triples.

These extensions are linked Rust crates. Standalone shared-library installation and loading are future work.
