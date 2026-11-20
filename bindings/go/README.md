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

## Arrow output

`DB.QueryArrow(sparql, unsafe.Pointer(&stream))` fills a caller-owned `struct ArrowArrayStream`, for example
arrow-go's `cdata.CArrowArrayStream`, which `cdata.ImportCRecordReader` then reads. The binding itself does not
depend on Arrow; the import tests live in the separate `arrowtest` module (arrow-go v18.8.0).
