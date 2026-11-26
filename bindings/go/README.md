# TripleLight for Go

```go
import triplelight "github.com/odysa/triplelight/bindings/go"

db, err := triplelight.OpenInMemory() // or triplelight.Open("data/db")
defer db.Close()
db.Load("data.nt")
r, err := db.Query(`SELECT ?s ?o WHERE { ?s <urn:p> ?o }`)
for _, row := range r.Rows { // row[i] is nil when r.Vars[i] is unbound
	fmt.Println(row[0].Value, row[1].Kind, row[1].Datatype)
}
```

Build the native library first (`cd ../c && cargo build --release`); cgo links its static archive. `go test ./...`
runs the tests. A `*DB` is safe for concurrent use; `Close` waits for calls in flight.

## Scalar SPARQL functions

```go
arity := 1
err = db.RegisterFunction("urn:app:echo", triplelight.ScalarFunctionOptions{Arity: &arity, Pure: true},
	func(args []*triplelight.Term) (*triplelight.Term, error) {
		return args[0], nil
	})
// Use <urn:app:echo>(?o) in a SPARQL expression.
```

Registration applies to later queries on this database. A callback receives Go-owned terms and can return
one of them or construct a new term. A nil result, error, panic, or invalid term is a SPARQL expression error.
Callbacks can run concurrently and must avoid reentering or mutating the database that called them.
Triple terms and directional literals require a native library built with `rdf-12`.

## Arrow output

`DB.QueryArrow(sparql, unsafe.Pointer(&stream))` fills a caller-owned `struct ArrowArrayStream`, for example
arrow-go's `cdata.CArrowArrayStream`, which `cdata.ImportCRecordReader` then reads. The binding itself does not
depend on Arrow; the import tests live in the separate `arrowtest` module (arrow-go v18.8.0).
arrow-go's `String.Value` aliases the batch buffer, which is released when the reader advances; copy values
(`strings.Clone`) that must outlive the batch.
