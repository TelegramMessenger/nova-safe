// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Builtins that run user code must keep their handles rooted across it: a
//! garbage collection inside the user code compacts the heap and moves the
//! objects the builtin still holds. Each case allocates garbage before the
//! objects it uses and drops it, so that the collection run from inside the
//! callback (`gc()`) moves them, and checks the builtin's result.

use nova_vm::{
    ecmascript::{
        Agent, AgentOptions, ArgumentsList, Behaviour, BuiltinFunctionArgs, DefaultHostHooks,
        GcAgent, InternalMethods, JsResult, Object, PropertyDescriptor, PropertyKey, String, Value,
        create_builtin_function, parse_script, script_evaluation,
    },
    engine::{Bindable, GcScope},
};

fn initialize_global_object(agent: &mut Agent, global: Object, gc: GcScope) {
    fn run_gc<'gc>(
        agent: &mut Agent,
        _this: Value,
        _args: ArgumentsList,
        gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Value<'gc>> {
        agent.gc(gc);
        Ok(Value::Undefined)
    }
    let function = create_builtin_function(
        agent,
        Behaviour::Regular(run_gc),
        BuiltinFunctionArgs::new(0, "gc"),
        gc.nogc(),
    );
    let property_key = PropertyKey::from_static_str(agent, "gc", gc.nogc());
    global
        .internal_define_own_property(
            agent,
            property_key.unbind(),
            PropertyDescriptor {
                value: Some(function.unbind().into()),
                ..Default::default()
            },
            gc,
        )
        .unwrap();
}

const PRELUDE: &str = r#"
function garbage(make) { var junk = []; for (var i = 0; i < 2000; i++) junk.push(make()); return junk; }
function check(name, actual, expected) {
  if (actual !== expected) throw new Error(name + ": expected " + String(expected) + ", got " + String(actual));
}
function gcIterable(count) {
  return { [Symbol.iterator]() { var i = 0; return { next() { gc(); return { done: i >= count, value: ++i }; } }; } };
}
"#;

fn run_case(name: &str, source: &str) {
    let mut agent = GcAgent::new(AgentOptions::default(), &DefaultHostHooks);
    let create_global_object: Option<for<'a> fn(&mut Agent, GcScope<'a, '_>) -> Object<'a>> = None;
    let create_global_this_value: Option<for<'a> fn(&mut Agent, GcScope<'a, '_>) -> Object<'a>> =
        None;
    let realm = agent.create_realm(
        create_global_object,
        create_global_this_value,
        Some(initialize_global_object),
    );
    let source = format!("{PRELUDE}\n{source}\ngc();\n");
    agent.run_in_realm(&realm, |agent, mut gc| {
        let realm = agent.current_realm(gc.nogc());
        let source_text = String::from_string(agent, source, gc.nogc());
        let script = parse_script(agent, source_text, realm, false, None, gc.nogc()).unwrap();
        if let Err(err) = script_evaluation(agent, script.unbind(), gc.reborrow()) {
            panic!(
                "{name}: {}",
                err.unbind()
                    .to_string(agent, gc)
                    .to_string_lossy(agent)
                    .into_owned()
            )
        }
    });
    agent.gc();
}

#[test]
fn has_own_property_keeps_this_across_to_property_key() {
    run_case(
        "hasOwnProperty",
        r#"
var junk = garbage(() => ({ b: 1 }));
var target = { a: 1 };
junk = null;
check("hasOwnProperty", target.hasOwnProperty({ toString() { gc(); return "a"; } }), true);
"#,
    );
}

#[test]
fn property_is_enumerable_keeps_this_across_to_property_key() {
    run_case(
        "propertyIsEnumerable",
        r#"
var junk = garbage(() => ({ b: 1 }));
var target = { a: 1 };
junk = null;
check("propertyIsEnumerable", target.propertyIsEnumerable({ toString() { gc(); return "a"; } }), true);
var it = "xy"[Symbol.iterator]();
junk = garbage(() => "ab"[Symbol.iterator]());
it = "xy"[Symbol.iterator]();
junk = null;
check("iterator hasOwnProperty", it.hasOwnProperty({ toString() { gc(); return "next"; } }), false);
"#,
    );
}

#[test]
fn data_view_set_keeps_value_across_to_index() {
    run_case(
        "DataView set",
        r#"
var junk = garbage(() => ({}));
var dv = new DataView(new ArrayBuffer(16));
var value = { valueOf() { return 7; } };
var big = { valueOf() { return 9n; } };
junk = null;
dv.setInt8({ valueOf() { gc(); return 1; } }, value);
check("setInt8", dv.getInt8(1), 7);
junk = garbage(() => ({}));
value = { valueOf() { return 300; } };
big = { valueOf() { return 9n; } };
junk = null;
dv.setUint16({ valueOf() { gc(); return 2; } }, value);
check("setUint16", dv.getUint16(2), 300);
dv.setBigInt64({ valueOf() { gc(); return 8; } }, big);
check("setBigInt64", dv.getBigInt64(8), 9n);
"#,
    );
}

#[test]
fn function_constructor_keeps_arguments_across_to_string() {
    run_case(
        "Function",
        r#"
var junk = garbage(() => ({}));
var second = { toString() { return "b"; } };
var body = { toString() { return "return a + b"; } };
junk = null;
var f = Function({ toString() { gc(); return "a"; } }, second, body);
check("Function", f(1, 2), 3);
"#,
    );
}

#[test]
fn string_concat_keeps_arguments_across_to_string() {
    run_case(
        "String.prototype.concat",
        r#"
var junk = garbage(() => ({}));
var second = { toString() { return "b"; } };
junk = null;
check("concat", "x".concat({ toString() { gc(); return "a"; } }, second), "xab");
"#,
    );
}

#[test]
fn math_hypot_keeps_arguments_across_to_number() {
    run_case(
        "Math.hypot",
        r#"
var junk = garbage(() => ({}));
var second = { valueOf() { return 4; } };
junk = null;
check("hypot", Math.hypot({ valueOf() { gc(); return 3; } }, second), 5);
check("hypot numbers", Math.hypot(3, 4, 12), 13);
"#,
    );
}

#[test]
fn iterator_to_list_keeps_iterator_across_next() {
    run_case(
        "IteratorToList",
        r#"
var junk = garbage(() => function () {});
var it = gcIterable(3);
junk = null;
check("AggregateError", new AggregateError(it).errors.join(), "1,2,3");
junk = garbage(() => function () {});
it = gcIterable(3);
junk = null;
check("TypedArray.from", Uint8Array.from(it).join(), "1,2,3");
junk = garbage(() => ({}));
it = gcIterable(3);
junk = null;
check("TypedArray constructor", new Int8Array(it).join(), "1,2,3");
"#,
    );
}

#[test]
fn typed_array_survives_gc_before_its_buffer_exists() {
    run_case(
        "TypedArray from array-like",
        r#"
var junk = garbage(() => ({}));
junk = null;
var ta = new Int16Array({ get length() { gc(); return 2; }, 0: 5, 1: 6 });
check("array-like", ta.join(), "5,6");
check("buffer", ta.buffer.byteLength, 4);
"#,
    );
}

#[test]
fn error_cause_keeps_options_across_has_property() {
    run_case(
        "Error cause",
        r#"
var junk = garbage(() => new Proxy({}, {}));
var options = new Proxy({}, { has() { gc(); return true; }, get(t, k) { return k === "cause" ? 42 : undefined; } });
junk = null;
check("cause", new Error("m", options).cause, 42);
"#,
    );
}

#[test]
fn create_list_from_array_like_keeps_object_across_get() {
    run_case(
        "Function.prototype.apply",
        r#"
var junk = garbage(() => new Proxy({}, {}));
var args = new Proxy({}, { get(t, k) { gc(); return k === "length" ? 2 : "v" + k; } });
junk = null;
check("apply", (function (a, b) { return a + b; }).apply(null, args), "v0v1");
"#,
    );
}

#[test]
fn array_push_keeps_items_across_proxy_traps() {
    run_case(
        "Array.prototype.push",
        r#"
var junk = garbage(() => ({}));
var a = { x: 1 }, b = { y: 2 };
junk = null;
var recv = new Proxy([], { get(t, k) { gc(); return Reflect.get(t, k); }, set(t, k, v) { gc(); return Reflect.set(t, k, v); } });
check("push length", Array.prototype.push.call(recv, a, b), 2);
gc();
check("pushed a", recv[0], a);
check("pushed b", recv[1], b);
check("pushed a.x", recv[0].x, 1);
check("pushed b.y", recv[1].y, 2);
var plain = [1];
check("plain push", plain.push(2, { z: 3 }), 3);
check("plain pushed", plain[2].z, 3);
"#,
    );
}

#[test]
fn integrity_level_keeps_object_across_proxy_traps() {
    run_case(
        "Object.freeze / isFrozen",
        r#"
var junk = garbage(() => new Proxy({}, {}));
var handler = {
  preventExtensions(t) { gc(); return Reflect.preventExtensions(t); },
  isExtensible(t) { gc(); return Reflect.isExtensible(t); },
  ownKeys(t) { gc(); return Reflect.ownKeys(t); },
  getOwnPropertyDescriptor(t, k) { gc(); return Reflect.getOwnPropertyDescriptor(t, k); },
  defineProperty(t, k, d) { gc(); return Reflect.defineProperty(t, k, d); },
};
var frozen = new Proxy({ a: 1, b: 2 }, handler);
var sealed = new Proxy({ a: 1 }, handler);
junk = null;
check("freeze", Object.freeze(frozen), frozen);
check("isFrozen", Object.isFrozen(frozen), true);
check("seal", Object.seal(sealed), sealed);
check("isSealed", Object.isSealed(sealed), true);
check("sealed is not frozen", Object.isFrozen(sealed), false);
"#,
    );
}

#[test]
fn date_and_regexp_constructors_keep_new_target() {
    run_case(
        "Date / RegExp new.target",
        r#"
var junk = garbage(() => function () {});
class D extends Date { constructor(a, b) { super(a, b); } }
class R extends RegExp { constructor(p) { super(p); } }
junk = null;
var d = new D({ valueOf() { gc(); return 2000; } }, 1);
check("Date subclass", Object.getPrototypeOf(d), D.prototype);
check("Date value", d.getMonth(), 1);
junk = garbage(() => function () {});
class R2 extends RegExp { constructor(p) { super(p); } }
junk = null;
var r = new R2({ get [Symbol.match]() { gc(); return false; }, toString() { return "a+"; } });
check("RegExp subclass", Object.getPrototypeOf(r), R2.prototype);
check("RegExp source", r.source, "a+");
"#,
    );
}

#[test]
fn atomics_index_conversion_keeps_offset() {
    run_case(
        "Atomics",
        r#"
var junk = garbage(() => new Int32Array(new ArrayBuffer(16), 4, 2));
var ta = new Int32Array(new ArrayBuffer(32), 8, 4);
junk = null;
Atomics.store(ta, { valueOf() { gc(); return 1; } }, 42);
check("store at offset", new Int32Array(ta.buffer)[3], 42);
check("load", Atomics.load(ta, { valueOf() { gc(); return 1; } }), 42);
"#,
    );
}

#[test]
fn object_binding_keeps_reference_across_getters() {
    run_case(
        "object binding patterns",
        r#"
function closure() { var q = 1; return function () { return q; }; }
var junk = garbage(closure);
function viaVar(obj) { var { x } = obj; return function () { return x; }; }
function viaLet(obj) { let { x } = obj; return function () { return x; }; }
function viaRest(obj) { var { ...r } = obj; return function () { return r; }; }
function getter() { junk = null; gc(); return 42; }
check("var", viaVar({ get x() { return getter(); } })(), 42);
junk = garbage(closure);
check("let", viaLet({ get x() { return getter(); } })(), 42);
junk = garbage(closure);
check("rest", viaRest({ get y() { return getter(); } })().y, 42);
junk = garbage(closure);
var proxy = new Proxy({ x: 7 }, { get(t, k) { junk = null; gc(); return Reflect.get(t, k); } });
check("proxy", viaLet(proxy)(), 7);
"#,
    );
}

#[test]
fn instanceof_keeps_object_across_prototype_getter() {
    run_case(
        "instanceof",
        r#"
var junk = garbage(() => []);
var arr = [];
junk = null;
var C = () => {};
Object.defineProperty(C, "prototype", { get() { gc(); return Array.prototype; } });
check("instanceof getter", arr instanceof C, true);
junk = garbage(() => ({}));
var plain = {};
junk = null;
check("instanceof getter false", plain instanceof C, false);
check("instanceof plain", [] instanceof Array, true);
"#,
    );
}
