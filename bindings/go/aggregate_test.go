package triplelight

import (
	"reflect"
	"strconv"
	"strings"
	"testing"
)

type countingAggregate struct {
	count      int
	panicPhase string
}

func (state *countingAggregate) Step(term *Term) error {
	if state.panicPhase == "step" {
		panic("step")
	}
	if term.Kind == Literal {
		state.count++
	}
	return nil
}

func (state *countingAggregate) Finish() (*Term, error) {
	if state.panicPhase == "finish" {
		panic("finish")
	}
	return &Term{Kind: Literal, Value: strconv.Itoa(state.count), Datatype: xsd + "integer"}, nil
}

func TestAggregateFunctionGroups(t *testing.T) {
	db := mustMem(t)
	factory := func() (AggregateState, error) { return &countingAggregate{}, nil }
	if err := db.RegisterAggregateFunction("urn:go:count", factory); err != nil {
		t.Fatal(err)
	}
	if err := db.RegisterAggregateFunction("urn:go:count", factory); err == nil {
		t.Fatal("duplicate aggregate registration succeeded")
	}
	for _, query := range []string{
		`SELECT ?g (<urn:go:count>(DISTINCT ?v) AS ?n) WHERE {
			VALUES (?g ?v) { (1 "x") (1 "x") (1 UNDEF) (2 "y") (2 "z") }
		} GROUP BY ?g ORDER BY ?g`,
		`SELECT (<urn:go:count>(?v) AS ?n) WHERE { FILTER(false) }`,
	} {
		got, err := db.Query(query)
		if err != nil {
			t.Fatal(err)
		}
		want, err := db.Query(strings.Replace(query, "<urn:go:count>", "COUNT", 1))
		if err != nil {
			t.Fatal(err)
		}
		if !reflect.DeepEqual(got, want) {
			t.Fatalf("aggregate disagrees with COUNT: got %+v, want %+v", got, want)
		}
	}
}

func TestAggregateFunctionContainsCallbackPanics(t *testing.T) {
	db := mustMem(t)
	for _, phase := range []string{"create", "step", "finish"} {
		if err := db.RegisterAggregateFunction("urn:go:"+phase, func() (AggregateState, error) {
			if phase == "create" {
				panic("create")
			}
			return &countingAggregate{panicPhase: phase}, nil
		}); err != nil {
			t.Fatal(err)
		}
	}
	r, err := db.Query(`SELECT (<urn:go:create>(?v) AS ?a) (<urn:go:step>(?v) AS ?b)
		(<urn:go:finish>(?v) AS ?c) WHERE { VALUES ?v { "x" } }`)
	if err != nil {
		t.Fatal(err)
	}
	if len(r.Rows) != 1 || !reflect.DeepEqual(r.Rows[0], []*Term{nil, nil, nil}) {
		t.Fatalf("failed aggregate results should be unbound: %+v", r)
	}
}
