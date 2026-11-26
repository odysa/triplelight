#include <stdlib.h>
#include "_cgo_export.h"

static int32_t aggregate_create(void *context, void **state) {
    uintptr_t *handle = malloc(sizeof *handle);
    if (!handle) {
        return TL_ERR_INTERNAL;
    }
    *handle = 0;
    int32_t code = triplelightGoAggregateCreate(*(uintptr_t *)context, handle);
    if (code == TL_OK) {
        *state = handle;
    } else {
        free(handle);
    }
    return code;
}

static int32_t aggregate_step(void *state, const tl_scalar_args *arg) {
    return triplelightGoAggregateStep(*(uintptr_t *)state, (tl_scalar_args *)arg);
}

static int32_t aggregate_finish(void *state, tl_scalar_output *output) {
    return triplelightGoAggregateFinish(*(uintptr_t *)state, output);
}

static void aggregate_release(void *opaque) {
    triplelightGoCallbackRelease(*(uintptr_t *)opaque);
    free(opaque);
}

int32_t triplelight_go_register_aggregate(const tl_db *db, const char *iri, size_t len,
                                        uintptr_t factory, tl_error **err) {
    uintptr_t *context = malloc(sizeof *context);
    if (!context) {
        return TL_ERR_INTERNAL;
    }
    *context = factory;
    const tl_aggregate_callbacks callbacks = {
        .create = aggregate_create,
        .step = aggregate_step,
        .finish = aggregate_finish,
        .drop_state = aggregate_release,
        .release_context = aggregate_release,
    };
    int32_t code = tl_register_aggregate_function(db, iri, len, &callbacks, context, err);
    if (code != TL_OK) {
        free(context);
    }
    return code;
}
