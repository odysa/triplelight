package arrowtest

import (
	"errors"
	"fmt"
	"reflect"
	"strings"
	"testing"
	"unsafe"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/array"
	"github.com/apache/arrow-go/v18/arrow/cdata"
	tl "github.com/odysa/triplelight/bindings/go"
)

const xsd = "http://www.w3.org/2001/XMLSchema#"

// cell is one solution cell; Bound false means unbound. Datatype and Language are nil when absent.
type cell struct {
	Bound              bool
	Kind, Value        string
	Datatype, Language *string
}

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
func importStream(stream *cdata.CArrowArrayStream) (array.RecordReader, error) {
	r, err := cdata.ImportCRecordReader(stream, nil)
	if err != nil {
		return nil, err
	}
	return r.(array.RecordReader), nil
}

func opt(a *array.String, i int) *string {
	if a.IsNull(i) {
		return nil
	}
	v := strings.Clone(a.Value(i))
	return &v
}

func arrowCell(s *array.Struct, i int) cell {
	if s.IsNull(i) {
		return cell{}
	}
	// arrow-go's Value aliases the batch buffer, which is freed when the reader advances: copy.
	return cell{Bound: true, Kind: strings.Clone(s.Field(0).(*array.String).Value(i)), Value: strings.Clone(s.Field(1).(*array.LargeString).Value(i)),
		Datatype: opt(s.Field(2).(*array.String), i), Language: opt(s.Field(3).(*array.String), i)}
}

func viewCell(t *tl.Term) cell {
	if t == nil {
		return cell{}
	}
	c := cell{Bound: true, Kind: map[tl.TermKind]string{tl.IRI: "iri", tl.BlankNode: "bnode", tl.Literal: "literal"}[t.Kind], Value: t.Value}
	if t.Kind == tl.Literal {
		dt := t.Datatype
		c.Datatype = &dt
		if t.Language != "" {
			lang := t.Language
			c.Language = &lang
		}
	}
	return c
}

func show(row []cell) string {
	var b strings.Builder
	for _, c := range row {
		d, l := "<nil>", "<nil>"
		if c.Datatype != nil {
			d = *c.Datatype
		}
		if c.Language != nil {
			l = *c.Language
		}
		fmt.Fprintf(&b, "{%v %q %q %q %q} ", c.Bound, c.Kind, c.Value, d, l)
	}
	return b.String()
}

type imported struct {
	names   []string
	rows    [][]cell
	batches []int
}

func viaStream(t *testing.T, db *tl.DB, q string) imported {
	t.Helper()
	var stream cdata.CArrowArrayStream
	if err := db.QueryArrow(q, unsafe.Pointer(&stream)); err != nil {
		t.Fatal(err)
	}
	rdr, err := importStream(&stream)
	if err != nil {
		t.Fatal(err)
	}
	defer rdr.Release()
	var out imported
	for _, f := range rdr.Schema().Fields() {
		if !f.Nullable || f.Type.ID() != arrow.STRUCT {
			t.Fatalf("field %s: %v nullable=%v", f.Name, f.Type, f.Nullable)
		}
		out.names = append(out.names, f.Name)
	}
	for rdr.Next() {
		rec := rdr.RecordBatch()
		if int(rec.NumCols()) != len(out.names) {
			t.Fatalf("batch has %d columns", rec.NumCols())
		}
		out.batches = append(out.batches, int(rec.NumRows()))
		for i := 0; i < int(rec.NumRows()); i++ {
			row := make([]cell, rec.NumCols())
			for c := range row {
				row[c] = arrowCell(rec.Column(c).(*array.Struct), i)
			}
			out.rows = append(out.rows, row)
		}
	}
	if err := rdr.Err(); err != nil {
		t.Fatal(err)
	}
	return out
}

func viaView(t *testing.T, db *tl.DB, q string) ([]string, [][]cell) {
	t.Helper()
	r, err := db.Query(q)
	if err != nil {
		t.Fatal(err)
	}
	rows := [][]cell{}
	for _, row := range r.Rows {
		cells := make([]cell, len(row))
		for c, term := range row {
			cells[c] = viewCell(term)
		}
		rows = append(rows, cells)
	}
	return r.Vars, rows
}

func sameAsView(t *testing.T, db *tl.DB, q string) imported {
	t.Helper()
	got := viaStream(t, db, q)
	vars, want := viaView(t, db, q)
	if !reflect.DeepEqual(append([]string{}, got.names...), append([]string{}, vars...)) {
		t.Fatalf("names: %v vs %v", got.names, vars)
	}
	if got.rows == nil {
		got.rows = [][]cell{}
	}
	if !reflect.DeepEqual(got.rows, want) {
		for i := range got.rows {
			if i >= len(want) || !reflect.DeepEqual(got.rows[i], want[i]) {
				t.Fatalf("stream and view differ for %q at row %d of %d/%d:\n%s\n%s", q, i, len(got.rows), len(want), show(got.rows[i]), show(want[min(i, len(want)-1)]))
			}
		}
		t.Fatalf("stream and view differ for %q: %d vs %d rows", q, len(got.rows), len(want))
	}
	return got
}

func TestTypedTermsMatchView(t *testing.T) {
	db := mustMem(t)
	if _, err := db.Update(`INSERT DATA { <urn:a> <urn:p> <urn:iri> . <urn:a> <urn:p> "urn:iri" . <urn:a> <urn:p> "" .
		<urn:a> <urn:p> "hi"@en . <urn:a> <urn:p> "01"^^<` + xsd + `int> . <urn:a> <urn:p> _:b .
		<urn:a> <urn:p> "a|b ^c @d - e" }`); err != nil {
		t.Fatal(err)
	}
	q := `SELECT ?o ?z ?len WHERE { <urn:a> <urn:p> ?o OPTIONAL { ?o <urn:none> ?z }
		BIND(IF(isLiteral(?o), STRLEN(STR(?o)), 0) AS ?len) } ORDER BY STR(?o)`
	got := sameAsView(t, db, q)
	if len(got.rows) != 7 {
		t.Fatalf("rows: %d", len(got.rows))
	}
	found := false
	for _, r := range got.rows {
		if r[0].Value == "a|b ^c @d - e" && r[0].Language == nil && *r[0].Datatype == xsd+"string" && !r[1].Bound {
			found = true
		}
	}
	if !found {
		t.Fatal("delimiter-bearing literal not round-tripped")
	}
}

func TestBatchBoundaries(t *testing.T) {
	for _, n := range []int{8191, 8192, 8193, 16387} {
		t.Run(fmt.Sprint(n), func(t *testing.T) {
			db := mustMem(t)
			var b strings.Builder
			b.WriteString("INSERT DATA {")
			for i := 0; i < n; i++ {
				// Mixed across the boundary: language-tagged, empty, noncanonical integers; every 5th row binds ?x.
				o := fmt.Sprintf(`"0%d"^^<%sint>`, i, xsd)
				switch i % 7 {
				case 0:
					o = fmt.Sprintf(`"t%d"@en`, i)
				case 3:
					o = `""`
				}
				fmt.Fprintf(&b, " <urn:r%06d> <urn:p> %s .", i, o)
				if i%5 == 0 {
					fmt.Fprintf(&b, " <urn:r%06d> <urn:q> <urn:x%d> .", i, i)
				}
			}
			b.WriteString(" }")
			if _, err := db.Update(b.String()); err != nil {
				t.Fatal(err)
			}
			got := sameAsView(t, db, `SELECT ?s ?o ?x WHERE { ?s <urn:p> ?o OPTIONAL { ?s <urn:q> ?x } } ORDER BY ?s`)
			total := 0
			for i, s := range got.batches {
				if s > 8192 || (i < len(got.batches)-1 && s != 8192) {
					t.Fatalf("batch sizes %v", got.batches)
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
	if _, err := db.Update(`INSERT DATA { <urn:a> <urn:p> 1 . <urn:a> <urn:p> 2 . <urn:a> <urn:p> 3 . <urn:b> <urn:p> 2 }`); err != nil {
		t.Fatal(err)
	}
	empty := viaStream(t, db, `SELECT ?s WHERE { ?s <urn:none> ?o }`)
	if len(empty.rows) != 0 || !reflect.DeepEqual(empty.names, []string{"s"}) {
		t.Fatalf("empty: %+v", empty)
	}
	// A SELECT with no variables keeps its cardinality as the batch row count.
	for q, n := range map[string]int{
		`SELECT * WHERE { <urn:a> <urn:p> 2 }`:  1,
		`SELECT * WHERE { <urn:a> <urn:p> [] }`: 3,
		`SELECT * WHERE { <urn:a> <urn:p> [] . [] <urn:p> 2 }`: 6,
		`SELECT * WHERE { <urn:a> <urn:p> 9 }`:  0,
	} {
		got := viaStream(t, db, q)
		vars, view := viaView(t, db, q)
		if len(got.names) != 0 || len(vars) != 0 || len(got.rows) != n || len(view) != n {
			t.Fatalf("%q: names %v rows %d view %d, want %d", q, got.names, len(got.rows), len(view), n)
		}
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
		if q[0] != 'S' && tle.Kind != tl.ErrUnsupported {
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
	rdr, err := importStream(&stream)
	if err != nil {
		t.Fatal(err)
	}
	defer rdr.Release()
	if !rdr.Next() || rdr.RecordBatch().NumRows() != 1 {
		t.Fatal("stream must outlive Close")
	}
	dt := xsd + "string"
	if got := arrowCell(rdr.RecordBatch().Column(1).(*array.Struct), 0); !reflect.DeepEqual(got, cell{Bound: true, Kind: "literal", Value: "x", Datatype: &dt}) {
		t.Fatalf("cell after Close: %+v", got)
	}
	if rdr.Next() || rdr.Err() != nil {
		t.Fatalf("end of stream: %v", rdr.Err())
	}
}
