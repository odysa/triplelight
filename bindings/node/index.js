"use strict";
// TripleLight for Node.js: an embedded RDF database with SPARQL 1.1. The native addon does the work on worker threads;
// this file turns its errors into typed exceptions.
const native = loadNative();

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
const KINDS = { SyntaxError: SyntaxError_, UnsupportedError, StorageError, LockedError, GraphStateError };

function wrapError(e) {
  const m = /^(SyntaxError|UnsupportedError|StorageError|LockedError|GraphStateError): ([\s\S]*)$/.exec(e && e.message);
  if (!m) return e;
  const err = new KINDS[m[1]](m[2], m[1]);
  err.cause = e;
  return err;
}
const sync = (f) => { try { return f(); } catch (e) { throw wrapError(e); } };
const async_ = (f) => { try { return f().catch((e) => { throw wrapError(e); }); } catch (e) { return Promise.reject(wrapError(e)); } };

class Snapshot {
  constructor(raw) { this._raw = raw; }
  query(sparql, options = {}) { return async_(() => this._raw.query(sparql, options.entailment ?? null)); }
  querySync(sparql, options = {}) { return sync(() => this._raw.querySync(sparql, options.entailment ?? null)); }
  queryJson(sparql, options = {}) { return async_(() => this._raw.queryJson(sparql, options.entailment ?? null)); }
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
  query(sparql, options = {}) { return async_(() => this._raw.query(sparql, options.entailment ?? null)); }
  querySync(sparql, options = {}) { return sync(() => this._raw.querySync(sparql, options.entailment ?? null)); }
  queryJson(sparql, options = {}) { return async_(() => this._raw.queryJson(sparql, options.entailment ?? null)); }
  update(sparql) { return async_(() => this._raw.update(sparql)); }
  load(path) { return async_(() => this._raw.load(String(path))); }
  compact() { return async_(() => this._raw.compact()).then(() => undefined); }
  statsReady() { return sync(() => this._raw.statsReady()); }
  read() { return new Snapshot(sync(() => this._raw.read())); }
  close() { this._raw.close(); }
}

module.exports = { Database, Snapshot, TripleLightError, SyntaxError: SyntaxError_, UnsupportedError, StorageError, LockedError, GraphStateError };
