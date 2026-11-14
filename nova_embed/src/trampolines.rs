// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The behaviours of host functions. A builtin function's behaviour is a
//! plain `fn` with no data, so each slot gets its own instance of
//! [`trampoline`]: the slot number is a constant in the function, and the
//! function object carries nothing a script could read or change.

use nova_vm::{
    ecmascript::{Agent, ArgumentsList, JsResult, RegularFn, Value},
    engine::GcScope,
};

pub(crate) const SLOTS: usize = 32;

fn trampoline<'gc, const SLOT: usize>(
    agent: &mut Agent,
    _this: Value,
    args: ArgumentsList,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, Value<'gc>> {
    crate::values::dispatch(SLOT, agent, args, gc)
}

const BEHAVIOURS: [RegularFn; SLOTS] = [
    trampoline::<0>,
    trampoline::<1>,
    trampoline::<2>,
    trampoline::<3>,
    trampoline::<4>,
    trampoline::<5>,
    trampoline::<6>,
    trampoline::<7>,
    trampoline::<8>,
    trampoline::<9>,
    trampoline::<10>,
    trampoline::<11>,
    trampoline::<12>,
    trampoline::<13>,
    trampoline::<14>,
    trampoline::<15>,
    trampoline::<16>,
    trampoline::<17>,
    trampoline::<18>,
    trampoline::<19>,
    trampoline::<20>,
    trampoline::<21>,
    trampoline::<22>,
    trampoline::<23>,
    trampoline::<24>,
    trampoline::<25>,
    trampoline::<26>,
    trampoline::<27>,
    trampoline::<28>,
    trampoline::<29>,
    trampoline::<30>,
    trampoline::<31>,
];

pub(crate) fn behaviour(slot: usize) -> Option<RegularFn> {
    BEHAVIOURS.get(slot).copied()
}
