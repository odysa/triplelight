package triplelight

/*
#include "triplelight.h"

int32_t triplelight_go_register_aggregate(const tl_db *db, const char *iri, size_t len,
                                        uintptr_t factory, tl_error **err);
*/
import "C"

import (
	"runtime"
	"runtime/cgo"
)

// AggregateState accumulates one occurrence of an aggregate in one group. Step
// receives a Go-owned copy of each bound argument. Unbound arguments and expression
// errors are skipped. Finish also runs for an empty implicit group.
type AggregateState interface {
	Step(*Term) error
	Finish() (*Term, error)
}

// AggregateFactory returns a fresh state for each call. Factory calls may overlap
// across queries. Returning an error or panicking in any callback leaves the
// affected aggregate result unbound. Callbacks must not reenter or mutate the
// database that is evaluating them.
type AggregateFactory func() (AggregateState, error)

// RegisterAggregateFunction registers a unary SPARQL aggregate under an absolute
// IRI for future queries. Scalar and aggregate functions share the same names.
func (d *DB) RegisterAggregateFunction(iri string, factory AggregateFactory) error {
	if factory == nil {
		return &Error{Kind: ErrInvalidArgument, Message: "aggregate factory is nil"}
	}
	return d.with(func(db *C.tl_db) error {
		handle := cgo.NewHandle(factory)
		var e *C.tl_error
		p, n := cstr(iri)
		code := C.triplelight_go_register_aggregate(db, p, n, C.uintptr_t(handle), &e)
		runtime.KeepAlive(iri)
		if code != C.TL_OK {
			handle.Delete()
		}
		return check(code, e)
	})
}

//export triplelightGoAggregateCreate
func triplelightGoAggregateCreate(factory C.uintptr_t, handle *C.uintptr_t) (status C.int32_t) {
	status = C.TL_ERR_INTERNAL
	defer containCallbackPanic()
	state, err := cgo.Handle(factory).Value().(AggregateFactory)()
	if err != nil || state == nil {
		return C.TL_ERR_INVALID_ARGUMENT
	}
	*handle = C.uintptr_t(cgo.NewHandle(state))
	return C.TL_OK
}

//export triplelightGoAggregateStep
func triplelightGoAggregateStep(handle C.uintptr_t, arg *C.tl_scalar_args) (status C.int32_t) {
	status = C.TL_ERR_INTERNAL
	defer containCallbackPanic()
	state := cgo.Handle(handle).Value().(AggregateState)
	if err := state.Step(decodeCallbackArgs(arg)[0]); err != nil {
		return C.TL_ERR_INVALID_ARGUMENT
	}
	return C.TL_OK
}

//export triplelightGoAggregateFinish
func triplelightGoAggregateFinish(handle C.uintptr_t, output *C.tl_scalar_output) (status C.int32_t) {
	status = C.TL_ERR_INTERNAL
	defer containCallbackPanic()
	state := cgo.Handle(handle).Value().(AggregateState)
	term, err := state.Finish()
	if err != nil {
		return C.TL_ERR_INVALID_ARGUMENT
	}
	return writeCallbackTerm(output, term)
}
