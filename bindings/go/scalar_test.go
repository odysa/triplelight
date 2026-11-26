package triplelight

import (
	"errors"
	"os"
	"reflect"
	"testing"
)

func TestScalarFunctionRoundTrip(t *testing.T) {
	db := mustMem(t)
	arity := 1
	echo := func(args []*Term) (*Term, error) { return args[0], nil }
	options := ScalarFunctionOptions{Arity: &arity, Pure: true}
	if err := db.RegisterFunction("urn:go:echo", options, echo); err != nil {
		t.Fatal(err)
	}
	if _, err := db.Update("INSERT DATA { <urn:s> <urn:p> <urn:iri>, _:b, \"01\"^^<" + xsd + "int>, \"hi\"@en, \"a\\u0000b\\t\\\"\\\\\" }"); err != nil {
		t.Fatal(err)
	}
	const query = "SELECT ?o (<urn:go:echo>(?o) AS ?echo) WHERE { <urn:s> <urn:p> ?o }"
	r, err := db.Query(query)
	if err != nil {
		t.Fatal(err)
	}
	if len(r.Rows) != 5 {
		t.Fatalf("rows = %v", r.Rows)
	}
	for _, row := range r.Rows {
		if row[0] == nil || !reflect.DeepEqual(row[0], row[1]) {
			t.Fatalf("scalar round-trip: input %+v, output %+v", row[0], row[1])
		}
	}
	if err := db.RegisterFunction("urn:go:echo", options, echo); err == nil {
		t.Fatal("duplicate registration succeeded")
	}
	if err := db.RegisterFunction("not an IRI", options, echo); err == nil {
		t.Fatal("invalid IRI registration succeeded")
	}
	if _, err := mustMem(t).Query(query); err == nil {
		t.Fatal("registration leaked to another database")
	}
	if err := db.Close(); err != nil {
		t.Fatal(err)
	}
	if err := db.RegisterFunction("urn:go:closed", options, echo); !errors.Is(err, ErrClosed) {
		t.Fatalf("register on closed database: %v", err)
	}
}

func TestScalarFunctionExpressionErrors(t *testing.T) {
	db := mustMem(t)
	zero := 0
	options := ScalarFunctionOptions{Arity: &zero}
	callbacks := map[string]ScalarFunction{
		"urn:go:panics": func([]*Term) (*Term, error) { panic("callback panic") },
		"urn:go:returns-error": func([]*Term) (*Term, error) {
			return nil, errors.New("callback error")
		},
		"urn:go:nil": func([]*Term) (*Term, error) { return nil, nil },
		"urn:go:invalid": func([]*Term) (*Term, error) {
			return &Term{Kind: IRI, Value: "invalid IRI"}, nil
		},
		"urn:go:arity": func([]*Term) (*Term, error) { return &Term{Kind: Literal, Value: "called"}, nil },
	}
	for iri, callback := range callbacks {
		if err := db.RegisterFunction(iri, options, callback); err != nil {
			t.Fatal(err)
		}
	}
	r, err := db.Query("SELECT (<urn:go:panics>() AS ?a) (<urn:go:returns-error>() AS ?b) (<urn:go:nil>() AS ?c) (<urn:go:invalid>() AS ?d) (<urn:go:arity>(42) AS ?e) (<urn:go:arity>() AS ?f) WHERE {}")
	if err != nil {
		t.Fatal(err)
	}
	if len(r.Rows) != 1 || len(r.Rows[0]) != 6 {
		t.Fatalf("result shape: %+v", r)
	}
	for i, term := range r.Rows[0][:5] {
		if term != nil {
			t.Errorf("column %d should be unbound: %+v", i, term)
		}
	}
	if term := r.Rows[0][5]; term == nil || term.Value != "called" {
		t.Errorf("zero-argument result: %+v", term)
	}
}

func TestRDF12ScalarFunctionRoundTrip(t *testing.T) {
	db := mustMem(t)
	if _, err := db.Update("INSERT DATA { <urn:a> <urn:says> <<( <urn:b> <urn:p> \"v\" )>> . <urn:a> <urn:label> \"שלום\"@he--rtl }"); err != nil {
		if os.Getenv("TRIPLELIGHT_EXPECT_RDF12") != "" {
			t.Fatal(err)
		}
		t.Skip("built without the rdf-12 feature: ", err)
	}
	if err := db.RegisterFunction("urn:go:echo", ScalarFunctionOptions{}, func(args []*Term) (*Term, error) {
		return args[0], nil
	}); err != nil {
		t.Fatal(err)
	}
	r, err := db.Query("SELECT ?o (<urn:go:echo>(?o) AS ?echo) WHERE { <urn:a> ?p ?o }")
	if err != nil {
		t.Fatal(err)
	}
	if len(r.Rows) != 2 {
		t.Fatalf("rows = %v", r.Rows)
	}
	for _, row := range r.Rows {
		if row[0] == nil || !reflect.DeepEqual(row[0], row[1]) {
			t.Fatalf("RDF 1.2 round-trip: input %+v, output %+v", row[0], row[1])
		}
	}
}
