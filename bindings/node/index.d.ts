// TripleLight for Node.js. Terms follow the RDF/JS data model (https://rdf.js.org/data-model-spec/).

export interface NamedNode { termType: "NamedNode"; value: string }
export interface BlankNode { termType: "BlankNode"; value: string }
export interface Literal { termType: "Literal"; value: string; language: string; datatype: NamedNode }
export type Term = NamedNode | BlankNode | Literal;

/** SELECT: one object per solution; unbound variables are absent from it. */
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
export class Database {
  constructor(path: string, options?: OpenOptions);
  /** Runs a SPARQL query on the current state, off the event loop. */
  query(sparql: string): Promise<QueryResult>;
  /** Like `query`, but blocks the calling thread. */
  querySync(sparql: string): QueryResult;
  /** SPARQL 1.1 Query Results JSON as a string: the fastest path for large results. */
  queryJson(sparql: string): Promise<string>;
  /** Applies a SPARQL Update atomically and durably; resolves to the number of triples changed. */
  update(sparql: string): Promise<number>;
  /** Atomically imports an N-Triples (.nt) or Turtle (.ttl) file; resolves to the number of triples added. */
  load(path: string): Promise<number>;
  /** Merges updates into a new immutable base; open snapshots keep their view. */
  compact(): Promise<void>;
  /** True once background optimizer statistics are built (queries work before that, with simpler plans). */
  statsReady(): boolean;
  /** A consistent view for several queries; later updates do not change it. */
  read(): Snapshot;
  /** Releases this handle; the directory lock is released once no snapshot or pending operation uses it. */
  close(): void;
}

export class Snapshot {
  query(sparql: string): Promise<QueryResult>;
  querySync(sparql: string): QueryResult;
  queryJson(sparql: string): Promise<string>;
}

export class TripleLightError extends Error { kind: "SyntaxError" | "UnsupportedError" | "StorageError" | "LockedError" }
export class SyntaxError extends TripleLightError {}
export class UnsupportedError extends TripleLightError {}
export class StorageError extends TripleLightError {}
export class LockedError extends TripleLightError {}
