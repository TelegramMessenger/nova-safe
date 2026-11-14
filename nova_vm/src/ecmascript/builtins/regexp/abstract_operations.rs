// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::ops::ControlFlow;

use oxc_allocator::Allocator;
use oxc_ast::ast::RegExpFlags;
use oxc_regular_expression::{LiteralParser, Options};

use super::data::{
    COMPILE_PEAK_BYTES_PER_BYTE, MAX_PATTERN_NESTING, PROGRAM_BYTES_PER_BYTE, RegExpExecError,
    pattern_nesting,
};
use crate::{
    ecmascript::{
        Agent, ArgumentsList, Array, BUILTIN_STRING_MEMORY, ExceptionType, Function, JsResult,
        Number, Object, PropertyKey, PropertyLookupCache, ProtoIntrinsics, RegExp, RegExpHeapData,
        RegExpLastIndex, String, TryError, TryGetResult, Value, array_create, call_function,
        handle_try_get_result, is_callable, ordinary_create_from_constructor,
        ordinary_object_create_null, throw_set_error, to_length, to_string,
        try_create_data_property_or_throw, try_get, try_result_into_js, try_to_length, unwrap_try,
        unwrap_try_get_value,
    },
    engine::{Bindable, GcScope, NoGcScope, Scopable, Scoped},
    heap::{ArenaAccessMut, CreateHeapData, DirectArenaAccessMut},
};

/// ### [22.2.3.1 RegExpCreate ( P, F )](https://tc39.es/ecma262/#sec-regexpcreate)
///
/// The abstract operation RegExpCreate takes arguments P (an ECMAScript
/// language value) and F (a String or undefined) and returns either a normal
/// completion containing an Object or a throw completion.
pub(crate) fn reg_exp_create<'a>(
    agent: &mut Agent,
    p: Scoped<Value>,
    f: Option<String>,
    gc: GcScope<'a, '_>,
) -> JsResult<'a, RegExp<'a>> {
    let f = f.map_or(Ok(RegExpFlags::empty()), |f| {
        Err(Value::from(f).scope(agent, gc.nogc()))
    });
    // 1. Let obj be ! RegExpAlloc(%RegExp%).
    let obj = agent.heap.create(RegExpHeapData::default()).bind(gc.nogc());
    // 2. Return ? RegExpInitialize(obj, P, F).
    reg_exp_initialize(agent, obj.unbind(), p, f, gc)
}

/// ### [22.2.3.1 RegExpCreate ( P, F )](https://tc39.es/ecma262/#sec-regexpcreate)
///
/// The abstract operation RegExpCreate takes arguments P (an ECMAScript
/// language value) and F (a String or undefined) and returns either a normal
/// completion containing an Object or a throw completion.
///
/// This is a variant for RegExp literal creation that cannot fail and skips
/// all of the abstract operation busy-work.
pub(crate) fn reg_exp_create_literal<'a>(
    agent: &mut Agent,
    p: String,
    f: Option<RegExpFlags>,
    gc: NoGcScope<'a, '_>,
) -> RegExp<'a> {
    // 1. Let obj be ! RegExpAlloc(%RegExp%).
    // 2. Return ? RegExpInitialize(obj, P, F).
    let f = f.unwrap_or(RegExpFlags::empty());
    let data = RegExpHeapData::new(agent, p, f);
    agent.heap.data_bytes_since_gc += data.program_bytes();
    agent.heap.create(data).bind(gc)
}

/// ### [22.2.3.2 RegExpAlloc ( newTarget )](https://tc39.es/ecma262/#sec-regexpalloc)
///
/// The abstract operation RegExpAlloc takes argument newTarget (a constructor)
/// and returns either a normal completion containing an Object or a throw
/// completion.
pub(crate) fn reg_exp_alloc<'a>(
    agent: &mut Agent,
    new_target: Function,
    gc: GcScope<'a, '_>,
) -> JsResult<'a, RegExp<'a>> {
    // 1. Let obj be ? OrdinaryCreateFromConstructor(newTarget, "%RegExp.prototype%", « [[OriginalSource]], [[OriginalFlags]], [[RegExpRecord]], [[RegExpMatcher]] »).
    let obj = RegExp::try_from(ordinary_create_from_constructor(
        agent,
        new_target,
        ProtoIntrinsics::RegExp,
        gc,
    )?)
    .unwrap();
    // 2. Perform ! DefinePropertyOrThrow(obj, "lastIndex", PropertyDescriptor { [[Writable]]: true, [[Enumerable]]: false, [[Configurable]]: false }).
    // 3. Return obj.
    Ok(obj)
}

/// ### [22.2.3.3 RegExpInitialize ( obj, pattern, flags )](https://tc39.es/ecma262/#sec-regexpinitialize)
///
/// The abstract operation RegExpInitialize takes arguments obj (an Object),
/// pattern (an ECMAScript language value), and flags (an ECMAScript language
/// value) and returns either a normal completion containing an Object or a
/// throw completion.
pub(crate) fn reg_exp_initialize<'a>(
    agent: &mut Agent,
    obj: RegExp,
    scoped_pattern: Scoped<Value>,
    scoped_flags: Result<RegExpFlags, Scoped<Value>>,
    mut gc: GcScope<'a, '_>,
) -> JsResult<'a, RegExp<'a>> {
    let obj = obj.bind(gc.nogc());
    let pattern = scoped_pattern.take(agent).bind(gc.nogc());
    let flags = scoped_flags
        .as_ref()
        .map(|f| *f)
        .map_err(|v| v.get(agent).bind(gc.nogc()));
    let quick_pattern = if pattern.is_undefined() {
        Some(None)
    } else if let Ok(pattern) = String::try_from(pattern) {
        Some(Some(pattern))
    } else {
        None
    };
    let quick_flags = match flags {
        Ok(f) => Some(Ok(f)),
        Err(flags) => {
            if flags.is_undefined() {
                Some(Ok(RegExpFlags::empty()))
            } else if let Ok(f) = String::try_from(flags) {
                Some(Err(f))
            } else {
                None
            }
        }
    };
    let (obj, p, f) = if let (Some(p), Some(f)) = (quick_pattern, quick_flags) {
        let p = p.unwrap_or(String::EMPTY_STRING);
        (obj, p, f)
    } else {
        let obj = obj.scope(agent, gc.nogc());
        let flags = scoped_flags;
        // 1. If pattern is undefined, let P be the empty String.
        let mut p = if pattern.is_undefined() {
            String::EMPTY_STRING
        } else {
            // 2. Else, let P be ? ToString(pattern).
            to_string(agent, pattern.unbind(), gc.reborrow())
                .unbind()?
                .bind(gc.nogc())
        };
        let f = match flags {
            Ok(f) => Ok(f),
            Err(flags) => {
                let flags = flags.take(agent).bind(gc.nogc());
                // 3. If flags is undefined,
                if flags.is_undefined() {
                    // let F be the empty String.
                    Ok(RegExpFlags::empty())
                } else {
                    // 4. Else, let F be ? ToString(flags).
                    let scoped_p = p.scope(agent, gc.nogc());
                    let f = to_string(agent, flags.unbind(), gc.reborrow())
                        .unbind()?
                        .bind(gc.nogc());
                    p = scoped_p.take(agent).bind(gc.nogc());
                    Err(f)
                }
            }
        };
        let obj = obj.take(agent).bind(gc.nogc());
        (obj, p, f)
    };
    // Refuse a pattern whose compile could take more memory than the heap
    // limit leaves (see COMPILE_PEAK_BYTES_PER_BYTE).
    let pattern_bytes = p.len_(agent);
    agent
        .check_heap_limit(
            pattern_bytes.saturating_mul(COMPILE_PEAK_BYTES_PER_BYTE),
            gc.nogc(),
        )
        .unbind()?;
    // 5. If F contains any code unit other than "d", "g", "i", "m", "s", "u",
    //    "v", or "y", or if F contains any code unit more than once, throw a
    //    SyntaxError exception.
    // 6. If F contains "i", let i be true; else let i be false.
    // 7. If F contains "m", let m be true; else let m be false.
    // 8. If F contains "s", let s be true; else let s be false.
    // 9. If F contains "u", let u be true; else let u be false.
    // 10. If F contains "v", let v be true; else let v be false.
    // 11. If u is true or v is true, then
    //     a. Let patternText be StringToCodePoints(P).
    // 12. Else,
    //     a. Let patternText be the result of interpreting each of P's 16-bit
    //     elements as a Unicode BMP code point. UTF-16 decoding is not applied
    //     to the elements.
    let f_str = f.map(|f| f.to_inline_string());
    let f_str = match &f_str {
        Ok(f) => f.as_str().into(),
        Err(f) => f.to_string_lossy_(agent),
    };
    let flags: Option<&str> = if f_str.is_empty() {
        None
    } else {
        Some(f_str.as_ref())
    };

    if pattern_nesting(p.as_wtf8_(agent)) > MAX_PATTERN_NESTING {
        return Err(agent.throw_exception_with_static_message(
            ExceptionType::SyntaxError,
            "regular expression is nested too deeply",
            gc.into_nogc(),
        ));
    }
    let allocator = Allocator::new();
    // 13. Let parseResult be ParsePattern(patternText, u, v).
    match LiteralParser::new(
        &allocator,
        &p.to_string_lossy_(agent),
        flags,
        Options::default(),
    )
    .parse()
    {
        Ok(_) => {
            // 15. Assert: parseResult is a Pattern Parse Node.
        }
        // 14. If parseResult is a non-empty List of SyntaxError objects,
        Err(err) => {
            // throw a SyntaxError exception.
            return Err(agent.throw_exception(
                ExceptionType::SyntaxError,
                err.message.to_string(),
                gc.into_nogc(),
            ));
        }
    };
    let f = f.unwrap_or_else(|f| parse_flags(&f.to_string_lossy_(agent)).unwrap());
    // 18. Let capturingGroupsCount be CountLeftCapturingParensWithin(parseResult).
    // 19. Let rer be the RegExp Record { [[IgnoreCase]]: i, [[Multiline]]: m, [[DotAll]]: s, [[Unicode]]: u, [[UnicodeSets]]: v, [[CapturingGroupsCount]]: capturingGroupsCount }.
    // 21. Set obj.[[RegExpMatcher]] to CompilePattern of parseResult with argument rer.
    let reg_exp_matcher = match RegExpHeapData::compile_pattern(p.as_wtf8_(agent), f) {
        Ok(matcher) => matcher,
        Err(message) => {
            return Err(agent.throw_exception(ExceptionType::SyntaxError, message, gc.into_nogc()));
        }
    };
    {
        let data = obj.get_mut(agent);
        // 16. Set obj.[[OriginalSource]] to P.
        data.original_source = p.unbind();
        // 17. Set obj.[[OriginalFlags]] to F.
        data.original_flags = f;
        // 20. Set obj.[[RegExpRecord]] to rer.
        // 21. Set obj.[[RegExpMatcher]] to CompilePattern of parseResult with argument rer.
        data.reg_exp_matcher = Some(Ok(reg_exp_matcher));
        data.program_bytes = pattern_bytes.saturating_mul(PROGRAM_BYTES_PER_BYTE);
    }
    agent.heap.data_bytes_since_gc += pattern_bytes.saturating_mul(PROGRAM_BYTES_PER_BYTE);

    // 22. Perform ? Set(obj, "lastIndex", +0𝔽, true).
    if !obj.set_last_index(agent, RegExpLastIndex::ZERO, gc.nogc()) {
        return throw_set_error(
            agent,
            BUILTIN_STRING_MEMORY.lastIndex.to_property_key(),
            gc.into_nogc(),
        )
        .into();
    }
    // 23. Return obj.
    Ok(obj.unbind())
}

fn parse_flags(f: &str) -> Option<RegExpFlags> {
    let mut flags: u8 = 0;
    for cu in f.as_bytes() {
        match cu {
            b'd' => flags |= RegExpFlags::D.bits(),
            b'g' => flags |= RegExpFlags::G.bits(),
            // 6. If F contains "i", let i be true; else let i be false.
            b'i' => flags |= RegExpFlags::I.bits(),
            // 7. If F contains "m", let m be true; else let m be false.
            b'm' => flags |= RegExpFlags::M.bits(),
            // 8. If F contains "s", let s be true; else let s be false.
            b's' => flags |= RegExpFlags::S.bits(),
            // 9. If F contains "u", let u be true; else let u be false.
            b'u' => flags |= RegExpFlags::U.bits(),
            // 10. If F contains "v", let v be true; else let v be false.
            b'v' => flags |= RegExpFlags::V.bits(),
            b'y' => flags |= RegExpFlags::Y.bits(),
            // 5. If F contains any code unit other than "d", "g", "i", "m",
            //    "s", "u", "v", or "y", or if F contains any code unit more
            //    than once, throw a SyntaxError exception.
            _ => return None,
        }
    }
    Some(RegExpFlags::from_bits_retain(flags))
}

#[inline]
pub(crate) fn require_internal_slot_reg_exp<'a>(
    agent: &mut Agent,
    o: Value,
    gc: NoGcScope<'a, '_>,
) -> JsResult<'a, RegExp<'a>> {
    // 1. If O is not an Object, throw a TypeError exception.
    let Ok(o) = Object::try_from(o) else {
        let error_message = format!(
            "{} is not an object",
            o.unbind()
                .try_string_repr(agent, gc)
                .to_string_lossy_(agent)
        );
        return Err(agent.throw_exception(ExceptionType::TypeError, error_message, gc));
    };
    require_internal_slot_reg_exp_object(agent, o, gc)
}

#[inline]
pub(crate) fn require_internal_slot_reg_exp_object<'a>(
    agent: &mut Agent,
    o: Object,
    gc: NoGcScope<'a, '_>,
) -> JsResult<'a, RegExp<'a>> {
    match o {
        // 1. Perform ? RequireInternalSlot(O, [[RegExpMatcher]]).
        Object::RegExp(reg_exp) => Ok(reg_exp.unbind().bind(gc)),
        _ => Err(agent.throw_exception_with_static_message(
            ExceptionType::TypeError,
            "Expected this to be RegExp",
            gc,
        )),
    }
}

/// ### [22.2.7.1 RegExpExec ( R, S )](https://tc39.es/ecma262/#sec-regexpexec)
///
/// The abstract operation RegExpExec takes arguments R (an Object) and S (a
/// String) and returns either a normal completion containing either an Object
/// or null, or a throw completion.
///
/// > NOTE: If a callable "exec" property is not found this algorithm falls
/// > back to attempting to use the built-in RegExp matching algorithm. This
/// > provides compatible behaviour for code written for prior editions where
/// > most built-in algorithms that use regular expressions did not perform a
/// > dynamic property lookup of "exec".
pub(crate) fn reg_exp_exec<'a>(
    agent: &mut Agent,
    r: Object,
    s: String,
    mut gc: GcScope<'a, '_>,
) -> JsResult<'a, Option<Object<'a>>> {
    let (r, s) = match reg_exp_exec_prepare(agent, r, s, gc.reborrow()) {
        ControlFlow::Continue(r) => r,
        ControlFlow::Break(result) => return result.unbind().bind(gc.into_nogc()),
    };

    // 4. Return ? RegExpBuiltinExec(R, S).
    reg_exp_builtin_exec(agent, r.unbind(), s.unbind(), gc).map(|o| o.map(|o| o.into()))
}

/// Performs steps 1-3 of RegExpExec
fn reg_exp_exec_prepare<'a>(
    agent: &mut Agent,
    r: Object,
    s: String,
    mut gc: GcScope<'a, '_>,
) -> ControlFlow<JsResult<'a, Option<Object<'a>>>, (RegExp<'a>, String<'a>)> {
    let mut s = s.bind(gc.nogc());
    let mut r = r.bind(gc.nogc());
    // 1. Let exec be ? Get(R, "exec").
    let key = BUILTIN_STRING_MEMORY.exec.to_property_key();
    let exec = try_get(
        agent,
        r,
        key,
        PropertyLookupCache::get(agent, key),
        gc.nogc(),
    );
    let exec = match exec {
        ControlFlow::Continue(TryGetResult::Unset) => Value::Undefined,
        ControlFlow::Continue(TryGetResult::Value(v)) => v,
        ControlFlow::Break(TryError::Err(e)) => {
            return ControlFlow::Break(Err(e.unbind().bind(gc.into_nogc())));
        }
        _ => {
            let scoped_r = r.scope(agent, gc.nogc());
            let scoped_s = s.scope(agent, gc.nogc());
            let exec = handle_try_get_result(
                agent,
                r.unbind(),
                BUILTIN_STRING_MEMORY.exec.to_property_key(),
                exec.unbind(),
                gc.reborrow(),
            )
            .unbind()
            .bind(gc.nogc());
            let exec = match exec {
                Ok(e) => e,
                Err(err) => return ControlFlow::Break(Err(err.unbind())),
            };
            let gc = gc.nogc();
            {
                s = scoped_s.take(agent).bind(gc);
                r = scoped_r.take(agent).bind(gc);
            }
            exec
        }
    };

    // Fast path: native RegExp object and intrinsic exec function.
    if let Object::RegExp(r) = r
        && exec
            == agent
                .current_realm_record()
                .intrinsics()
                .reg_exp_prototype_exec()
                .into()
    {
        return ControlFlow::Continue((r.unbind(), s.unbind()));
    }

    // 2. If IsCallable(exec) is true, then
    if let Some(exec) = is_callable(exec, gc.nogc()) {
        // a. Let result be ? Call(exec, R, « S »).
        let result = call_function(
            agent,
            exec.unbind(),
            r.unbind().into(),
            Some(ArgumentsList::from_mut_value(&mut s.unbind().into())),
            gc.reborrow(),
        )
        .unbind();
        let gc = gc.into_nogc();
        let result = result.bind(gc);
        let result = match result {
            Ok(r) => r,
            Err(err) => return ControlFlow::Break(Err(err)),
        };
        // b. If result is not an Object and result is not null,
        let result = if let Ok(result) = Object::try_from(result) {
            Some(result)
        } else if result.is_null() {
            None
        } else {
            // throw a TypeError exception.
            return ControlFlow::Break(Err(agent.throw_exception_with_static_message(
                ExceptionType::TypeError,
                "'exec' function result was not object or null",
                gc,
            )));
        };
        // c. Return result.
        return ControlFlow::Break(Ok(result));
    }
    // 3. Perform ? RequireInternalSlot(R, [[RegExpMatcher]]).
    let r = require_internal_slot_reg_exp_object(agent, r, gc.nogc()).unbind();
    let s = s.unbind();
    let gc = gc.into_nogc();
    let r = r.bind(gc);
    let s = s.bind(gc);
    let r = match r {
        Ok(r) => r,
        Err(err) => return ControlFlow::Break(Err(err)),
    };
    ControlFlow::Continue((r, s))
}

pub(crate) fn reg_exp_test<'a>(
    agent: &mut Agent,
    r: Object,
    s: String,
    mut gc: GcScope<'a, '_>,
) -> JsResult<'a, bool> {
    let (r, s) = match reg_exp_exec_prepare(agent, r, s, gc.reborrow()) {
        ControlFlow::Continue(r) => r,
        ControlFlow::Break(result) => {
            return result.unbind().bind(gc.into_nogc()).map(|r| r.is_some());
        }
    };
    // 4. Return ? RegExpBuiltinExec(R, S).
    reg_exp_builtin_test(agent, r.unbind(), s.unbind(), gc)
}

#[derive(Bindable)]
pub(crate) struct RegExpExecBase<'gc> {
    pub(crate) r: RegExp<'gc>,
    pub(crate) s: String<'gc>,
    pub(crate) last_index: usize,
    pub(crate) global: bool,
    pub(crate) sticky: bool,
    pub(crate) has_indices: bool,
    pub(crate) full_unicode: bool,
}

pub(crate) fn reg_exp_builtin_exec_prepare<'a>(
    agent: &mut Agent,
    r: RegExp,
    s: String,
    mut gc: GcScope<'a, '_>,
) -> JsResult<'a, RegExpExecBase<'a>> {
    let mut r = r.bind(gc.nogc());
    let mut s = s.bind(gc.nogc());

    // 1. Let length be the length of S.
    // 2. Let lastIndex be ℝ(? ToLength(? Get(R, "lastIndex"))).
    let mut last_index = if let Some(last_index) = r.try_get_last_index(agent) {
        last_index as usize
    } else {
        // Note: calling Get(R, "lastIndex") cannot trigger JavaScript
        // execution, as the "lastIndex" property is always an unconfigurable
        // data property of every RegExp object.
        let last_index = unwrap_try_get_value(try_get(
            agent,
            r,
            BUILTIN_STRING_MEMORY.lastIndex.to_property_key(),
            None,
            gc.nogc(),
        ));
        if let Some(last_index) =
            try_result_into_js(try_to_length(agent, last_index, gc.nogc())).unbind()?
        {
            last_index as usize
        } else {
            let scoped_r = r.scope(agent, gc.nogc());
            let scoped_s = s.scope(agent, gc.nogc());
            let last_index =
                to_length(agent, last_index.unbind(), gc.reborrow()).unbind()? as usize;
            {
                s = scoped_s.take(agent).bind(gc.nogc());
                r = scoped_r.take(agent).bind(gc.nogc());
            }
            last_index
        }
    };
    let r = r.unbind();
    let s = s.unbind();
    let gc = gc.into_nogc();
    let r = r.bind(gc);
    let s = s.bind(gc);

    // 3. Let flags be R.[[OriginalFlags]].
    let flags = r.original_flags(agent);
    // 4. If flags contains "g", let global be true; else let global be false.
    let global = (flags & RegExpFlags::G).bits() > 0;
    // 5. If flags contains "y", let sticky be true; else let sticky be false.
    let sticky = (flags & RegExpFlags::Y).bits() > 0;
    // 6. If flags contains "d", let hasIndices be true; else let hasIndices be false.
    let has_indices = (flags & RegExpFlags::D).bits() > 0;
    // 7. If global is false and sticky is false, set lastIndex to 0.
    if !global && !sticky {
        last_index = 0;
    }
    // 8. Let matcher be R.[[RegExpMatcher]].
    if let Err(err) = r
        .get_direct_mut(&mut agent.heap.regexps)
        .matcher(&agent.heap.strings)
    {
        let message = err.clone();
        return Err(agent.throw_exception(ExceptionType::SyntaxError, message, gc));
    };
    // 9. If flags contains "u" or flags contains "v", let fullUnicode be true;
    //    else let fullUnicode be false.
    let full_unicode = (flags & (RegExpFlags::U | RegExpFlags::V)).bits() > 0;
    Ok(RegExpExecBase {
        r,
        s,
        last_index,
        global,
        sticky,
        has_indices,
        full_unicode,
    })
}

/// ### [22.2.7.2 RegExpBuiltinExec ( R, S )](https://tc39.es/ecma262/#sec-regexpbuiltinexec)
///
/// The abstract operation RegExpBuiltinExec takes arguments R (an initialized
/// RegExp instance) and S (a String) and returns either a normal completion
/// containing either an Array exotic object or null, or a throw completion.
pub(crate) fn reg_exp_builtin_exec<'a>(
    agent: &mut Agent,
    r: RegExp,
    s: String,
    mut gc: GcScope<'a, '_>,
) -> JsResult<'a, Option<Array<'a>>> {
    let r = r.bind(gc.nogc());
    let s = s.bind(gc.nogc());
    let result =
        reg_exp_builtin_exec_prepare(agent, r.unbind(), s.unbind(), gc.reborrow()).unbind()?;
    let gc = gc.into_nogc();
    let RegExpExecBase {
        r,
        s,
        last_index,
        global,
        sticky,
        has_indices: _,
        full_unicode: _,
    } = result.bind(gc);
    // 1. Let length be the length of S.
    let length = s.utf16_len_(agent);
    // 10. Let matchSucceeded be false.
    // 11. If fullUnicode is true, let input be StringToCodePoints(S).
    //     Otherwise, let input be a List whose elements are the code units
    //     that are the elements of S.
    // 12. NOTE: Each element of input is considered to be a character.
    // 13. Repeat, while matchSucceeded is false,
    // a. If lastIndex > length, then
    if last_index > length {
        // i. If global is true or sticky is true, then
        if global || sticky {
            // 1. Perform ? Set(R, "lastIndex", +0𝔽, true).
            set_last_index(agent, r, RegExpLastIndex::ZERO, gc)?;
        }
        // ii. Return null.
        return Ok(None);
    }
    // b. Let inputIndex be the index into input of the character that was
    //    obtained from element lastIndex of S.
    // c. Let r be matcher(input, inputIndex).
    // d. If r is failure, then
    //    i. If sticky is true, then [...] Return null.
    //    ii. Set lastIndex to AdvanceStringIndex(S, lastIndex, fullUnicode).
    let result = match search(agent, r, s, last_index, sticky) {
        Ok(result) => result,
        Err(err) => return Err(throw_exec_error(agent, err, gc)),
    };
    let Some(full_match) = result else {
        // i. If global is true or sticky is true, then
        if global || sticky {
            // 1. Perform ? Set(R, "lastIndex", +0𝔽, true).
            set_last_index(agent, r, RegExpLastIndex::ZERO, gc)?;
        }
        // ii. Return null.
        return Ok(None);
    };
    let last_index = full_match.start();
    // e. Else,
    // i. Assert: r is a MatchState.
    // ii. Set matchSucceeded to true.
    // 14. Let e be r.[[EndIndex]].
    // 15. If fullUnicode is true, set e to GetStringIndex(S, e).
    let e = full_match.end();
    // 16. If global is true or sticky is true, then
    if global || sticky {
        // a. Perform ? Set(R, "lastIndex", 𝔽(e), true).
        set_last_index(agent, r, e.into(), gc)?;
    }
    // 17. Let n be the number of elements in r.[[Captures]].
    let n = full_match.captures.len() + 1;
    // 19. Assert: n < 2**32 - 1.
    debug_assert!(n < 2usize.pow(32) - 1);
    let has_group_name = !full_match.group_names().is_empty();
    // 20. Let A be ! ArrayCreate(n + 1).
    // Note: we use n because it already contains the full-match group in it.
    let a = array_create(agent, n, n, None, gc)?;
    // 21. Assert: The mathematical value of A's "length" property is n + 1.
    debug_assert_eq!(a.len(agent) as usize, n);
    // 22. Perform ! CreateDataPropertyOrThrow(A, "index", 𝔽(lastIndex)).
    unwrap_try(try_create_data_property_or_throw(
        agent,
        a,
        BUILTIN_STRING_MEMORY.index.to_property_key(),
        Number::try_from(last_index).unwrap().into(),
        None,
        gc,
    ));
    let input = String::from_static_str(agent, "input", gc).to_property_key();
    // 23. Perform ! CreateDataPropertyOrThrow(A, "input", S).
    unwrap_try(try_create_data_property_or_throw(
        agent,
        a,
        input,
        s.into(),
        None,
        gc,
    ));
    // 24. Let match be the Match Record { [[StartIndex]]: lastIndex, [[EndIndex]]: e }.
    // 25. Let indices be a new empty List.
    // 26. Let groupNames be a new empty List.
    // 27. Append match to indices.
    // 28. Let matchedSubstr be GetMatchString(S, match).
    // 29. Perform ! CreateDataPropertyOrThrow(A, "0", matchedSubstr).
    // 30. If R contains any GroupName, then
    let groups = if has_group_name {
        // a. Let groups be OrdinaryObjectCreate(null).
        // b. Let hasGroups be true.
        Some(ordinary_object_create_null(agent, gc))
    } else {
        // 31. Else,
        // a. Let groups be undefined.
        // b. Let hasGroups be false.
        None
    };
    let key = String::from_static_str(agent, "groups", gc).to_property_key();
    // 32. Perform ! CreateDataPropertyOrThrow(A, "groups", groups).
    unwrap_try(try_create_data_property_or_throw(
        agent,
        a,
        key,
        groups.map_or(Value::Undefined, |g| g.into()),
        None,
        gc,
    ));
    // 33. Let matchedGroupNames be a new empty List.
    let mut matched_group_names: Vec<&str> = Vec::new();
    // 34. For each integer i such that 1 ≤ i ≤ n, in ascending order, do
    for i in 0..n {
        // a. Let captureI be ith element of r.[[Captures]].
        let capture_i = full_match.group(i);
        // b. If captureI is undefined, then
        //    i. Let capturedValue be undefined.
        // c. Else,
        //    v. Let capturedValue be GetMatchString(S, capture).
        let captured_value = if let Some(range) = capture_i {
            s.utf16_substring(agent, range.start, range.end, gc).into()
        } else {
            Value::Undefined
        };
        // d. Perform ! CreateDataPropertyOrThrow(A, ! ToString(𝔽(i)), capturedValue).
        unwrap_try(try_create_data_property_or_throw(
            agent,
            a,
            PropertyKey::try_from(i).unwrap(),
            captured_value,
            None,
            gc,
        ));
        // e. If the ith capture of R was defined with a GroupName, then
        let name = if i == 0 {
            None
        } else {
            full_match
                .group_names()
                .get(i - 1)
                .map(|name| name.as_ref())
                .filter(|name| !name.is_empty())
        };
        if let (Some(groups), Some(name)) = (groups, name) {
            // i. Let s be the CapturingGroupName of that GroupName.
            // ii. If matchedGroupNames contains s, then
            if matched_group_names.contains(&name) {
                // 1. Assert: capturedValue is undefined.
                // 2. Append undefined to groupNames.
                continue;
            }
            // iii. Else,
            // 1. If capturedValue is not undefined, append s to matchedGroupNames.
            if !captured_value.is_undefined() {
                matched_group_names.push(name);
            }
            // 3. Perform ! CreateDataPropertyOrThrow(groups, s, capturedValue).
            let name = String::from_str(agent, name, gc).to_property_key();
            unwrap_try(try_create_data_property_or_throw(
                agent,
                groups,
                name,
                captured_value,
                None,
                gc,
            ));
        }
    }
    // 35. If hasIndices is true, then
    //         a. Let indicesArray be MakeMatchIndicesIndexPairArray(S, indices, groupNames, hasGroups).
    //         b. Perform ! CreateDataPropertyOrThrow(A, "indices", indicesArray).
    // 36. Return A.
    Ok(Some(a))
}

pub(crate) fn reg_exp_builtin_test<'a>(
    agent: &mut Agent,
    r: RegExp,
    s: String,
    mut gc: GcScope<'a, '_>,
) -> JsResult<'a, bool> {
    let r = r.bind(gc.nogc());
    let s = s.bind(gc.nogc());
    // if (r.get(agent).original_flags & (RegExpFlags::G | RegExpFlags::S)).bits() > 0 {
    //     // We have to perform the actual matching because global and sticky
    //     // RegExp's can observe its match results from lastIndex.
    //     return reg_exp_builtin_exec(agent, r.unbind(), s.unbind(), gc).map(|a| a.is_some());
    // }
    let result =
        reg_exp_builtin_exec_prepare(agent, r.unbind(), s.unbind(), gc.reborrow()).unbind()?;
    let gc = gc.into_nogc();
    let RegExpExecBase {
        r,
        s,
        last_index,
        sticky,
        global,
        ..
    } = result.bind(gc);

    // 1. Let length be the length of S.
    let length = s.utf16_len_(agent);
    if last_index > length {
        // i. If global is true or sticky is true, then
        if global || sticky {
            // 1. Perform ? Set(R, "lastIndex", +0𝔽, true).
            set_last_index(agent, r, RegExpLastIndex::ZERO, gc)?;
        }
        // ii. Return null.
        return Ok(false);
    }
    let result = match search(agent, r, s, last_index, sticky) {
        Ok(result) => result,
        Err(err) => return Err(throw_exec_error(agent, err, gc)),
    };
    match result {
        Some(found) => {
            if global || sticky {
                set_last_index(agent, r, found.end().into(), gc)?;
            }
            Ok(true)
        }
        None => {
            if global || sticky {
                set_last_index(agent, r, RegExpLastIndex::ZERO, gc)?;
            }
            Ok(false)
        }
    }
}

/// Set(R, "lastIndex", value, true) for RegExpBuiltinExec: a TypeError when
/// lastIndex was made non-writable.
fn set_last_index<'a>(
    agent: &mut Agent,
    r: RegExp,
    value: RegExpLastIndex,
    gc: NoGcScope<'a, '_>,
) -> JsResult<'a, ()> {
    if r.set_last_index(agent, value, gc) {
        Ok(())
    } else {
        Err(throw_set_error(
            agent,
            BUILTIN_STRING_MEMORY.lastIndex.to_property_key(),
            gc,
        ))
    }
}

/// Runs `r`'s matcher on `s`, polling the host's interrupt and the heap
/// limit as `Agent::should_terminate` does while the matcher holds borrows
/// of the heap.
fn search(
    agent: &mut Agent,
    r: RegExp,
    s: String,
    last_index: usize,
    sticky: bool,
) -> Result<Option<regress::Match>, RegExpExecError> {
    let heap_size = agent.heap_size();
    let collectable = agent.heap.alloc_counter != 0 || agent.heap.data_bytes_since_gc != 0;
    let host_hooks = agent.host_hooks;
    let limits = &mut agent.limits;
    let mut poll = || limits.should_terminate_matcher(host_hooks, heap_size, collectable);
    let data = r.get_direct_mut(&mut agent.heap.regexps);
    let cached_before = data.utf16_input_bytes();
    let result = data.search(&agent.heap.strings, s, last_index, sticky, &mut poll);
    let grown = data.utf16_input_bytes().saturating_sub(cached_before);
    agent.heap.data_bytes_since_gc += grown;
    result
}

fn throw_exec_error<'a>(
    agent: &mut Agent,
    err: RegExpExecError,
    gc: NoGcScope<'a, '_>,
) -> crate::ecmascript::JsError<'a> {
    if agent.termination().is_some() {
        return agent.termination_error(gc);
    }
    let kind = match err {
        RegExpExecError::TooComplex => ExceptionType::RangeError,
        RegExpExecError::Interrupted => ExceptionType::Error,
    };
    agent.throw_exception_with_static_message(kind, err.message(), gc)
}

/// ### [22.2.7.3 AdvanceStringIndex ( S, index, unicode )](https://tc39.es/ecma262/#sec-advancestringindex)
///
/// The abstract operation AdvanceStringIndex takes arguments S (a String),
/// index (a non-negative integer), and unicode (a Boolean) and returns an
/// integer.
pub(crate) fn advance_string_index(agent: &Agent, s: String, index: usize, unicode: bool) -> usize {
    // 1. Assert: index ≤ 2**53 - 1.
    assert!(index < 2usize.pow(53));
    // 2. If unicode is false, return index + 1.
    if !unicode {
        return index + 1;
    }
    // 3. Let length be the length of S.
    let length = s.utf16_len_(agent);
    // 4. If index + 1 ≥ length, return index + 1.
    if index + 1 >= length {
        return index + 1;
    }
    // 5. Let cp be CodePointAt(S, index).
    let cp = s.code_point_at_(agent, index);
    // 6. Return index + cp.[[CodeUnitCount]].
    let code = cp.to_u32();
    if (code & !0xFFFF) > 0 {
        // Two-code-unit character.
        index + 2
    } else {
        index + 1
    }
}
