package arrowtest

import (
	"errors"
	"fmt"
	"strings"
	"testing"
	"unsafe"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/array"
	"github.com/apache/arrow-go/v18/arrow/cdata"
	tl "github.com/odysa/triplelight/bindings/go"
)

const xsd = "http://www.w3.org/2001/XMLSchema#"

func mustMem(t *testing.T) *tl.DB {
	t.Helper()
	db, err := tl.OpenInMemory()
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { db.Close() })
	return db
}

// importStream imports a C stream with arrow-go; the reader takes ownership of the stream.
func importStream(stream *cdata.CArrowArrayStream, schema *arrow.Schema) (array.RecordReader, error) {
	r, err := cdata.ImportCRecordReader(stream, schema)
	if err != nil {
		return nil, err
	}
	return r.(array.RecordReader), nil
}

// cell renders one struct cell; "-" is unbound.
func cell(s *array.Struct, i int) string {
	if s.IsNull(i) {
		return "-"
	}
	kind := s.Field(0).(*array.String).Value(i)
	value := s.Field(1).(*array.LargeString).Value(i)
	dt, lang := s.Field(2).(*array.String), s.Field(3).(*array.String)
	out := kind + "|" + value
	if !dt.IsNull(i) {
		out += "|^" + dt.Value(i)
	}
	if !lang.IsNull(i) {
		out += "|@" + lang.Value(i)
	}
	return out
}

func viewCell(t *tl.Term) string {
	if t == nil {
		return "-"
	}
	kind := map[tl.TermKind]string{tl.IRI: "iri", tl.BlankNode: "bnode", tl.Literal: "literal"}[t.Kind]
	out := kind + "|" + t.Value
	if t.Kind == tl.Literal {
		out += "|^" + t.Datatype
		if t.Language != "" {
			out += "|@" + t.Language
		}
	}
	return out
}

// viaStream imports the query's stream with arrow-go and returns the rows, the field names and the batch sizes.
func viaStream(t *testing.T, db *tl.DB, q string) ([]string, []string, []int) {
	t.Helper()
	var stream cdata.CArrowArrayStream
	if err := db.QueryArrow(q, unsafe.Pointer(&stream)); err != nil {
		t.Fatal(err)
	}
	rdr, err := importStream(&stream, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer rdr.Release()
	var names []string
	for _, f := range rdr.Schema().Fields() {
		if !f.Nullable || f.Type.ID() != arrow.STRUCT {
			t.Fatalf("field %s: %v nullable=%v", f.Name, f.Type, f.Nullable)
		}
		names = append(names, f.Name)
	}
	var rows []string
	var sizes []int
	for rdr.Next() {
		rec := rdr.RecordBatch()
		sizes = append(sizes, int(rec.NumRows()))
		for i := 0; i < int(rec.NumRows()); i++ {
			parts := make([]string, rec.NumCols())
			for c := range parts {
				parts[c] = cell(rec.Column(c).(*array.Struct), i)
			}
			rows = append(rows, strings.Join(parts, " "))
		}
	}
	if err := rdr.Err(); err != nil {
		t.Fatal(err)
	}
	return rows, names, sizes
}

func viaView(t *testing.T, db *tl.DB, q string) ([]string, []string) {
	t.Helper()
	r, err := db.Query(q)
	if err != nil {
		t.Fatal(err)
	}
	var rows []string
	for _, row := range r.Rows {
		parts := make([]string, len(row))
		for c, term := range row {
			parts[c] = viewCell(term)
		}
		rows = append(rows, strings.Join(parts, " "))
	}
	return rows, r.Vars
}

func same(t *testing.T, a, b []string) {
	t.Helper()
	if strings.Join(a, "\n") != strings.Join(b, "\n") {
		t.Fatalf("stream and view differ:\n%v\n%v", a, b)
	}
}

func TestTypedTermsMatchView(t *testing.T) {
	db := mustMem(t)
	if _, err := db.Update(`INSERT DATA { <urn:a> <urn:p> <urn:iri> . <urn:a> <urn:p> "urn:iri" . <urn:a> <urn:p> "" .
		<urn:a> <urn:p> "hi"@en . <urn:a> <urn:p> "01"^^<` + xsd + `int> . <urn:a> <urn:p> _:b }`); err != nil {
		t.Fatal(err)
	}
	q := `SELECT ?o ?z ?len WHERE { <urn:a> <urn:p> ?o OPTIONAL { ?o <urn:none> ?z }
		BIND(IF(isLiteral(?o), STRLEN(STR(?o)), 0) AS ?len) } ORDER BY STR(?o)`
	got, names, _ := viaStream(t, db, q)
	want, vars := viaView(t, db, q)
	same(t, names, vars)
	same(t, got, want)
	if len(got) != 6 {
		t.Fatalf("rows: %d", len(got))
	}
}

func TestBatchBoundaries(t *testing.T) {
	for _, n := range []int{8191, 8192, 8193} {
		t.Run(fmt.Sprint(n), func(t *testing.T) {
			db := mustMem(t)
			var b strings.Builder
			b.WriteString("INSERT DATA {")
			for i := 0; i < n; i++ {
				fmt.Fprintf(&b, ` <urn:s%d> <urn:p> "%d"@en .`, i, i)
			}
			b.WriteString(" }")
			if _, err := db.Update(b.String()); err != nil {
				t.Fatal(err)
			}
			q := `SELECT ?s ?o ?z WHERE { ?s <urn:p> ?o OPTIONAL { ?s <urn:none> ?z } } ORDER BY ?s`
			got, _, sizes := viaStream(t, db, q)
			want, _ := viaView(t, db, q)
			same(t, got, want)
			total := 0
			for i, s := range sizes {
				if s > 8192 || (i < len(sizes)-1 && s != 8192) {
					t.Fatalf("batch sizes %v", sizes)
				}
				total += s
			}
			if total != n {
				t.Fatalf("total %d", total)
			}
		})
	}
}

func TestZeroColumnsAndEmpty(t *testing.T) {
	db := mustMem(t)
	if _, err := db.Update(`INSERT DATA { <urn:a> <urn:p> 1 . <urn:b> <urn:p> 2 }`); err != nil {
		t.Fatal(err)
	}
	rows, names, _ := viaStream(t, db, `SELECT ?s WHERE { ?s <urn:none> ?o }`)
	if len(rows) != 0 || len(names) != 1 {
		t.Fatalf("empty: %v %v", rows, names)
	}
	// A SELECT with no variables keeps its cardinality as the batch row count.
	rows, names, _ = viaStream(t, db, `SELECT * WHERE { <urn:a> <urn:p> 1 }`)
	if len(names) != 0 || len(rows) != 1 {
		t.Fatalf("zero columns: %v %v", names, rows)
	}
	rows, _, _ = viaStream(t, db, `SELECT * WHERE { <urn:a> <urn:p> 2 }`)
	if len(rows) != 0 {
		t.Fatalf("zero columns, no match: %v", rows)
	}
}

func TestErrorsAndLifetime(t *testing.T) {
	db := mustMem(t)
	if _, err := db.Update(`INSERT DATA { <urn:a> <urn:p> "x" }`); err != nil {
		t.Fatal(err)
	}
	for _, q := range []string{`ASK { ?s ?p ?o }`, `CONSTRUCT { ?s ?p ?o } WHERE { ?s ?p ?o }`, `SELECT WHERE {`} {
		var stream cdata.CArrowArrayStream
		err := db.QueryArrow(q, unsafe.Pointer(&stream))
		var tle *tl.Error
		if err == nil || !errors.As(err, &tle) {
			t.Fatalf("%q: %v", q, err)
		}
		if want := tl.ErrUnsupported; q[0] != 'S' && tle.Kind != want {
			t.Fatalf("%q: kind %v", q, tle.Kind)
		}
		if stream != (cdata.CArrowArrayStream{}) {
			t.Fatalf("%q: stream written on failure", q)
		}
	}
	var stream cdata.CArrowArrayStream
	if err := db.QueryArrow(`SELECT ?s ?o WHERE { ?s ?p ?o }`, unsafe.Pointer(&stream)); err != nil {
		t.Fatal(err)
	}
	db.Close()
	rdr, err := importStream(&stream, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer rdr.Release()
	if !rdr.Next() || rdr.RecordBatch().NumRows() != 1 || cell(rdr.RecordBatch().Column(1).(*array.Struct), 0) != "literal|x|^"+xsd+"string" {
		t.Fatal("stream must outlive Close")
	}
	if rdr.Next() || rdr.Err() != nil {
		t.Fatalf("end of stream: %v", rdr.Err())
	}
}
