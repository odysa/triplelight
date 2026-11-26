# TripleLight for Java 25

Uses the Foreign Function & Memory API (no JNI) over the C ABI in `../c`.

```java
try (Database db = Database.inMemory()) {          // or Database.open(Path.of("data/db"))
    db.load(Path.of("data.nt"));
    Result r = db.query("SELECT ?s ?o WHERE { ?s <urn:p> ?o }");
    for (Term[] row : r.rows()) {                   // null cell = unbound
        if (row[1] instanceof Term.Literal l) System.out.println(l.value() + " " + l.datatype());
    }
}
```

Build: `cd ../c && cargo build --release`, then `mvn test` here. Run with `--enable-native-access=ALL-UNNAMED` and
point `-Dtriplelight.library=` (or `TRIPLELIGHT_LIBRARY`) at `libtriplelight_c.so`/`.dylib`, or put it on the
library path. Requires Java 25. `Database` is thread-safe; `close()` waits for calls in flight.

Results are copied into plain Java objects in one pass: the native string arena is copied once, each distinct term is
decoded once and shared across rows, and the cell array is read as one `int[]`.

## Scalar SPARQL callbacks

```java
db.registerFunction("urn:example:echo", 1, true, arguments -> arguments[0]);
Result result = db.query("SELECT (<urn:example:echo>(<urn:value>) AS ?value) WHERE {}");
```

The callback receives Java-owned RDF terms. Return one term, or return `null` or throw an exception to signal a
SPARQL expression error. Pass `null` for the arity to accept any argument count. Set `pure` only when the callback
has no side effects and its result depends solely on the arguments. Calls can run concurrently. Keep callbacks
brief and avoid calls back into the evaluating database. The registry keeps each callback alive until native code
releases the registration. This API requires a C library with scalar callback support.

## Arrow output

`Database.queryArrow(sparql, streamAddress)` fills a caller-owned `struct ArrowArrayStream`, for example
`org.apache.arrow.c.ArrowArrayStream.allocateNew(allocator).memoryAddress()`, which `Data.importArrayStream`
then reads. The binding itself does not depend on Arrow; the tests use arrow-c-data 19.0.0 with the Unsafe
allocator.
