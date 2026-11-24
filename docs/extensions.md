# TripleLight extensions

TripleLight ships RDFS entailment and SHACL Core validation as official extensions. Cargo features `rdfs` and `shacl` control which are available in a build. Both are enabled by default. With `--no-default-features`, enable either feature individually.

## Load and inspect

`Database::load_extension("rdfs")` loads an extension for one database. Loading it again succeeds. `list_extensions()` reports its name, build availability, and load state. A read snapshot retains the custom registrations and load state it saw when created. It can use compiled-in official providers directly. Its status list reports their availability as well as the captured load state. For the async Rust API, use `as_sync()` to load an extension or inspect its status. Loading an unavailable official extension returns `ExtensionError::Unsupported` and names the Cargo feature.

An explicit RDFS query through `query_with_entailment` loads RDFS automatically. The ordinary query method continues to match stored triples. `validate_with_extension("shacl", &data, &shapes)` loads SHACL automatically and returns conformance and its RDF report. The CLI and standalone SHACL API select the relevant linked extension when invoked.

## Register a Rust extension

Implement `Extension::register` to declare an entailment provider, a graph validator, and any scalar functions it supplies. Call `Database::register_extension` with that implementation. Registrations become visible together after the callback succeeds. Registered providers are selected by extension name through `query_with_extension` or `validate_with_extension`. Official names are reserved.

An entailment provider wraps the active graph with a read-only `Snapshot` view. The executor supplies IDs for virtual predicates that are absent from the store dictionary. Named graphs and an explicit `FROM` merge each receive their own view. A validator receives the data and shapes graphs as triples.

## Register a scalar function in Rust

`Database::register_function(iri, options, callback)` registers a Rust callback
under an absolute IRI. Extensions use `Registrar::function` with the same
arguments. An existing registration or built-in function reserves its IRI.
New read snapshots see a successful registration.

```rust
use triplelight::{embedded::Database, extension::FunctionOptions, rdf::Term};

let db = Database::in_memory();
db.register_function(
    "urn:example:identity",
    FunctionOptions { arity: Some(1), pure: true },
    |args: &[Term]| args.first().cloned(),
)?;
```

Call it as `<urn:example:identity>(?value)` in a query. The callback receives
RDF terms. Returning `None` or unwinding produces an expression error, so an
errored FILTER rejects the solution and an errored BIND leaves its target
unbound. Registered functions may run on an executor thread. Keep callbacks
synchronous and avoid changing or reentering the database during evaluation.
A running callback must return before query cancellation or a deadline can take
effect. Rust's panic hook still runs when a callback unwinds.

The registry applies to queries, including correlated EXISTS. SPARQL UPDATE
uses its own evaluation path. [EXPLAIN](explain.md) runs the query, so it can
invoke registered callbacks.

Only mark a function pure when its result depends solely on the supplied terms.
Purity enables a per-query call cache. The planner still treats registered calls
as opaque. A cache entry requires an existing ID for every evaluated argument,
and the memory charge includes an estimate based on the result's RDF spelling.

These extensions are linked Rust crates. Standalone shared-library installation and loading are future work.

## Bindings

| Binding | Load | List |
| --- | --- | --- |
| Python | `db.load_extension(name)` | `db.list_extensions()` |
| Node.js and WebAssembly | `db.loadExtension(name)` | `db.listExtensions()` |
| Go | `db.LoadExtension(name)` | `db.ListExtensions()` |
| Java | `db.loadExtension(name)` | `db.listExtensions()` |
| C | `tl_load_extension` | `tl_list_extensions` |

The list reports the extension name and two flags for build availability and load state. The C call returns columns `name`, `compiled_in`, and `loaded` in an owned result. Free it with `tl_result_free`.
