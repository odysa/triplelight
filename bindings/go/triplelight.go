// Package triplelight is the Go binding for the embedded TripleLight RDF database.
//
// It links the static C ABI library built from ../c (`cargo build --release` there). A *DB is safe for concurrent
// use by multiple goroutines; Close must not race with other calls on the same DB.
package triplelight

/*
#cgo CFLAGS: -I${SRCDIR}/../c/include
#cgo LDFLAGS: ${SRCDIR}/../c/target/release/libtriplelight_c.a
#cgo linux LDFLAGS: -lm -ldl -lpthread
#cgo darwin LDFLAGS: -framework CoreFoundation -framework Security
#include <stdlib.h>
#include "triplelight.h"
*/
import "C"

import (
	"errors"
	"fmt"
	"runtime"
	"sync"
	"unsafe"
)

// ErrorKind classifies a TripleLight error.
type ErrorKind int

const (
	ErrSyntax          ErrorKind = C.TL_ERR_SYNTAX
	ErrUnsupported     ErrorKind = C.TL_ERR_UNSUPPORTED
	ErrStorage         ErrorKind = C.TL_ERR_STORAGE
	ErrLocked          ErrorKind = C.TL_ERR_LOCKED
	ErrInvalidArgument ErrorKind = C.TL_ERR_INVALID_ARGUMENT
	ErrInternal        ErrorKind = C.TL_ERR_INTERNAL
)

func (k ErrorKind) String() string {
	switch k {
	case ErrSyntax:
		return "syntax"
	case ErrUnsupported:
		return "unsupported"
	case ErrStorage:
		return "storage"
	case ErrLocked:
		return "locked"
	case ErrInvalidArgument:
		return "invalid argument"
	case ErrInternal:
		return "internal"
	}
	return fmt.Sprintf("error(%d)", int(k))
}

// Error is returned by every failing call.
type Error struct {
	Kind    ErrorKind
	Message string
}

func (e *Error) Error() string { return "triplelight: " + e.Kind.String() + ": " + e.Message }

// ErrClosed is returned when a closed DB is used.
var ErrClosed = errors.New("triplelight: database is closed")

func check(code C.int32_t, e *C.tl_error) error {
	if code == C.TL_OK {
		return nil
	}
	err := &Error{Kind: ErrorKind(code)}
	if e != nil {
		var n C.size_t
		p := C.tl_error_message(e, &n)
		err.Message = C.GoStringN((*C.char)(unsafe.Pointer(p)), C.int(n))
		C.tl_error_free(e)
	}
	return err
}

// cstr passes a Go string without copying; the C side never retains it.
func cstr(s string) (*C.char, C.size_t) {
	if len(s) == 0 {
		return nil, 0
	}
	return (*C.char)(unsafe.Pointer(unsafe.StringData(s))), C.size_t(len(s))
}

func init() {
	if v := C.tl_abi_version(); v != C.TL_ABI_VERSION {
		panic(fmt.Sprintf("triplelight: C ABI version %d, binding expects %d", v, C.TL_ABI_VERSION))
	}
}

// DB is an open TripleLight database. Calls hold a read lock and Close takes the write lock, so Close waits for
// in-flight calls and later calls return ErrClosed instead of touching a freed handle.
type DB struct {
	mu sync.RWMutex
	h  *C.tl_db
}

// with runs f on the live handle under the read lock.
func (d *DB) with(f func(h *C.tl_db) error) error {
	d.mu.RLock()
	defer d.mu.RUnlock()
	if d.h == nil {
		return ErrClosed
	}
	return f(d.h)
}

// Open opens (or creates) a persistent database in dir.
func Open(dir string) (*DB, error) {
	var h *C.tl_db
	var e *C.tl_error
	p, n := cstr(dir)
	err := check(C.tl_open(p, n, &h, &e), e)
	runtime.KeepAlive(dir)
	if err != nil {
		return nil, err
	}
	return newDB(h), nil
}

// OpenInMemory creates an isolated, non-durable in-memory database.
func OpenInMemory() (*DB, error) {
	var h *C.tl_db
	var e *C.tl_error
	if err := check(C.tl_open_in_memory(&h, &e), e); err != nil {
		return nil, err
	}
	return newDB(h), nil
}

func newDB(h *C.tl_db) *DB {
	d := &DB{h: h}
	runtime.SetFinalizer(d, func(d *DB) { d.Close() })
	return d
}

// Close releases the database (and, for a persistent database, its directory lock). Further calls return ErrClosed.
func (d *DB) Close() error {
	d.mu.Lock()
	defer d.mu.Unlock()
	if d.h != nil {
		C.tl_close(d.h)
		d.h = nil
	}
	return nil
}

// Update applies a SPARQL Update and returns the number of triples inserted or deleted.
func (d *DB) Update(sparql string) (uint64, error) {
	var n C.uint64_t
	err := d.with(func(h *C.tl_db) error {
		var e *C.tl_error
		p, l := cstr(sparql)
		defer runtime.KeepAlive(sparql)
		return check(C.tl_update(h, p, l, &n, &e), e)
	})
	return uint64(n), err
}

// Load imports an N-Triples (.nt) or Turtle (.ttl) file and returns the number of triples added.
func (d *DB) Load(path string) (uint64, error) {
	var n C.uint64_t
	err := d.with(func(h *C.tl_db) error {
		var e *C.tl_error
		p, l := cstr(path)
		defer runtime.KeepAlive(path)
		return check(C.tl_load(h, p, l, &n, &e), e)
	})
	return uint64(n), err
}

// Compact rewrites the current state into a new base (persistent) or rebuilds the indexes (in-memory).
func (d *DB) Compact() error {
	return d.with(func(h *C.tl_db) error {
		var e *C.tl_error
		return check(C.tl_compact(h, &e), e)
	})
}

// StatsReady reports whether planner statistics for the current base are ready; queries may run either way.
func (d *DB) StatsReady() bool {
	ready := false
	_ = d.with(func(h *C.tl_db) error { ready = C.tl_stats_ready(h) == 1; return nil })
	return ready
}

// QueryArrow runs a SELECT query and exports its solutions as an Arrow C Stream into out, which must point to a
// caller-owned struct ArrowArrayStream (for example arrow-go's cdata.CArrowArrayStream). On success
// the caller owns the stream and must release it; it stays valid after Close. On failure out is left untouched.
// Each variable is a nullable struct column {kind, value, datatype, language}; a null struct is an unbound cell.
// ASK and CONSTRUCT queries return ErrUnsupported.
func (d *DB) QueryArrow(sparql string, out unsafe.Pointer) error {
	return d.with(func(h *C.tl_db) error {
		var e *C.tl_error
		p, l := cstr(sparql)
		defer runtime.KeepAlive(sparql)
		return check(C.tl_query_arrow(h, p, l, (*C.struct_ArrowArrayStream)(out), &e), e)
	})
}

// TermKind is the kind of an RDF term.
type TermKind uint8

const (
	IRI       TermKind = C.TL_TERM_IRI
	BlankNode TermKind = C.TL_TERM_BLANK
	Literal   TermKind = C.TL_TERM_LITERAL
)

// Term is an RDF term. For literals Datatype is always set and Language is empty unless language-tagged.
type Term struct {
	Kind     TermKind
	Value    string
	Language string
	Datatype string
}

// ResultKind says which fields of a Result are set.
type ResultKind int

const (
	Bindings ResultKind = iota // Vars and Rows
	Boolean                    // Bool
	Triples                    // Rows of [subject, predicate, object]
)

// Result is a fully decoded query result. Rows share *Term values: each distinct term is decoded once. A nil cell
// is an unbound variable.
type Result struct {
	Kind ResultKind
	Vars []string
	Rows [][]*Term
	Bool bool
}

// Query runs a SPARQL SELECT, ASK or CONSTRUCT query.
func (d *DB) Query(sparql string) (*Result, error) {
	var r *C.tl_result
	err := d.with(func(h *C.tl_db) error {
		var e *C.tl_error
		p, l := cstr(sparql)
		defer runtime.KeepAlive(sparql)
		return check(C.tl_query(h, p, l, &r, &e), e)
	})
	if err != nil {
		return nil, err
	}
	// The result is independent of the database; decode copies everything out, then frees the native buffer.
	defer C.tl_result_free(r)
	return decode(C.tl_result_get_view(r)), nil
}

func decode(v *C.tl_result_view) *Result {
	bytes := unsafe.Slice((*byte)(unsafe.Pointer(v.bytes)), int(v.n_bytes))
	// One Go copy of the arena; every string below is a substring of it (no per-term allocation for text).
	arena := string(bytes)
	str := func(s C.tl_str) string { return arena[s.off : s.off+s.len] }
	out := &Result{}
	switch v.kind {
	case C.TL_RESULT_BOOLEAN:
		out.Kind, out.Bool = Boolean, v.boolean != 0
		return out
	case C.TL_RESULT_TRIPLES:
		out.Kind = Triples
	default:
		out.Kind = Bindings
		for _, s := range unsafe.Slice(v.vars, int(v.n_vars)) {
			out.Vars = append(out.Vars, str(s))
		}
	}
	terms := make([]Term, int(v.n_terms))
	for i, t := range unsafe.Slice(v.terms, int(v.n_terms)) {
		terms[i] = Term{Kind: TermKind(t.kind), Value: str(t.value), Language: str(t.language), Datatype: str(t.datatype)}
	}
	width := int(v.n_vars)
	cells := unsafe.Slice(v.cells, int(v.n_rows)*width)
	backing := make([]*Term, len(cells))
	for i, c := range cells {
		if c != 0 {
			backing[i] = &terms[c-1]
		}
	}
	out.Rows = make([][]*Term, int(v.n_rows))
	for i := range out.Rows {
		out.Rows[i] = backing[i*width : (i+1)*width : (i+1)*width]
	}
	return out
}
