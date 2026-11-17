/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

/* The C interface of nova_embed: one Nova JavaScript engine per handle.
 *
 * Rules for every function:
 * - An engine handle is used by one thread at a time; calls may come from
 *   different threads as long as they do not overlap.
 * - Input buffers are (pointer, length): read during the call and copied, never
 *   kept. A NULL pointer is allowed with length 0. Source and names are UTF-8;
 *   the strings a script receives (nova_engine_call arguments,
 *   nova_return_string) are WTF-8 as a host function's arguments are, so one
 *   it handed out goes back in unchanged; bytes that are not become U+FFFD.
 * - A nova_result passed as `out` is written whole (it may be uninitialized);
 *   its `bytes` belong to the library until nova_result_free. NULL `out` is
 *   allowed. On NOVA_OK it holds the value (NOVA_VALUE_UNDEFINED, _STRING or
 *   _NUMBER); on any other status a NOVA_VALUE_STRING with the error text
 *   (the exception's "name: message", the missing function's name, a reason).
 * - The engine calls back only into its callbacks (host function, interrupt,
 *   time zone), on the calling thread, inside a call. A call into the same
 *   engine from a callback returns NOVA_REFUSED; nova_engine_free from there
 *   is deferred until the outermost call returns.
 * - After NOVA_DEAD the engine never runs anything again; every call returns
 *   NOVA_DEAD. Free it. */

#ifndef NOVA_ENGINE_H
#define NOVA_ENGINE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum {
    NOVA_OK = 0,
    NOVA_EXCEPTION = 1,
    NOVA_NOT_A_FUNCTION = 2,
    NOVA_INTERRUPTED = 3,
    NOVA_OUT_OF_MEMORY = 4,
    NOVA_DEAD = 5,
    NOVA_REFUSED = 6,
    NOVA_INVALID = 7,
};

enum {
    NOVA_VALUE_UNDEFINED = 0,
    NOVA_VALUE_NULL = 1,
    NOVA_VALUE_STRING = 2,
    NOVA_VALUE_NUMBER = 3,
    NOVA_VALUE_BYTES = 4,
    NOVA_VALUE_OTHER = 5,
};

/* nova_arg.array_kind of a NOVA_VALUE_BYTES argument. */
enum {
    NOVA_ARRAY_INT8 = 1,
    NOVA_ARRAY_UINT8 = 2,
    NOVA_ARRAY_UINT8_CLAMPED = 3,
    NOVA_ARRAY_INT16 = 4,
    NOVA_ARRAY_UINT16 = 5,
    NOVA_ARRAY_INT32 = 6,
    NOVA_ARRAY_UINT32 = 7,
    NOVA_ARRAY_BIGINT64 = 8,
    NOVA_ARRAY_BIGUINT64 = 9,
    NOVA_ARRAY_FLOAT32 = 10,
    NOVA_ARRAY_FLOAT64 = 11,
};

/* How a host function receives typed arrays: as NOVA_VALUE_OTHER, or as a copy
 * of the bytes they view (NOVA_VALUE_BYTES). */
enum {
    NOVA_ARGS_VALUES = 0,
    NOVA_ARGS_VALUES_AND_BYTES = 1,
};

/* A host function receives at most this many arguments; later ones are
 * dropped. One engine registers at most NOVA_MAX_HOST_FUNCTIONS. */
#define NOVA_MAX_HOST_ARGUMENTS 8
#define NOVA_MAX_HOST_FUNCTIONS 32
#define NOVA_MAX_CALL_ARGS 64

typedef struct nova_engine nova_engine;
typedef struct nova_return nova_return;

typedef struct nova_result {
    int32_t kind;
    double number;
    uint8_t *bytes;
    size_t len;
} nova_result;

typedef struct nova_str {
    const uint8_t *ptr;
    size_t len;
} nova_str;

/* An argument of a host function call; `bytes` is valid during the callback
 * only. Strings are WTF-8 without a terminator (and may contain NUL): UTF-8,
 * with a lone surrogate as its own 3-byte sequence, as QuickJS hands them out.
 * Result strings (NOVA_VALUE_STRING on NOVA_OK) are WTF-8 too. */
typedef struct nova_arg {
    int32_t kind;
    int32_t array_kind;
    double number;
    const uint8_t *bytes;
    size_t len;
} nova_arg;

/* A script called the host function registered with `id`. Set the return value
 * with one nova_return_* call on `ret` before returning (none: undefined).
 * `ret` must not be used after the callback returns. */
typedef void (*nova_host_fn)(void *context, uint32_t id, const nova_arg *args, size_t argc, nova_return *ret);

/* Polled while scripts run; true ends the running script with
 * NOVA_INTERRUPTED, which try/catch/finally cannot stop. */
typedef bool (*nova_interrupt_fn)(void *context);

/* A new engine with one realm and the standard globals; NULL on failure, with
 * the reason in `error` (may be NULL). `host` is required, `interrupt` may be
 * NULL. Both must stay callable with `context` until nova_engine_free.
 * memory_limit: heap bytes (0: none); a script that goes past it ends with
 * NOVA_OUT_OF_MEMORY. stack_limit: native stack bytes JavaScript may use,
 * counted from the call into the engine (0: 1 MB); deeper calls throw a
 * catchable RangeError. Leave a quarter of the thread's stack free (about
 * 2.8 KB per JavaScript call level in a release build). The interrupt
 * callback is polled every 4096 instructions and native loop steps: keep it
 * to a flag or a cheap clock check. */
nova_engine *nova_engine_new(size_t memory_limit, size_t stack_limit, nova_host_fn host,
                             nova_interrupt_fn interrupt, void *context, nova_result *error);

/* Frees the engine (NULL is ignored). From inside one of its callbacks the
 * free waits until the outermost call into the engine returns; until then
 * every call into it is refused. */
void nova_engine_free(nova_engine *engine);

/* Seconds east of UTC of the host's local time zone at `epoch_seconds`,
 * daylight saving time included (what localtime_r's tm_gmtoff says). */
typedef int32_t (*nova_time_zone_fn)(void *context, int64_t epoch_seconds);

/* Sets the local time zone of Date (NULL: UTC, the default). Called with the
 * engine's context; it must stay callable until the engine is freed or it is
 * replaced. */
int32_t nova_engine_set_time_zone(const nova_engine *engine, nova_time_zone_fn time_zone);

/* Defines the global `name` (1 to 64 bytes) as a writable, configurable host
 * function of `length` that calls `host` with `id`. */
int32_t nova_engine_register(const nova_engine *engine, const uint8_t *name, size_t name_len, uint32_t id,
                             uint32_t length, int32_t mode, nova_result *out);

/* Evaluates `source` as a sloppy global script, or as a module (which cannot
 * import anything). */
int32_t nova_engine_eval(const nova_engine *engine, const uint8_t *source, size_t source_len, bool as_module,
                         nova_result *out);

/* A script compiled once and evaluated by any number of engines without
 * parsing it again: for the host's own trusted bundles. It is immutable, so
 * engines on different threads may evaluate the same one at once; it holds no
 * engine state and cannot be made from bytes. */
typedef struct nova_script nova_script;
/* Compiles `source` as a sloppy global script; NULL on failure, with the
 * reason in `error` (may be NULL): NOVA_EXCEPTION for a SyntaxError (the text
 * nova_engine_eval gives), NOVA_INVALID for code that cannot be shared between
 * engines (evaluate the source with nova_engine_eval instead). */
nova_script *nova_script_compile(const uint8_t *source, size_t source_len, nova_result *error);
/* Evaluates a compiled script: the same as nova_engine_eval of its source with
 * as_module false. `script` must not be freed during the call. */
int32_t nova_engine_eval_script(const nova_engine *engine, const nova_script *script, nova_result *out);
/* Frees a compiled script (NULL is ignored) once no call uses it. */
void nova_script_free(nova_script *script);
/* Keeps a private reference to the global function `name`, which
 * nova_engine_call uses from then on even after the global is deleted.
 * `*captured` (may be NULL) says whether the global was a function. */
int32_t nova_engine_capture(const nova_engine *engine, const uint8_t *name, size_t name_len, bool *captured,
                            nova_result *out);

/* Calls the captured function `name`, else the global `name`, with string
 * arguments and `this` undefined. NOVA_NOT_A_FUNCTION if it is not one. */
int32_t nova_engine_call(const nova_engine *engine, const uint8_t *name, size_t name_len, const nova_str *args,
                         size_t argc, nova_result *out);

/* Whether a promise job is queued. */
bool nova_engine_has_pending_job(const nova_engine *engine);

/* Runs the oldest queued promise job. `*ran` (may be NULL) is false when there
 * was none. A status other than NOVA_OK is the job's error; later jobs still
 * run. */
int32_t nova_engine_run_job(const nova_engine *engine, bool *ran, nova_result *out);

/* Bytes of the engine's heap in use (0 once dead). */
size_t nova_engine_memory_used(const nova_engine *engine);

/* Sets the memory limit in bytes (0: none). */
int32_t nova_engine_set_memory_limit(const nova_engine *engine, size_t bytes);

/* What one allocation the memory limit has no room for does. false, the
 * default: it throws a RangeError; caught, the script goes on just under the
 * limit, uncaught, the call ends with NOVA_OUT_OF_MEMORY. true: it ends the
 * script with NOVA_OUT_OF_MEMORY wherever it happens, as going past the limit
 * does, and no try/catch, promise reaction or getter keeps the script going. */
int32_t nova_engine_set_out_of_memory_fatal(const nova_engine *engine, bool fatal);

int32_t nova_engine_collect_garbage(const nova_engine *engine);

/* Frees result->bytes and empties the result; safe to call twice. */
void nova_result_free(nova_result *result);

/* Inside a host function callback only, on the `ret` it was given. Strings
 * and bytes are copied. */
void nova_return_null(nova_return *ret);
void nova_return_number(nova_return *ret, double number);
void nova_return_string(nova_return *ret, const uint8_t *ptr, size_t len);
void nova_return_bytes(nova_return *ret, const uint8_t *ptr, size_t len);
/* The call throws a RangeError with this message, which the script can catch,
 * instead of returning a value. */
void nova_return_range_error(nova_return *ret, const uint8_t *ptr, size_t len);

/* Which limits this build enforces. Without NOVA_LIMIT_INTERRUPT a script that
 * never ends is never stopped; without NOVA_LIMIT_HEAP the memory limit is not
 * enforced; without NOVA_LIMIT_STACK deep recursion overflows the native stack
 * (the process stops). */
enum {
    NOVA_LIMIT_INTERRUPT = 1,
    NOVA_LIMIT_HEAP = 2,
    NOVA_LIMIT_STACK = 4,
};
uint32_t nova_embed_limits(void);

/* "nova_embed <version> (nova <commit>)", static. */
const char *nova_embed_version(void);

#ifdef __cplusplus
}
#endif

#endif
