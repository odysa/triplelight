"use strict";
// TripleLight for Node.js: an embedded RDF database with SPARQL 1.1. The native addon does the work on worker threads;
// this file turns its errors into typed exceptions.
const native = loadNative();
const { isPromise } = require("node:util").types;
const promiseThen = Promise.prototype.then;
const ignoreRejection = () => {};

function scalarCallback(callback) {
  if (typeof callback !== "function") throw new TypeError("scalar callback must be a function");
  return (...args) => {
    const value = callback(...args);
    if (!isPromise(value)) return value;
    Reflect.apply(promiseThen, value, [undefined, ignoreRejection]);
    return null;
  };
}

// The published package carries one prebuilt addon per platform under prebuilds/<platform>-<arch>/. A local build
// (`npm run build`) puts the addon at ./triplelight.node, which is used when there is no prebuild (development).
function loadNative() {
  const target = `${process.platform}-${process.arch}`;
  const candidates = [`./prebuilds/${target}/triplelight.node`, "./triplelight.node"];
  let failure = null;
  for (const candidate of candidates) {
    try {
      return require(candidate);
    } catch (e) {
      if (e && e.code !== "MODULE_NOT_FOUND") failure = e; // found but would not load (for example an older glibc)
    }
  }
  if (failure) throw failure;
  throw new Error(`TripleLight has no native addon for ${target}. Supported: darwin-arm64, linux-x64 and linux-arm64 (glibc 2.31 or newer).`);
}

class TripleLightError extends Error {
  constructor(message, kind) { super(message); this.name = kind || "TripleLightError"; this.kind = kind; }
}
class SyntaxError_ extends TripleLightError {}
class UnsupportedError extends TripleLightError {}
class StorageError extends TripleLightError {}
class LockedError extends TripleLightError {}
class GraphStateError extends TripleLightError {}
class ShapesError extends TripleLightError {}
/** A query stopped by its `signal` or its `timeoutMs`; `timedOut` tells which. */
class InterruptedError extends TripleLightError {
  constructor(message, kind) { super(message, kind); this.timedOut = message === "query timed out"; }
}
/** A query that needed more working memory than its `memoryMb`. */
class ResourceLimitError extends TripleLightError {}
const KINDS = { SyntaxError: SyntaxError_, UnsupportedError, StorageError, LockedError, GraphStateError, ShapesError, InterruptedError, ResourceLimitError };

function wrapError(e) {
  const m = /^(SyntaxError|UnsupportedError|StorageError|LockedError|GraphStateError|ShapesError|InterruptedError|ResourceLimitError): ([\s\S]*)$/.exec(e && e.message);
  if (!m) return e;
  const err = new KINDS[m[1]](m[2], m[1]);
  err.cause = e;
  return err;
}

// The native limit arguments of a query's options (#110): `timeoutMs` and `signal` share one interrupt, which an
// aborted signal cancels; `memoryMb` is a working-memory budget, and `spill` lets a GROUP BY over it spill to disk.
// A signal aborted already cancels at once.
function limits(options) {
  let interrupt = null;
  if (options.timeoutMs != null || options.signal != null) {
    interrupt = new native.Interrupt(options.timeoutMs ?? null);
    const signal = options.signal;
    if (signal) {
      if (signal.aborted) interrupt.cancel();
      else signal.addEventListener("abort", () => interrupt.cancel(), { once: true });
    }
  }
  return [options.entailment ?? null, interrupt, options.memoryMb ?? null, options.spill ?? null];
}
const sync = (f) => { try { return f(); } catch (e) { throw wrapError(e); } };
const async_ = (f) => { try { return f().catch((e) => { throw wrapError(e); }); } catch (e) { return Promise.reject(wrapError(e)); } };

class Snapshot {
  constructor(raw) { this._raw = raw; }
  query(sparql, options = {}) { return async_(() => this._raw.query(sparql, ...limits(options))); }
  querySync(sparql, options = {}) { return sync(() => this._raw.querySync(sparql, ...limits(options))); }
  queryJson(sparql, options = {}) { return async_(() => this._raw.queryJson(sparql, ...limits(options))); }
}

class Database {
  constructor(path, options = {}) {
    // `Database.open` passes an already-opened native handle.
    this._raw = path instanceof native.Database ? path : sync(() => new native.Database(String(path), options.initial ?? null));
  }
  /** Opens the database on a worker thread instead of the calling thread. */
  static open(path, options = {}) {
    return async_(() => native.Database.open(String(path), options.initial ?? null)).then((raw) => new Database(raw));
  }
  /** Creates a non-durable in-memory database. */
  static inMemory() { return new Database(sync(() => native.Database.inMemory())); }
  query(sparql, options = {}) { return async_(() => this._raw.query(sparql, ...limits(options))); }
  querySync(sparql, options = {}) { return sync(() => this._raw.querySync(sparql, ...limits(options))); }
  queryJson(sparql, options = {}) { return async_(() => this._raw.queryJson(sparql, ...limits(options))); }
  update(sparql) { return async_(() => this._raw.update(sparql)); }
  load(path) { return async_(() => this._raw.load(String(path))); }
  compact() { return async_(() => this._raw.compact()).then(() => undefined); }
  loadExtension(name) { return sync(() => this._raw.loadExtension(name)); }
  registerFunction(iri, callback, arity, pure) {
    return sync(() => this._raw.registerFunction(iri, scalarCallback(callback), arity, pure));
  }
  listExtensions() { return sync(() => this._raw.listExtensions()); }
  statsReady() { return sync(() => this._raw.statsReady()); }
  read() { return new Snapshot(sync(() => this._raw.read())); }
  close() { this._raw.close(); }
}

/** SHACL Core validation of a data document against a shapes document (`options.dataFormat` / `options.shapesFormat`:
 * "turtle", the default, or "ntriples"). Returns `{ conforms, triples }`. */
function validate(data, shapes, options = {}) {
  return sync(() => native.validate(String(data), String(shapes), options.dataFormat ?? null, options.shapesFormat ?? null));
}

module.exports = { Database, Snapshot, validate, TripleLightError, SyntaxError: SyntaxError_, UnsupportedError, StorageError, LockedError, GraphStateError, ShapesError, InterruptedError, ResourceLimitError };
