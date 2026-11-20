# TripleLight C ABI

The stable C interface behind the Go and Java bindings: `include/triplelight.h`.

Build: `cargo build --release` here produces `target/release/libtriplelight_c.{a,so,dylib}`. Test: `cargo test`.

Design:

- Strings cross as `(pointer, byte length)` UTF-8, never NUL-terminated, so U+0000 inside literals survives.
- A query returns one owned `tl_result`: a row-major array of `u32` cells that index a de-duplicated term table
  (0 = unbound), plus one UTF-8 arena. Each distinct term is encoded once; hosts decode it once and share it across
  rows. Results and errors are independent of the database and outlive `tl_close`.
- Every call returns `TL_OK` or a `TL_ERR_*` kind with an optional owned `tl_error`. Panics never unwind into the host.
- A `tl_db` may be used from several threads; `tl_close` must not race with other calls on the same handle (the Go
  and Java bindings serialize this for you).

## Arrow output

`tl_query_arrow` exports SELECT solutions as an [Arrow C Stream](https://arrow.apache.org/docs/format/CStreamInterface.html):
one nullable struct column `{kind, value, datatype, language}` per variable (a null struct is an unbound cell),
batches of at most 8192 rows. The stream is independent of the database handle. See `include/triplelight.h` for
the full contract.
