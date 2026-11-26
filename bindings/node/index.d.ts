// TripleLight for Node.js. Terms are plain objects shaped like the RDF/JS data model
// (https://rdf.js.org/data-model-spec/): termType, value, language, datatype. They have no equals() method.

export interface NamedNode { termType: "NamedNode"; value: string }
export interface BlankNode { termType: "BlankNode"; value: string }
export interface Literal { termType: "Literal"; value: string; language: string; datatype: NamedNode; direction?: "ltr" | "rtl" } // present only on a directional literal (rdf-12 build)
/** RDF 1.2 triple term (only with the `rdf-12` build): an RDF/JS Quad in the default graph; `value` is "". */
export interface Quad { termType: "Quad"; value: ""; subject: NamedNode | BlankNode | Quad; predicate: NamedNode; object: Term; graph: { termType: "DefaultGraph"; value: "" } }
export type Term = NamedNode | BlankNode | Literal | Quad;

/** SELECT: one object per solution (with a null prototype); unbound variables are absent from it. */
export interface BindingsResult { type: "bindings"; variables: string[]; bindings: Array<Record<string, Term>> }
/** ASK. */
export interface BooleanResult { type: "boolean"; boolean: boolean }
/** CONSTRUCT and DESCRIBE. */
export interface TriplesResult {
  type: "triples";
  triples: Array<{ subject: NamedNode | BlankNode; predicate: NamedNode; object: Term }>;
}
export type QueryResult = BindingsResult | BooleanResult | TriplesResult;

export interface OpenOptions {
  /** An N-Triples file loaded only when the directory holds no database yet. */
  initial?: string;
}

/** A persistent RDF database in a local directory (created if missing). One process may open a directory at a time. */
/** `entailment: "rdfs"` matches the default graph's RDFS closure (subclasses, subproperties, domain and range),
 * computed at query time; nothing inferred is stored. The default, "simple", matches the stored triples. */
export interface QueryOptions {
  entailment?: "simple" | "rdfs";
  /** Stop the query after this many milliseconds with an `InterruptedError` (`timedOut: true`). */
  timeoutMs?: number;
  /** Aborting the signal stops a running query with an `InterruptedError` (`timedOut: false`). */
  signal?: AbortSignal;
  /** Working-memory budget in megabytes; a query that needs more fails with `ResourceLimitError`. */
  memoryMb?: number;
  /** With `memoryMb`: a GROUP BY whose input does not fit spills it to `TRIPLELIGHT_SCRATCH_DIR` (else the system
   * temporary directory) and finishes. The default, `false`, never writes to disk. */
  spill?: boolean;
}

export interface ExtensionInfo { name: string; compiledIn: boolean; loaded: boolean }

export class Database {
  /** Opens on the calling thread; prefer `Database.open` in servers. */
  constructor(path: string, options?: OpenOptions);
  /** Opens on a worker thread (recovery and the initial load can take a while). */
  static open(path: string, options?: OpenOptions): Promise<Database>;
  /** Creates a non-durable in-memory database. */
  static inMemory(): Database;
  /** Runs a SPARQL query on the current state. Evaluation runs on a worker thread; converting the result to JS
   * objects happens on the event loop when the Promise resolves (use `queryJson` for very large results). */
  query(sparql: string, options?: QueryOptions): Promise<QueryResult>;
  /** Like `query`, but blocks the calling thread. */
  querySync(sparql: string, options?: QueryOptions): QueryResult;
  /** SPARQL 1.1 Query Results JSON as a string: the fastest path for large results. */
  queryJson(sparql: string, options?: QueryOptions): Promise<string>;
  /** Applies a SPARQL Update atomically and durably; resolves to the number of triples changed. */
  update(sparql: string): Promise<number>;
  /** Atomically imports an N-Triples (.nt), Turtle (.ttl), N-Quads (.nq) or TriG (.trig) file; resolves to the number of triples added. */
  load(path: string): Promise<number>;
  /** Merges updates into a new immutable base; open snapshots keep their view. */
  compact(): Promise<void>;
  /** True once background optimizer statistics are built (queries work before that, with simpler plans). */
  statsReady(): boolean;
  loadExtension(name: string): void;
  /** Registers a synchronous callback under an absolute IRI for future queries and snapshots. */
  registerFunction(iri: string, callback: (...args: Term[]) => Term | null, arity?: number, pure?: boolean): void;
  listExtensions(): ExtensionInfo[];
  /** A consistent view for several queries; later updates do not change it. */
  read(): Snapshot;
  /** Releases this handle. The directory lock is held until every snapshot and pending operation is gone. */
  close(): void;
}

export class Snapshot {
  query(sparql: string, options?: QueryOptions): Promise<QueryResult>;
  querySync(sparql: string, options?: QueryOptions): QueryResult;
  queryJson(sparql: string, options?: QueryOptions): Promise<string>;
}

/** A SHACL validation report: `conforms`, and the report graph (`sh:ValidationReport` and its results) as triples. */
export interface ValidationReport {
  conforms: boolean;
  triples: Array<{ subject: NamedNode | BlankNode; predicate: NamedNode; object: Term }>;
}
export interface ValidateOptions { dataFormat?: "turtle" | "ntriples"; shapesFormat?: "turtle" | "ntriples" }
/** SHACL Core validation of a data document against a shapes document. Violations are a report (`conforms` false); a
 * document that does not parse throws `SyntaxError`, shapes SHACL rejects throw `ShapesError`. */
export function validate(data: string, shapes: string, options?: ValidateOptions): ValidationReport;

export class TripleLightError extends Error { kind: "SyntaxError" | "UnsupportedError" | "StorageError" | "LockedError" | "GraphStateError" | "ShapesError" | "InterruptedError" | "ResourceLimitError" }
export class InterruptedError extends TripleLightError { timedOut: boolean }
export class ResourceLimitError extends TripleLightError {}
export class ShapesError extends TripleLightError {}
export class SyntaxError extends TripleLightError {}
export class UnsupportedError extends TripleLightError {}
export class GraphStateError extends TripleLightError {}
export class StorageError extends TripleLightError {}
export class LockedError extends TripleLightError {}
