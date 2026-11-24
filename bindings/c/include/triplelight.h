/* C ABI for the embedded TripleLight RDF database (shared by the Go and Java bindings).
 *
 * Strings are passed as (pointer, byte length) pairs of UTF-8 and are never NUL-terminated; they may contain NUL
 * bytes (U+0000 is valid in RDF literals), so always use the length. Input strings must be valid UTF-8, otherwise the
 * call fails with TL_ERR_INVALID_ARGUMENT; output strings are always valid UTF-8.
 * Every fallible call returns TL_OK (0) or a TL_ERR_* kind; when `err` is non-null it then receives an owned
 * tl_error, freed with tl_error_free. No call unwinds across this boundary.
 *
 * Threading: a tl_db may be used from several threads at once. tl_close must not race with other calls on the
 * same handle. A tl_result is immutable and may be read from any thread until tl_result_free.
 *
 * Output pointers: a required `out` argument is checked before any work is done (a NULL `out` fails with
 * TL_ERR_INVALID_ARGUMENT without opening files or running the query). On success `*out` (and any count output) is
 * written and `*err` is left untouched; on failure `*out` and count outputs are left untouched and `*err` receives an
 * owned tl_error when `err` is non-NULL. Read `*out`, counts and `*err` only as the returned status says: after
 * TL_OK, `*err` still holds whatever it held before the call; after a failure, `*out` does. Free any owned value
 * (tl_result_free, tl_error_free, tl_close) before passing the same variable to another call, and reset it to NULL
 * if you will inspect it again, so a stale or freed pointer is never mistaken for a new result or error.
 *
 * Ownership: tl_result and tl_error are independent of the database and stay valid after tl_close until freed with
 * tl_result_free / tl_error_free. All four free/close functions ignore NULL.
 */
#ifndef TRIPLELIGHT_H
#define TRIPLELIGHT_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

#define TL_ABI_VERSION 1u

#define TL_OK 0
#define TL_ERR_SYNTAX 1
#define TL_ERR_UNSUPPORTED 2
#define TL_ERR_STORAGE 3
#define TL_ERR_LOCKED 4
#define TL_ERR_INVALID_ARGUMENT 5
#define TL_ERR_INTERNAL 6
/* An update names a graph in the wrong state (CREATE of an existing graph; DROP, CLEAR, ADD, MOVE or COPY of an
 * absent one, without SILENT). */
#define TL_ERR_GRAPH_STATE 7
#define TL_ERR_INTERRUPTED 8
/* tl_validate: SHACL rejects the shapes graph (a malformed shape or list, or an undefined recursion). */
#define TL_ERR_SHAPES 9
#define TL_ERR_RESOURCE_LIMIT 10

#define TL_RESULT_BINDINGS 0u
#define TL_RESULT_BOOLEAN 1u
#define TL_RESULT_TRIPLES 2u

#define TL_TERM_IRI 1
#define TL_TERM_BLANK 2
#define TL_TERM_LITERAL 3
#define TL_TERM_TRIPLE 4 /* RDF 1.2 triple term; only in builds with the rdf-12 feature */

/* tl_term.direction for a language-tagged literal (RDF 1.2): 0 none, 1 ltr, 2 rtl. */
#define TL_DIR_NONE 0
#define TL_DIR_LTR 1
#define TL_DIR_RTL 2

typedef struct tl_db tl_db;
typedef struct tl_result tl_result;
typedef struct tl_error tl_error;

/* A slice of the result's byte arena. */
typedef struct tl_str { uint64_t off; uint64_t len; } tl_str;

/* One distinct term. Literals always carry a datatype IRI; `language` is empty unless language-tagged.
 * TL_TERM_TRIPLE: the subject, predicate and object are 1-based indices into the result's term table (the same numbering
 * as result cells), stored in value.off, value.len and language.off; the other fields are zero. They are never 0. */
typedef struct tl_term {
    uint8_t kind; /* TL_TERM_* */
    uint8_t direction; /* TL_DIR_*; formerly padding, so the layout and the ABI version are unchanged */
    uint8_t _pad[6];
    tl_str value;
    tl_str language;
    tl_str datatype;
} tl_term;

/* Read-only view of a result; pointers are valid until tl_result_free. */
typedef struct tl_result_view {
    uint32_t kind;     /* TL_RESULT_* */
    uint32_t boolean;  /* 0 or 1 for TL_RESULT_BOOLEAN */
    uint64_t n_vars;   /* projection width (also for empty results); 3 unnamed columns for triples */
    const tl_str *vars;
    uint64_t n_rows;
    /* n_rows * n_vars cells, row-major. A nonzero cell is a 1-based index into terms (cell c -> terms[c - 1]);
     * 0 = unbound. The library guarantees n_rows * n_vars and every tl_str offset + len fit in size_t. */
    const uint32_t *cells;
    uint64_t n_terms;
    const tl_term *terms;  /* each distinct term once */
    uint64_t n_bytes;
    const uint8_t *bytes;  /* UTF-8 arena for every tl_str */
} tl_result_view;

/* Arrow C Data / C Stream interface (https://arrow.apache.org/docs/format/CStreamInterface.html), declared here
 * unless the including program already has it. */
#ifndef ARROW_C_DATA_INTERFACE
#define ARROW_C_DATA_INTERFACE
#define ARROW_FLAG_DICTIONARY_ORDERED 1
#define ARROW_FLAG_NULLABLE 2
#define ARROW_FLAG_MAP_KEYS_SORTED 4
struct ArrowSchema {
  const char *format; const char *name; const char *metadata; int64_t flags; int64_t n_children;
  struct ArrowSchema **children; struct ArrowSchema *dictionary;
  void (*release)(struct ArrowSchema *); void *private_data;
};
struct ArrowArray {
  int64_t length; int64_t null_count; int64_t offset; int64_t n_buffers; int64_t n_children;
  const void **buffers; struct ArrowArray **children; struct ArrowArray *dictionary;
  void (*release)(struct ArrowArray *); void *private_data;
};
#endif
#ifndef ARROW_C_STREAM_INTERFACE
#define ARROW_C_STREAM_INTERFACE
struct ArrowArrayStream {
  int (*get_schema)(struct ArrowArrayStream *, struct ArrowSchema *out);
  int (*get_next)(struct ArrowArrayStream *, struct ArrowArray *out);
  const char *(*get_last_error)(struct ArrowArrayStream *);
  void (*release)(struct ArrowArrayStream *);
  void *private_data;
};
#endif

uint32_t tl_abi_version(void);

int32_t tl_open(const char *path, size_t path_len, tl_db **out, tl_error **err);
int32_t tl_open_in_memory(tl_db **out, tl_error **err);
void tl_close(tl_db *db);

int32_t tl_query(const tl_db *db, const char *sparql, size_t len, tl_result **out, tl_error **err);
/* tl_query under an entailment regime: TL_ENTAILMENT_SIMPLE (tl_query's) or TL_ENTAILMENT_RDFS, the default graph's
 * RDFS closure computed at query time. Any other value fails with TL_ERR_INVALID_ARGUMENT. */
#define TL_ENTAILMENT_SIMPLE 0
#define TL_ENTAILMENT_RDFS 1
int32_t tl_query_with_entailment(const tl_db *db, const char *sparql, size_t len, int32_t entailment, tl_result **out, tl_error **err);

/* Query limits (#110). An interrupt stops queries from outside them: tl_interrupt_cancel works from any thread, also
 * while a query runs with it, and is final, so use a new interrupt per query. timeout_ms 0 means no deadline;
 * otherwise a query stops timeout_ms after tl_interrupt_new. tl_interrupt_free must not race with its use. */
typedef struct tl_interrupt tl_interrupt;
int32_t tl_interrupt_new(uint64_t timeout_ms, tl_interrupt **out, tl_error **err);
void tl_interrupt_cancel(const tl_interrupt *interrupt);
void tl_interrupt_free(tl_interrupt *interrupt);
/* tl_query_with_entailment under limits: interrupt (may be NULL) gives TL_ERR_INTERRUPTED when cancelled or past its
 * deadline, and memory_bytes (0: unlimited) bounds the working memory, TL_ERR_RESOURCE_LIMIT past it. Neither leaves
 * a partial result. */
int32_t tl_query_with_limits(const tl_db *db, const char *sparql, size_t len, int32_t entailment, const tl_interrupt *interrupt, uint64_t memory_bytes, tl_result **out, tl_error **err);
/* SHACL Core validation of a data document against a shapes document, each TL_FORMAT_TURTLE or TL_FORMAT_NTRIPLES.
 * On success *conforms is 1 or 0 and *report is the report graph (TL_RESULT_TRIPLES; free it with tl_result_free).
 * Data that violates the shapes is a success with *conforms 0. A document that does not parse is TL_ERR_SYNTAX, shapes
 * SHACL rejects are TL_ERR_SHAPES. */
#define TL_FORMAT_TURTLE 0
#define TL_FORMAT_NTRIPLES 1
int32_t tl_validate(const char *data, size_t data_len, int32_t data_format, const char *shapes, size_t shapes_len,
                    int32_t shapes_format, int32_t *conforms, tl_result **report, tl_error **err);
int32_t tl_update(const tl_db *db, const char *sparql, size_t len, uint64_t *changed, tl_error **err);
int32_t tl_load(const tl_db *db, const char *path, size_t len, uint64_t *loaded, tl_error **err);
int32_t tl_compact(const tl_db *db, tl_error **err);
int32_t tl_stats_ready(const tl_db *db);
/* Loads an official extension. Unknown or unavailable names return TL_ERR_UNSUPPORTED. */
int32_t tl_load_extension(const tl_db *db, const char *name, size_t len, tl_error **err);
/* Returns name, compiled_in and loaded columns. Release the result with tl_result_free. */
int32_t tl_list_extensions(const tl_db *db, tl_result **out, tl_error **err);

/* SELECT solutions as an Arrow C Stream written into caller-allocated *out.
 * - out must be non-null (checked before the query runs); on failure *out is left untouched.
 * - Schema: one nullable struct column per projected variable, fields kind (utf8: "iri" | "bnode" | "literal"),
 *   value (large_utf8, lexical form), datatype (utf8, set for literals), language (utf8, set for language-tagged
 *   literals); a null struct is an unbound cell. A zero-variable SELECT yields zero-column batches whose length is
 *   the solution count.
 * - Batches hold at most 8192 solutions; this bounds rows per batch, not bytes.
 * - End of stream: get_next returns 0 and marks the output ArrowArray released (its release callback is NULL;
 *   the output pointer itself is not). Errors: nonzero errno; get_last_error is valid until the next call on the
 *   stream or its release. After a failed get_next, every later get_next returns the same error. No panic crosses
 *   get_schema, get_next, get_last_error or the stream's release; the release callbacks of returned batches and
 *   schemas only free their buffers.
 * - Single consumer: pulls must be serialized. The stream owns its snapshot and stays valid after tl_close; each
 *   returned batch and schema is independently owned and stays valid after the stream is released.
 * - ASK and CONSTRUCT return TL_ERR_UNSUPPORTED. The query's term IDs are materialized before the first batch. */
int32_t tl_query_arrow(const tl_db *db, const char *sparql, size_t len, struct ArrowArrayStream *out, tl_error **err);

const tl_result_view *tl_result_get_view(const tl_result *r);
void tl_result_free(tl_result *r);

int32_t tl_error_kind(const tl_error *e);
const uint8_t *tl_error_message(const tl_error *e, size_t *len);
void tl_error_free(tl_error *e);

#ifdef __cplusplus
}
#endif
#endif
