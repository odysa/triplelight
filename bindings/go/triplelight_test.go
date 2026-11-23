package triplelight

import (
	"errors"
	"os"
	"path/filepath"
	"sync"
	"testing"
)

const xsd = "http://www.w3.org/2001/XMLSchema#"

func mustMem(t *testing.T) *DB {
	t.Helper()
	db, err := OpenInMemory()
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { db.Close() })
	return db
}

func TestTypedTermsUnboundAndSharing(t *testing.T) {
	db := mustMem(t)
	n, err := db.Update(`INSERT DATA { <urn:a> <urn:p> "01"^^<` + xsd + `int> . <urn:a> <urn:p> "hi"@en .
		<urn:a> <urn:q> _:b . <urn:c> <urn:p> "x" }`)
	if err != nil || n != 4 {
		t.Fatalf("update: %d %v", n, err)
	}
	r, err := db.Query(`SELECT ?s ?o ?z WHERE { ?s <urn:p> ?o OPTIONAL { ?s <urn:none> ?z } } ORDER BY ?s ?o`)
	if err != nil {
		t.Fatal(err)
	}
	if r.Kind != Bindings || len(r.Vars) != 3 || r.Vars[2] != "z" || len(r.Rows) != 3 {
		t.Fatalf("shape: %+v", r)
	}
	seen := map[string]Term{}
	for _, row := range r.Rows {
		if row[2] != nil {
			t.Fatal("?z must be unbound")
		}
		seen[row[1].Value] = *row[1]
	}
	if r.Rows[0][0] != r.Rows[1][0] {
		t.Fatal("the same term should be shared across rows")
	}
	if got := seen["01"]; got.Kind != Literal || got.Datatype != xsd+"int" || got.Language != "" {
		t.Fatalf("typed literal: %+v", got)
	}
	if got := seen["hi"]; got.Language != "en" {
		t.Fatalf("language literal: %+v", got)
	}
	if got := seen["x"]; got.Datatype != xsd+"string" {
		t.Fatalf("simple literal: %+v", got)
	}
}

func TestEmptyAskConstructAndNUL(t *testing.T) {
	db := mustMem(t)
	if _, err := db.Update("INSERT DATA { <urn:a> <urn:q> _:b . <urn:a> <urn:n> \"a\\u0000b\" }"); err != nil {
		t.Fatal(err)
	}
	r, _ := db.Query("SELECT ?x ?y WHERE { ?x <urn:nothing> ?y }")
	if len(r.Vars) != 2 || len(r.Rows) != 0 {
		t.Fatalf("empty projection: %+v", r)
	}
	r, _ = db.Query("ASK { <urn:a> <urn:q> ?b }")
	if r.Kind != Boolean || !r.Bool {
		t.Fatalf("ask: %+v", r)
	}
	r, _ = db.Query("CONSTRUCT { ?s <urn:r> ?o } WHERE { ?s <urn:q> ?o }")
	if r.Kind != Triples || len(r.Rows) != 1 || r.Rows[0][2].Kind != BlankNode {
		t.Fatalf("construct: %+v", r)
	}
	r, _ = db.Query("SELECT ?v WHERE { <urn:a> <urn:n> ?v }")
	if r.Rows[0][0].Value != "a\x00b" {
		t.Fatalf("NUL must survive: %q", r.Rows[0][0].Value)
	}
}

func TestErrorsAndClose(t *testing.T) {
	db, _ := OpenInMemory()
	_, err := db.Query("SELEC nope")
	var te *Error
	if !errors.As(err, &te) || te.Kind != ErrSyntax {
		t.Fatalf("want syntax error, got %v", err)
	}
	if _, err := db.Load("/nonexistent/missing.nt"); !errors.As(err, &te) || te.Kind != ErrStorage {
		t.Fatalf("want storage error, got %v", err)
	}
	db.Close()
	db.Close()
	if _, err := db.Query("ASK {}"); err != ErrClosed {
		t.Fatalf("want ErrClosed, got %v", err)
	}
}

func TestPersistentLockAndParity(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "db")
	disk, err := Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	var te *Error
	if _, err := Open(dir); !errors.As(err, &te) || te.Kind != ErrLocked {
		t.Fatalf("second open must be locked, got %v", err)
	}
	mem := mustMem(t)
	nt := filepath.Join(t.TempDir(), "d.nt")
	os.WriteFile(nt, []byte("<urn:a> <urn:p> \"1\"^^<"+xsd+"integer> .\n<urn:b> <urn:p> \"2\"^^<"+xsd+"integer> .\n"), 0o644)
	for _, d := range []*DB{disk, mem} {
		if n, err := d.Load(nt); err != nil || n != 2 {
			t.Fatalf("load: %d %v", n, err)
		}
		r, err := d.Query("SELECT (SUM(?o) AS ?n) WHERE { ?s ?p ?o }")
		if err != nil || r.Rows[0][0].Value != "3" {
			t.Fatalf("sum: %+v %v", r, err)
		}
	}
	disk.Close()
	reopened, err := Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	defer reopened.Close()
	r, _ := reopened.Query("SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }")
	if r.Rows[0][0].Value != "2" {
		t.Fatalf("persisted count: %+v", r.Rows[0][0])
	}
}

func TestConcurrentQueriesAndClose(t *testing.T) {
	db, _ := OpenInMemory()
	db.Update("INSERT DATA { <urn:a> <urn:p> 1 }")
	var wg sync.WaitGroup
	for i := 0; i < 16; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for j := 0; j < 200; j++ {
				if _, err := db.Query("SELECT * WHERE { ?s ?p ?o }"); err != nil && err != ErrClosed {
					t.Error(err)
					return
				}
			}
		}()
	}
	db.Close()
	wg.Wait()
}

func TestRDF12TripleTermsAndDirection(t *testing.T) {
	db := mustMem(t)
	if _, err := db.Update(`INSERT DATA { <urn:a> <urn:says> <<( <urn:b> <urn:p> "v" )>> . <urn:a> <urn:label> "שלום"@he--rtl }`); err != nil {
		if os.Getenv("TRIPLELIGHT_EXPECT_RDF12") != "" {
			t.Fatal(err) // the build is supposed to have rdf-12: a refusal is a failure, not a skip
		}
		t.Skip("built without the rdf-12 feature: ", err)
	}
	r, err := db.Query(`SELECT ?p ?o WHERE { <urn:a> ?p ?o }`)
	if err != nil {
		t.Fatal(err)
	}
	got := map[string]*Term{}
	for _, row := range r.Rows {
		got[row[0].Value] = row[1]
	}
	tr := got["urn:says"]
	if tr == nil || tr.Kind != Triple || tr.Subject.Value != "urn:b" || tr.Predicate.Value != "urn:p" || tr.Object.Value != "v" {
		t.Fatalf("triple term: %+v", tr)
	}
	if l := got["urn:label"]; l == nil || l.Language != "he" || l.Direction != "rtl" {
		t.Fatalf("directional literal: %+v", l)
	}
}
