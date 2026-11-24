# TripleLight extensions

TripleLight ships RDFS entailment and SHACL Core validation as official extensions. Cargo features `rdfs` and `shacl` control which are available in a build. Both are enabled by default. With `--no-default-features`, enable either feature individually.

## Load and inspect

`Database::load_extension("rdfs")` loads an extension for one database. Loading it again succeeds. `list_extensions()` reports its name, build availability, and load state. A read snapshot retains the custom registrations and load state it saw when created. It can use compiled-in official providers directly. Its status list reports their availability as well as the captured load state. For the async Rust API, use `as_sync()` to load an extension or inspect its status. Loading an unavailable official extension returns `ExtensionError::Unsupported` and names the Cargo feature.

An explicit RDFS query through `query_with_entailment` loads RDFS automatically. The ordinary query method continues to match stored triples. `validate_with_extension("shacl", &data, &shapes)` loads SHACL automatically and returns conformance and its RDF report. The CLI and standalone SHACL API select the relevant linked extension when invoked.

## Register a Rust extension

Implement `Extension::register` to declare an entailment provider, a graph validator, or both. Call `Database::register_extension` with that implementation. Registrations become visible together after the callback succeeds. Registered providers are selected by extension name through `query_with_extension` or `validate_with_extension`. Official names are reserved.

An entailment provider wraps the active graph with a read-only `Snapshot` view. The executor supplies IDs for virtual predicates that are absent from the store dictionary. Named graphs and an explicit `FROM` merge each receive their own view. A validator receives the data and shapes graphs as triples.

These extensions are linked Rust crates. Standalone shared-library installation and loading are future work.

### OWL 2 RL equivalence

With the `rdfs` feature enabled, a Rust application can register an entailment
provider for an OWL 2 RL subset.

```rust
use triplelight::embedded::Database;
use triplelight::extension::OwlRlEquivalence;

let db = Database::in_memory();
db.register_extension(&OwlRlEquivalence)?;
db.update(
    "INSERT DATA {
        <urn:Novel> <http://www.w3.org/2002/07/owl#equivalentClass> <urn:Book> .
        <urn:story> a <urn:Novel> .
    }",
)?;
let result = db.query_with_extension("ASK { <urn:story> a <urn:Book> }", "owl-rl-equivalence")?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

The provider adds both directions of the corresponding RDFS subclass or
subproperty relation for asserted `owl:equivalentClass` and
`owl:equivalentProperty` statements between IRIs. Property equivalences
involving `rdf:type`, the RDFS predicates used by the view for schema rules,
or either OWL equivalence predicate are ignored. The resulting RDFS
entailments are scoped to each active graph, including a merged default graph
selected by `FROM`. They exist only in the query view.

## Bindings

| Binding | Load | List |
| --- | --- | --- |
| Python | `db.load_extension(name)` | `db.list_extensions()` |
| Node.js and WebAssembly | `db.loadExtension(name)` | `db.listExtensions()` |
| Go | `db.LoadExtension(name)` | `db.ListExtensions()` |
| Java | `db.loadExtension(name)` | `db.listExtensions()` |
| C | `tl_load_extension` | `tl_list_extensions` |

The list reports the extension name and two flags for build availability and load state. The C call returns columns `name`, `compiled_in`, and `loaded` in an owned result. Free it with `tl_result_free`.
