package triplelight

/*
#include "triplelight.h"

int32_t triplelight_go_register_function(const tl_db *db, const char *iri, size_t len,
                                        size_t arity, int32_t pure, uintptr_t handle, tl_error **err);
*/
import "C"

import (
	"fmt"
	"runtime"
	"runtime/cgo"
	"strings"
	"unicode"
	"unicode/utf8"
	"unsafe"
)

// ScalarFunction maps RDF arguments to an RDF term. A nil term, returned error, or panic is
// a SPARQL expression error. Arguments are Go-owned copies. Calls can run concurrently.
// The callback must not reenter or mutate the database that is evaluating it.
type ScalarFunction func(args []*Term) (*Term, error)

// ScalarFunctionOptions controls registration. Nil Arity accepts any number of arguments;
// a non-nil Arity specifies an exact count. Pure enables query-local memoization.
type ScalarFunctionOptions struct {
	Arity *int
	Pure  bool
}

// RegisterFunction registers a process-local scalar under an absolute IRI. Later queries see
// the registration. An existing registration or built-in with the same IRI is an error.
func (d *DB) RegisterFunction(iri string, options ScalarFunctionOptions, call ScalarFunction) error {
	if call == nil {
		return &Error{Kind: ErrInvalidArgument, Message: "scalar callback is nil"}
	}
	arity := C.size_t(^uintptr(0))
	if options.Arity != nil {
		if *options.Arity < 0 {
			return &Error{Kind: ErrInvalidArgument, Message: "scalar arity must be non-negative"}
		}
		arity = C.size_t(*options.Arity)
	}
	var pure C.int32_t
	if options.Pure {
		pure = 1
	}
	return d.with(func(db *C.tl_db) error {
		handle := cgo.NewHandle(call)
		var e *C.tl_error
		p, n := cstr(iri)
		code := C.triplelight_go_register_function(db, p, n, arity, pure, C.uintptr_t(handle), &e)
		runtime.KeepAlive(iri)
		if code != C.TL_OK {
			handle.Delete()
		}
		return check(code, e)
	})
}

//export triplelightGoScalarCall
func triplelightGoScalarCall(handle C.uintptr_t, args *C.tl_scalar_args, output *C.tl_scalar_output) (status C.int32_t) {
	status = C.TL_ERR_INTERNAL
	defer containCallbackPanic()
	result, err := cgo.Handle(handle).Value().(ScalarFunction)(decodeCallbackArgs(args))
	if err != nil {
		return C.TL_ERR_INVALID_ARGUMENT
	}
	return writeCallbackTerm(output, result)
}

func containCallbackPanic() { _ = recover() }

func decodeCallbackArgs(args *C.tl_scalar_args) []*Term {
	arena := string(unsafe.Slice((*byte)(unsafe.Pointer(args.bytes)), int(args.n_bytes)))
	terms := decodeTerms(unsafe.Slice(args.terms, int(args.n_terms)), func(s C.tl_str) string {
		return arena[s.off : s.off+s.len]
	})
	return decodeCells(unsafe.Slice(args.cells, int(args.len)), terms)
}

func writeCallbackTerm(output *C.tl_scalar_output, result *Term) C.int32_t {
	var term strings.Builder
	if !appendScalarTerm(&term, result, 0) {
		return C.TL_ERR_INVALID_ARGUMENT
	}
	value := term.String()
	p, n := cstr(value)
	status := C.tl_scalar_set_term(output, p, n)
	runtime.KeepAlive(value)
	return status
}

//export triplelightGoCallbackRelease
func triplelightGoCallbackRelease(handle C.uintptr_t) {
	cgo.Handle(handle).Delete()
}

func appendScalarIRI(out *strings.Builder, iri string) bool {
	if iri == "" || strings.ContainsAny(iri, "<>\"{}|^`\\") || strings.IndexFunc(iri, func(r rune) bool { return r <= ' ' }) >= 0 {
		return false
	}
	out.WriteByte('<')
	out.WriteString(iri)
	out.WriteByte('>')
	return true
}

func appendScalarTerm(out *strings.Builder, term *Term, depth int) bool {
	// Match the engine's RDF 1.2 term depth limit and bound cyclic Go inputs.
	if term == nil || depth > 63 || !utf8.ValidString(term.Value) {
		return false
	}
	switch term.Kind {
	case IRI:
		return appendScalarIRI(out, term.Value)
	case BlankNode:
		if term.Value == "" || strings.ContainsAny(term.Value, "<>()\"") || strings.IndexFunc(term.Value, unicode.IsSpace) >= 0 {
			return false
		}
		out.WriteString("_:")
		out.WriteString(term.Value)
	case Literal:
		return appendScalarLiteral(out, term)
	case Triple:
		out.WriteString("<<( ")
		if !appendScalarTerm(out, term.Subject, depth+1) {
			return false
		}
		out.WriteByte(' ')
		if term.Predicate == nil || term.Predicate.Kind != IRI || !appendScalarIRI(out, term.Predicate.Value) {
			return false
		}
		out.WriteByte(' ')
		if !appendScalarTerm(out, term.Object, depth+1) {
			return false
		}
		out.WriteString(" )>>")
	default:
		return false
	}
	return true
}

func appendScalarLiteral(out *strings.Builder, term *Term) bool {
	out.WriteByte('"')
	for _, r := range term.Value {
		switch {
		case r == '"' || r == '\\':
			out.WriteByte('\\')
			out.WriteRune(r)
		case r < 0x20 || r == 0x7f || r == 0xfffe || r == 0xffff:
			fmt.Fprintf(out, "\\u%04X", r)
		default:
			out.WriteRune(r)
		}
	}
	out.WriteByte('"')
	if term.Language != "" {
		const rdf = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
		datatype := rdf + "langString"
		if term.Direction != "" {
			datatype = rdf + "dirLangString"
		}
		if term.Datatype != "" && term.Datatype != datatype {
			return false
		}
		if strings.IndexFunc(term.Language, func(r rune) bool {
			return !(r == '-' || 'a' <= r && r <= 'z' || 'A' <= r && r <= 'Z' || '0' <= r && r <= '9')
		}) >= 0 {
			return false
		}
		out.WriteByte('@')
		out.WriteString(term.Language)
		switch term.Direction {
		case "":
		case "ltr", "rtl":
			out.WriteString("--")
			out.WriteString(term.Direction)
		default:
			return false
		}
	} else if term.Direction != "" {
		return false
	} else if term.Datatype != "" {
		out.WriteString("^^")
		return appendScalarIRI(out, term.Datatype)
	}
	return true
}
