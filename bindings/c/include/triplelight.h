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

#define TL_RESULT_BINDINGS 0u
#define TL_RESULT_BOOLEAN 1u
#define TL_RESULT_TRIPLES 2u

#define TL_TERM_IRI 1
#define TL_TERM_BLANK 2
#define TL_TERM_LITERAL 3

typedef struct tl_db tl_db;
typedef struct tl_result tl_result;
typedef struct tl_error tl_error;

/* A slice of the result's byte arena. */
typedef struct tl_str { uint64_t off; uint64_t len; } tl_str;

/* One distinct term. Literals always carry a datatype IRI; `language` is empty unless language-tagged. */
typedef struct tl_term {
    uint8_t kind; /* TL_TERM_* */
    uint8_t _pad[7];
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

uint32_t tl_abi_version(void);

int32_t tl_open(const char *path, size_t path_len, tl_db **out, tl_error **err);
int32_t tl_open_in_memory(tl_db **out, tl_error **err);
void tl_close(tl_db *db);

int32_t tl_query(const tl_db *db, const char *sparql, size_t len, tl_result **out, tl_error **err);
int32_t tl_update(const tl_db *db, const char *sparql, size_t len, uint64_t *changed, tl_error **err);
int32_t tl_load(const tl_db *db, const char *path, size_t len, uint64_t *loaded, tl_error **err);
int32_t tl_compact(const tl_db *db, tl_error **err);
int32_t tl_stats_ready(const tl_db *db);

const tl_result_view *tl_result_get_view(const tl_result *r);
void tl_result_free(tl_result *r);

int32_t tl_error_kind(const tl_error *e);
const uint8_t *tl_error_message(const tl_error *e, size_t *len);
void tl_error_free(tl_error *e);

#ifdef __cplusplus
}
#endif
#endif
