#include <stdlib.h>
#include "_cgo_export.h"

static int32_t scalar_call(void *context, const tl_scalar_args *args, tl_scalar_output *output) {
    return triplelightGoScalarCall(*(uintptr_t *)context, (tl_scalar_args *)args, output);
}

static void scalar_release(void *context) {
    triplelightGoCallbackRelease(*(uintptr_t *)context);
    free(context);
}

int32_t triplelight_go_register_function(const tl_db *db, const char *iri, size_t len,
                                        size_t arity, int32_t pure, uintptr_t handle, tl_error **err) {
    uintptr_t *context = malloc(sizeof *context);
    if (!context) {
        return TL_ERR_INTERNAL;
    }
    *context = handle;
    int32_t code = tl_register_function(db, iri, len, arity, pure, scalar_call, context, scalar_release, err);
    if (code != TL_OK) {
        free(context);
    }
    return code;
}
