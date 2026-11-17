/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

/* Runs one benchmark script in QuickJS or in Nova, embedded the way the
 * widget sandbox embeds its engine: a heap limit, an interrupt callback that
 * reads a clock, a 6 MB stack limit on an 8 MB thread, and two host
 * functions (`__now()` in milliseconds, `__emit(text)` to stdout).
 *
 *   bench qjs|nova PRELUDE.js CASE.js SELECT [HEAP_BYTES]
 *
 * SELECT is the index of the case to run, or -1 to list the cases of the
 * file. HEAP_BYTES defaults to 8 MB; 0 means no limit. One process runs one
 * case, so no case sees another's heap. A script the engine ends prints
 * {"fatal": "..."}. */

#include <pthread.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "nova_engine.h"
#include "quickjs.h"

#define STACK_BYTES ((size_t)8 << 20)
#define JS_STACK_BYTES ((size_t)6 << 20)
#define DEADLINE_MS 60000.0

static double now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec * 1000.0 + (double)ts.tv_nsec / 1e6;
}

static double deadline;
static const char *engine_name;
static char *source;
static size_t source_len;
static size_t heap_bytes = (size_t)8 << 20;

static void print_json_string(const char *text, size_t len) {
    putchar('"');
    for (size_t i = 0; i < len; i++) {
        unsigned char c = (unsigned char)text[i];
        if (c == '"' || c == '\\') printf("\\%c", c);
        else if (c < 0x20) printf("\\u%04x", c);
        else putchar(c);
    }
    putchar('"');
}

static void fatal(const char *text, size_t len) {
    printf("{\"fatal\": ");
    print_json_string(text, len);
    printf("}\n");
}

/* QuickJS */

static JSValue qjs_now(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv) {
    (void)this_val; (void)argc; (void)argv;
    return JS_NewFloat64(ctx, now_ms());
}

static JSValue qjs_emit(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv) {
    (void)this_val;
    if (argc > 0) {
        const char *text = JS_ToCString(ctx, argv[0]);
        if (text) {
            puts(text);
            JS_FreeCString(ctx, text);
        }
    }
    return JS_UNDEFINED;
}

static int qjs_interrupt(JSContext *ctx, void *opaque) {
    (void)ctx; (void)opaque;
    return now_ms() > deadline;
}

static void run_qjs(void) {
    JSRuntime *rt = JS_NewRuntime();
    if (heap_bytes) JS_SetMemoryLimit(rt, heap_bytes);
    JS_SetMaxStackSize(rt, JS_STACK_BYTES);
    JSContext *ctx = JS_NewContext(rt);
    JS_SetInterruptHandler(rt, qjs_interrupt, NULL);
    JSValue global = JS_GetGlobalObject(ctx);
    JS_SetPropertyStr(ctx, global, "__now", JS_NewCFunction(ctx, qjs_now, "__now", 0));
    JS_SetPropertyStr(ctx, global, "__emit", JS_NewCFunction(ctx, qjs_emit, "__emit", 1));
    JS_FreeValue(ctx, global);
    JSValue result = JS_Eval(ctx, source, source_len, "bench.js", JS_EVAL_TYPE_GLOBAL);
    bool failed = JS_IsException(result);
    JS_FreeValue(ctx, result);
    while (!failed) {
        JSContext *job_ctx = NULL;
        int ran = JS_ExecutePendingJob(rt, &job_ctx);
        if (ran < 0) failed = true;
        if (ran <= 0) break;
    }
    if (failed) {
        JSValue error = JS_GetException(ctx);
        const char *text = JS_ToCString(ctx, error);
        fatal(text ? text : "exception", text ? strlen(text) : 9);
        if (text) JS_FreeCString(ctx, text);
        JS_FreeValue(ctx, error);
    }
    fflush(stdout);
    /* No teardown: the process ends here, and freeing a large heap is not what is measured. */
}

/* Nova */

static void nova_host(void *context, uint32_t id, const nova_arg *args, size_t argc, nova_return *ret) {
    (void)context;
    if (id == 0) {
        nova_return_number(ret, now_ms());
    } else if (id == 1 && argc > 0 && args[0].kind == NOVA_VALUE_STRING) {
        fwrite(args[0].bytes, 1, args[0].len, stdout);
        putchar('\n');
    }
}

static bool nova_interrupt(void *context) {
    (void)context;
    return now_ms() > deadline;
}

static bool nova_check(int32_t status, nova_result *out) {
    if (status != NOVA_OK) {
        static const char *names[] = {"ok", "exception", "not a function", "interrupted", "out of memory", "dead", "refused", "invalid"};
        char text[512];
        int n = snprintf(text, sizeof text, "%s: %.*s", status >= 0 && status < 8 ? names[status] : "?", (int)(out->len < 400 ? out->len : 400), out->bytes ? (const char *)out->bytes : "");
        fatal(text, (size_t)n);
    }
    nova_result_free(out);
    return status == NOVA_OK;
}

static void run_nova(void) {
    nova_result out;
    nova_engine *engine = nova_engine_new(heap_bytes, JS_STACK_BYTES / 4 * 3, nova_host, nova_interrupt, NULL, &out);
    if (!engine) {
        fatal("no engine", 9);
        return;
    }
    nova_result_free(&out);
    nova_engine_set_out_of_memory_fatal(engine, true);
    nova_engine_register(engine, (const uint8_t *)"__now", 5, 0, 0, NOVA_ARGS_VALUES, NULL);
    nova_engine_register(engine, (const uint8_t *)"__emit", 6, 1, 1, NOVA_ARGS_VALUES, NULL);
    bool ok = nova_check(nova_engine_eval(engine, (const uint8_t *)source, source_len, false, &out), &out);
    while (ok) {
        bool ran = false;
        ok = nova_check(nova_engine_run_job(engine, &ran, &out), &out);
        if (!ran) break;
    }
    fflush(stdout);
}

static void *run(void *unused) {
    (void)unused;
    deadline = now_ms() + DEADLINE_MS;
    if (strcmp(engine_name, "qjs") == 0) run_qjs();
    else run_nova();
    return NULL;
}

static char *read_file(const char *path, size_t *len) {
    FILE *f = fopen(path, "rb");
    if (!f) {
        fprintf(stderr, "cannot read %s\n", path);
        exit(2);
    }
    fseek(f, 0, SEEK_END);
    long size = ftell(f);
    fseek(f, 0, SEEK_SET);
    char *data = malloc((size_t)size + 1);
    if (fread(data, 1, (size_t)size, f) != (size_t)size) exit(2);
    data[size] = 0;
    fclose(f);
    *len = (size_t)size;
    return data;
}

int main(int argc, char **argv) {
    if (argc < 5 || (strcmp(argv[1], "qjs") != 0 && strcmp(argv[1], "nova") != 0)) {
        fprintf(stderr, "usage: bench qjs|nova PRELUDE.js CASE.js SELECT [HEAP_BYTES]\n");
        return 2;
    }
    engine_name = argv[1];
    if (argc > 5) heap_bytes = (size_t)strtoull(argv[5], NULL, 10);
    size_t prelude_len, case_len;
    char *prelude = read_file(argv[2], &prelude_len);
    char *body = read_file(argv[3], &case_len);
    char select[64];
    int select_len = snprintf(select, sizeof select, "var __select = %d;\n", atoi(argv[4]));
    source_len = (size_t)select_len + prelude_len + 1 + case_len;
    source = malloc(source_len + 1);
    memcpy(source, select, (size_t)select_len);
    memcpy(source + select_len, prelude, prelude_len);
    source[select_len + prelude_len] = '\n';
    memcpy(source + select_len + prelude_len + 1, body, case_len);
    source[source_len] = 0;

    pthread_attr_t attr;
    pthread_attr_init(&attr);
    pthread_attr_setstacksize(&attr, STACK_BYTES);
    pthread_t thread;
    if (pthread_create(&thread, &attr, run, NULL) != 0) return 2;
    pthread_join(thread, NULL);
    return 0;
}
