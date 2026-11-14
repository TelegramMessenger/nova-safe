/* Checks nova_engine.h against the library from C: compiled with -Wall -Werror
 * -pedantic by safe/embed/test-c-abi.sh and linked against the staticlib. */

#include "nova_engine.h"

#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    int calls;
    int interrupts;
    double last_number;
    char last_string[64];
    size_t last_bytes;
    int last_array_kind;
} host_state;

static void host(void *context, uint32_t id, const nova_arg *args, size_t argc, nova_return *ret) {
    host_state *state = context;
    state->calls++;
    for (size_t i = 0; i < argc; i++) {
        if (args[i].kind == NOVA_VALUE_NUMBER) state->last_number = args[i].number;
        if (args[i].kind == NOVA_VALUE_STRING) {
            size_t n = args[i].len < 63 ? args[i].len : 63;
            memcpy(state->last_string, args[i].bytes, n);
            state->last_string[n] = 0;
        }
        if (args[i].kind == NOVA_VALUE_BYTES) {
            state->last_bytes = args[i].len;
            state->last_array_kind = args[i].array_kind;
        }
    }
    if (id == 1) {
        static const char reply[] = "from C";
        nova_return_string(ret, (const uint8_t *)reply, sizeof reply - 1);
    } else if (id == 2) {
        static const uint8_t bytes[] = {9, 8, 7};
        nova_return_bytes(ret, bytes, sizeof bytes);
    } else if (id == 3) {
        nova_return_null(ret);
    }
}

static bool interrupt(void *context) {
    host_state *state = context;
    state->interrupts++;
    return false;
}

static int32_t eval(nova_engine *engine, const char *source, nova_result *out) {
    return nova_engine_eval(engine, (const uint8_t *)source, strlen(source), false, out);
}

static void expect_string(const nova_result *result, const char *text) {
    assert(result->kind == NOVA_VALUE_STRING);
    assert(result->len == strlen(text));
    assert(memcmp(result->bytes, text, result->len) == 0);
}

int main(void) {
    host_state state = {0};
    nova_result out;
    nova_engine *engine = nova_engine_new(32u << 20, 0, host, interrupt, &state, &out);
    assert(engine != NULL);
    nova_result_free(&out);

    assert(nova_engine_register(engine, (const uint8_t *)"__s", 3, 1, 2, NOVA_ARGS_VALUES, NULL) == NOVA_OK);
    assert(nova_engine_register(engine, (const uint8_t *)"__ab", 4, 2, 0, NOVA_ARGS_VALUES_AND_BYTES, NULL) == NOVA_OK);
    assert(nova_engine_register(engine, (const uint8_t *)"__n", 3, 3, 0, NOVA_ARGS_VALUES, NULL) == NOVA_OK);

    assert(eval(engine, "__s('hello', 4.25) + '!'", &out) == NOVA_OK);
    expect_string(&out, "from C!");
    nova_result_free(&out);
    nova_result_free(&out);
    assert(state.calls == 1 && state.last_number == 4.25 && strcmp(state.last_string, "hello") == 0);

    assert(eval(engine, "new Uint8Array(__ab(new Float64Array(3))).join('-')", &out) == NOVA_OK);
    expect_string(&out, "9-8-7");
    nova_result_free(&out);
    assert(state.last_bytes == 24 && state.last_array_kind == NOVA_ARRAY_FLOAT64);

    assert(eval(engine, "__n() === null ? 1 : 0", &out) == NOVA_OK);
    assert(out.kind == NOVA_VALUE_NUMBER && out.number == 1);

    assert(eval(engine, "function f(a, b) { return a + b; } globalThis.log = []; Promise.resolve().then(() => log.push('job'));", &out) == NOVA_OK);
    assert(nova_engine_has_pending_job(engine));
    bool ran = false;
    assert(nova_engine_run_job(engine, &ran, &out) == NOVA_OK && ran);
    assert(nova_engine_run_job(engine, &ran, &out) == NOVA_OK && !ran);

    bool captured = false;
    assert(nova_engine_capture(engine, (const uint8_t *)"f", 1, &captured, &out) == NOVA_OK && captured);
    assert(eval(engine, "delete globalThis.f", &out) == NOVA_OK);
    nova_str args[2] = {{(const uint8_t *)"a", 1}, {(const uint8_t *)"bc", 2}};
    assert(nova_engine_call(engine, (const uint8_t *)"f", 1, args, 2, &out) == NOVA_OK);
    expect_string(&out, "abc");
    nova_result_free(&out);

    assert(nova_engine_call(engine, (const uint8_t *)"missing", 7, NULL, 0, &out) == NOVA_NOT_A_FUNCTION);
    expect_string(&out, "missing");
    nova_result_free(&out);

    assert(eval(engine, "throw new RangeError('far')", &out) == NOVA_EXCEPTION);
    expect_string(&out, "RangeError: far");
    nova_result_free(&out);

    assert(nova_engine_eval(engine, NULL, 3, false, &out) == NOVA_INVALID);
    nova_result_free(&out);
    assert(nova_engine_collect_garbage(engine) == NOVA_OK);
    assert(nova_engine_set_memory_limit(engine, 16u << 20) == NOVA_OK);
    (void)nova_engine_memory_used(engine);

    static const char compiled_source[] = "var k = (globalThis.k || 0) + 1; 'k' + k";
    nova_script *script = nova_script_compile((const uint8_t *)compiled_source, sizeof compiled_source - 1, &out);
    assert(script != NULL && out.kind == NOVA_VALUE_UNDEFINED);
    assert(nova_engine_eval_script(engine, script, &out) == NOVA_OK);
    expect_string(&out, "k1");
    nova_result_free(&out);
    assert(nova_engine_eval_script(engine, script, &out) == NOVA_OK);
    expect_string(&out, "k2");
    nova_result_free(&out);
    assert(nova_engine_eval_script(engine, NULL, &out) == NOVA_INVALID);
    nova_result_free(&out);
    nova_script_free(script);
    assert(nova_script_compile((const uint8_t *)"let = = 1", 9, &out) == NULL);
    assert(out.kind == NOVA_VALUE_STRING);
    nova_result_free(&out);

    nova_engine_free(engine);
    printf("c_api ok: %s\n", nova_embed_version());
    return 0;
}
