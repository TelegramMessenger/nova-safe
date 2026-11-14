// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::time::SystemTime;

use crate::{
    ecmascript::{
        Agent, ArgumentsList, BUILTIN_STRING_MEMORY, Behaviour, Builtin,
        BuiltinIntrinsicConstructor, Date, DateValue, Function, JsResult, Number, Object,
        ProtoIntrinsics, Realm, SmallInteger, String, Value, builders::BuiltinFunctionBuilder,
        ordinary_create_from_constructor, to_number, to_primitive,
    },
    engine::{Bindable, GcScope, Scopable},
    heap::{ArenaAccessMut, IntrinsicConstructorIndexes},
};

use super::date_prototype::{
    MS_PER_MINUTE, make_date, make_day, make_full_year, make_time, to_date_string, utc,
};

pub(crate) struct DateConstructor;

impl Builtin for DateConstructor {
    const BEHAVIOUR: Behaviour = Behaviour::Constructor(Self::constructor);
    const LENGTH: u8 = 7;
    const NAME: String<'static> = BUILTIN_STRING_MEMORY.Date;
}
impl BuiltinIntrinsicConstructor for DateConstructor {
    const INDEX: IntrinsicConstructorIndexes = IntrinsicConstructorIndexes::Date;
}

struct DateNow;
impl Builtin for DateNow {
    const BEHAVIOUR: Behaviour = Behaviour::Regular(DateConstructor::now);
    const LENGTH: u8 = 0;
    const NAME: String<'static> = BUILTIN_STRING_MEMORY.now;
}
struct DateParse;
impl Builtin for DateParse {
    const BEHAVIOUR: Behaviour = Behaviour::Regular(DateConstructor::parse);
    const LENGTH: u8 = 1;
    const NAME: String<'static> = BUILTIN_STRING_MEMORY.parse;
}
struct DateUTC;
impl Builtin for DateUTC {
    const BEHAVIOUR: Behaviour = Behaviour::Regular(DateConstructor::utc);
    const LENGTH: u8 = 7;
    const NAME: String<'static> = BUILTIN_STRING_MEMORY.UTC;
}
impl DateConstructor {
    /// ### [21.4.2.1 Date ( ...values )](https://tc39.es/ecma262/#sec-date)
    fn constructor<'gc>(
        agent: &mut Agent,
        _this_value: Value,
        arguments: ArgumentsList,
        new_target: Option<Object>,
        mut gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Value<'gc>> {
        let arg0 = arguments.get(0).bind(gc.nogc());
        let arg1 = arguments.get(1).scope(agent, gc.nogc());
        let arg2 = arguments.get(2).scope(agent, gc.nogc());
        let arg3 = arguments.get(3).scope(agent, gc.nogc());
        let arg4 = arguments.get(4).scope(agent, gc.nogc());
        let arg5 = arguments.get(5).scope(agent, gc.nogc());
        let arg6 = arguments.get(6).scope(agent, gc.nogc());
        // 1. If NewTarget is undefined, then
        let Some(new_target) = new_target.bind(gc.nogc()) else {
            // a. Let now be the time value (UTC) identifying the current time.
            let now = DateValue::now();
            // b. Return ToDateString(now).
            return Ok(Value::from_string(
                agent,
                to_date_string(agent, now),
                gc.into_nogc(),
            ));
        };
        let new_target = new_target.scope(agent, gc.nogc());
        // 2. Let numberOfArgs be the number of elements in values.
        let number_of_args = arguments.len() as u32;
        let dv = match number_of_args {
            // 3. If numberOfArgs = 0, then
            0 => {
                // a. Let dv be the time value (UTC) identifying the current time.
                DateValue::now()
            }
            // 4. Else if numberOfArgs = 1, then
            1 => {
                // a. Let value be values[0].
                let value = arg0;
                // b. If value is an Object and value has a [[DateValue]] internal slot, then
                let tv = if let Value::Date(date) = value {
                    // i. Let tv be value.[[DateValue]].
                    date.date_value(agent).get_f64().unwrap_or(f64::NAN)
                }
                // c. Else,
                else {
                    // i. Let v be ? ToPrimitive(value).
                    let v = to_primitive(agent, value.unbind(), None, gc.reborrow())
                        .unbind()?
                        .bind(gc.nogc());
                    // ii. If v is a String, then
                    if let Ok(v) = String::try_from(v) {
                        // 1. Assert: The next step never returns an abrupt completion because v is a String.
                        // 2. Let tv be the result of parsing v as a date, in exactly the same manner as for the parse method (21.4.3.2).
                        parse_date::parse(agent, &v.to_string_lossy_(agent))
                    }
                    // iii. Else,
                    else {
                        // 1. Let tv be ? ToNumber(v).
                        to_number(agent, v.unbind(), gc.reborrow())
                            .unbind()?
                            .to_real(agent)
                    }
                };
                // d. Let dv be TimeClip(tv).
                DateValue::time_clip(tv)
            }
            // 5. Else,
            _ => {
                // a. Assert: numberOfArgs ≥ 2.
                assert!(number_of_args >= 2);
                // b. Let y be ? ToNumber(values[0]).
                let y = to_number(agent, arg0.unbind(), gc.reborrow())
                    .unbind()?
                    .to_real(agent);
                // c. Let m be ? ToNumber(values[1]).
                let m = to_number(agent, arg1.get(agent), gc.reborrow())
                    .unbind()?
                    .to_real(agent);
                // d. If numberOfArgs > 2, let dt be ? ToNumber(values[2]); else let dt be 1𝔽.
                let dt = if number_of_args > 2 {
                    to_number(agent, arg2.get(agent), gc.reborrow())
                        .unbind()?
                        .to_real(agent)
                } else {
                    1.0
                };
                // e. If numberOfArgs > 3, let h be ? ToNumber(values[3]); else let h be +0𝔽.
                let h = if number_of_args > 3 {
                    to_number(agent, arg3.get(agent), gc.reborrow())
                        .unbind()?
                        .to_real(agent)
                } else {
                    0.0
                };
                // f. If numberOfArgs > 4, let min be ? ToNumber(values[4]); else let min be +0𝔽.
                let min = if number_of_args > 4 {
                    to_number(agent, arg4.get(agent), gc.reborrow())
                        .unbind()?
                        .to_real(agent)
                } else {
                    0.0
                };
                // g. If numberOfArgs > 5, let s be ? ToNumber(values[5]); else let s be +0𝔽.
                let s = if number_of_args > 5 {
                    to_number(agent, arg5.get(agent), gc.reborrow())
                        .unbind()?
                        .to_real(agent)
                } else {
                    0.0
                };
                // h. If numberOfArgs > 6, let milli be ? ToNumber(values[6]); else let milli be +0𝔽.
                let milli = if number_of_args > 6 {
                    to_number(agent, arg6.get(agent), gc.reborrow())
                        .unbind()?
                        .to_real(agent)
                } else {
                    0.0
                };
                // i. Let yr be MakeFullYear(y).
                let yr = make_full_year(y);
                // j. Let finalDate be MakeDate(MakeDay(yr, m, dt), MakeTime(h, min, s, milli)).
                let final_date = make_date(make_day(yr, m, dt), make_time(h, min, s, milli));
                // k. Let dv be TimeClip(UTC(finalDate)).
                DateValue::time_clip(utc(agent, final_date))
            }
        };

        // 6. Let O be ? OrdinaryCreateFromConstructor(NewTarget, "%Date.prototype%", « [[DateValue]] »).
        let o = Date::try_from(ordinary_create_from_constructor(
            agent,
            Function::try_from(new_target.get(agent)).unwrap(),
            ProtoIntrinsics::Date,
            gc,
        )?)
        .unwrap();
        // 7. Set O.[[DateValue]] to dv.
        o.get_mut(agent).date = dv;
        // 8. Return O.
        Ok(o.into())
    }

    /// ### [21.4.3.1 Date.now ( )](https://tc39.es/ecma262/#sec-date.now1)
    ///
    /// This function returns the time value designating the UTC date and time of the occurrence of the call to it.
    fn now<'gc>(
        _agent: &mut Agent,
        _this_value: Value,
        _arguments: ArgumentsList,
        _gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Value<'gc>> {
        let time_value = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        assert_eq!(time_value as u64 as u128, time_value);
        let time_value = time_value as u64;
        Ok(
            Number::from(SmallInteger::try_from(time_value).expect("SystemTime is beyond range"))
                .into(),
        )
    }

    /// ### [21.4.3.2 Date.parse ( string )](https://tc39.es/ecma262/#sec-date.parse)
    ///
    /// This function applies the ToString operator to its argument. If ToString results in
    /// an abrupt completion the Completion Record is immediately returned. Otherwise,
    /// this function interprets the resulting String as a date and time; it returns a Number,
    /// the UTC time value corresponding to the date and time. The String may be interpreted as a
    /// local time, a UTC time, or a time in some other time zone, depending on the contents of
    /// the String. The function first attempts to parse the String according to the format described
    /// in Date Time String Format (21.4.1.32), including expanded years. If the String does not
    /// conform to that format the function may fall back to any implementation-specific heuristics
    /// or implementation-specific date formats. Strings that are unrecognizable or contain
    /// out-of-bounds format element values shall cause this function to return NaN.
    ///
    /// If the String conforms to the Date Time String Format, substitute values take
    /// the place of absent format elements. When the MM or DD elements are absent,
    /// "01" is used. When the HH, mm, or ss elements are absent, "00" is used.
    /// When the sss element is absent, "000" is used. When the UTC offset representation is absent,
    /// date-only forms are interpreted as a UTC time and date-time forms are interpreted as a local time.
    ///
    /// If x is any Date whose milliseconds amount is zero within a particular implementation of ECMAScript,
    /// then all of the following expressions should produce the same numeric value in that implementation,
    /// if all the properties referenced have their initial values:
    ///
    /// ```js
    /// x.valueOf()
    /// Date.parse(x.toString())
    /// Date.parse(x.toUTCString())
    /// Date.parse(x.toISOString())
    /// ```
    ///
    /// However, the expression
    ///
    /// ```js
    /// Date.parse(x.toLocaleString())
    /// ```
    ///
    /// is not required to produce the same Number value as the preceding three expressions and,
    /// in general, the value produced by this function is implementation-defined when given any
    /// Stringvalue that does not conform to the Date Time String Format (21.4.1.32) and that
    /// could not be produced in that implementation by the toString or toUTCString method.
    fn parse<'gc>(
        agent: &mut Agent,
        _this_value: Value,
        arguments: ArgumentsList,
        mut gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Value<'gc>> {
        let input = arguments
            .get(0)
            .to_string(agent, gc.reborrow())
            .unbind()?
            .bind(gc.nogc());
        let parsed = parse_date::parse(agent, &input.to_string_lossy_(agent));
        Ok(Value::from_f64(agent, parsed, gc.into_nogc()))
    }

    /// ### [21.4.3.4 Date.UTC ( year \[ , month \[ , date \[ , hours \[ , minutes \[ , seconds \[ , ms \] \] \] \] \] \] )](https://tc39.es/ecma262/#sec-date.utc)
    /// > #### Note
    /// >
    /// > This function differs from the Date constructor in two ways: it
    /// > returns a time value as a Number, rather than creating a Date,
    /// > and it interprets the arguments in UTC rather than as local time.
    fn utc<'gc>(
        agent: &mut Agent,
        _this_value: Value,
        arguments: ArgumentsList,
        mut gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Value<'gc>> {
        let year = arguments.get(0).bind(gc.nogc());
        let month = arguments.get(1).scope(agent, gc.nogc());
        let date = arguments.get(2).scope(agent, gc.nogc());
        let hours = arguments.get(3).scope(agent, gc.nogc());
        let minutes = arguments.get(4).scope(agent, gc.nogc());
        let seconds = arguments.get(5).scope(agent, gc.nogc());
        let ms = arguments.get(6).scope(agent, gc.nogc());
        // 1. Let y be ? ToNumber(year).
        let y = to_number(agent, year.unbind(), gc.reborrow())
            .unbind()?
            .to_real(agent);
        // 2. If month is present, let m be ? ToNumber(month); else let m be +0𝔽.
        let m = if arguments.len() > 1 {
            to_number(agent, month.get(agent), gc.reborrow())
                .unbind()?
                .to_real(agent)
        } else {
            0.0
        };
        // 3. If date is present, let dt be ? ToNumber(date); else let dt be 1𝔽.
        let dt = if arguments.len() > 2 {
            to_number(agent, date.get(agent), gc.reborrow())
                .unbind()?
                .to_real(agent)
        } else {
            1.0
        };
        // 4. If hours is present, let h be ? ToNumber(hours); else let h be +0𝔽.
        let h = if arguments.len() > 3 {
            to_number(agent, hours.get(agent), gc.reborrow())
                .unbind()?
                .to_real(agent)
        } else {
            0.0
        };
        // 5. If minutes is present, let min be ? ToNumber(minutes); else let min be +0𝔽.
        let min = if arguments.len() > 4 {
            to_number(agent, minutes.get(agent), gc.reborrow())
                .unbind()?
                .to_real(agent)
        } else {
            0.0
        };
        // 6. If seconds is present, let s be ? ToNumber(seconds); else let s be +0𝔽.
        let s = if arguments.len() > 5 {
            to_number(agent, seconds.get(agent), gc.reborrow())
                .unbind()?
                .to_real(agent)
        } else {
            0.0
        };
        // 7. If ms is present, let milli be ? ToNumber(ms); else let milli be +0𝔽.
        let milli = if arguments.len() > 6 {
            to_number(agent, ms.get(agent), gc.reborrow())
                .unbind()?
                .to_real(agent)
        } else {
            0.0
        };
        // 8. Let yr be MakeFullYear(y).
        let yr = make_full_year(y);
        // 9. Return TimeClip(MakeDate(MakeDay(yr, m, dt), MakeTime(h, min, s, milli))).
        Ok(
            DateValue::time_clip(make_date(make_day(yr, m, dt), make_time(h, min, s, milli)))
                .into(),
        )
    }

    pub(crate) fn create_intrinsic(agent: &mut Agent, realm: Realm<'static>) {
        let intrinsics = agent.get_realm_record_by_id(realm).intrinsics();
        let date_prototype = intrinsics.date_prototype();

        BuiltinFunctionBuilder::new_intrinsic_constructor::<DateConstructor>(agent, realm)
            .with_property_capacity(4)
            .with_builtin_function_property::<DateNow>()
            .with_builtin_function_property::<DateParse>()
            .with_prototype_property(date_prototype.into())
            .with_builtin_function_property::<DateUTC>()
            .build();
    }
}

/// Ported from Boa JS engine. [Source](https://github.com/boa-dev/boa/blob/13a030a0aa452e6f78e4a7e8bbc0e11b878bbd58/core/engine/src/builtins/date/utils.rs#L745)
///
/// Copyright (c) 2019 Jason Williams
mod parse_date {
    use super::*;

    /// Parse a date string according to the steps specified in [`Date.parse`][spec].
    ///
    /// We parse three different formats:
    /// - The [`Date Time String Format`][spec-format] specified in the spec: `YYYY-MM-DDTHH:mm:ss.sssZ`
    /// - The `toString` format: `Thu Jan 01 1970 00:00:00 GMT+0000`
    /// - The `toUTCString` format: `Thu, 01 Jan 1970 00:00:00 GMT`
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-date.parse
    /// [spec-format]: https://tc39.es/ecma262/#sec-date-time-string-format
    pub(crate) fn parse(agent: &Agent, date: &str) -> f64 {
        // Date Time String Format: 'YYYY-MM-DDTHH:mm:ss.sssZ'
        if let Some(dt) = DateParser::new(agent, date).parse() {
            return dt as f64;
        }

        // Every other format QuickJS accepts: `toString`
        // (`Thu Jan 01 1970 00:00:00 GMT+0000 (Zone)`), `toUTCString`
        // (`Thu, 01 Jan 1970 00:00:00 GMT`), RFC 2822 with offsets, US and
        // slash dates, AM/PM and a few zone abbreviations.
        legacy::parse(agent, date).unwrap_or(f64::NAN)
    }

    /// A port of QuickJS's `js_date_parse_isostring` and
    /// `js_date_parse_otherstring`: the strings that are not exactly the Date
    /// Time String Format are parsed as QuickJS (and, closely, V8) parse them.
    mod legacy {
        use super::*;

        const MONTH_NAMES: &[u8; 36] = b"JanFebMarAprMayJunJulAugSepOctNovDec";

        const TZ_ABBREVIATIONS: [(&str, i32); 18] = [
            ("GMT", 0),
            ("UTC", 0),
            ("UT", 0),
            ("Z", 0),
            ("EDT", -4 * 60),
            ("EST", -5 * 60),
            ("CDT", -5 * 60),
            ("CST", -6 * 60),
            ("MDT", -6 * 60),
            ("MST", -7 * 60),
            ("PDT", -7 * 60),
            ("PST", -8 * 60),
            ("WET", 0),
            ("WEST", 60),
            ("CET", 60),
            ("CEST", 2 * 60),
            ("EET", 2 * 60),
            ("EEST", 3 * 60),
        ];

        /// The fields QuickJS fills: year, month (0-based once parsed), day,
        /// hours, minutes, seconds, milliseconds, the zone offset in minutes.
        #[derive(Default)]
        struct Fields {
            year: i64,
            month: i64,
            day: i64,
            hour: i64,
            minute: i64,
            second: i64,
            millisecond: i64,
            offset: i64,
            is_local: bool,
        }

        /// The input as bytes, with a 0 after the end, as QuickJS sees it:
        /// white space other than U+2028 and U+2029 is a space, U+2212 MINUS
        /// SIGN is '-' and other code units above 255 are 'x'. QuickJS looks
        /// at the first 127 code units.
        struct Input {
            bytes: Vec<u8>,
        }

        impl Input {
            fn new(date: &str) -> Self {
                let mut bytes = Vec::with_capacity(date.len().min(127) + 1);
                for unit in date.encode_utf16().take(127) {
                    let byte = match unit {
                        0x09..=0x0D
                        | 0x20
                        | 0xA0
                        | 0x1680
                        | 0x2000..=0x200A
                        | 0x202F
                        | 0x205F
                        | 0x3000
                        | 0xFEFF => b' ',
                        0x2212 => b'-',
                        0..=0xFF => unit as u8,
                        _ => b'x',
                    };
                    bytes.push(byte);
                }
                bytes.push(0);
                Self { bytes }
            }

            fn at(&self, p: usize) -> u8 {
                self.bytes.get(p).copied().unwrap_or(0)
            }

            fn skip_char(&self, p: &mut usize, c: u8) -> bool {
                if self.at(*p) == c {
                    *p += 1;
                    true
                } else {
                    false
                }
            }

            fn skip_spaces(&self, p: &mut usize) -> u8 {
                while self.at(*p) == b' ' {
                    *p += 1;
                }
                self.at(*p)
            }

            fn skip_separators(&self, p: &mut usize) {
                while matches!(self.at(*p), b'-' | b'/' | b'.' | b',') {
                    *p += 1;
                }
            }

            fn skip_until(&self, p: &mut usize, stop: &[u8]) {
                loop {
                    let c = self.at(*p);
                    if c == 0 || stop.contains(&c) {
                        return;
                    }
                    *p += 1;
                }
            }

            /// A run of digits (at most `max_digits` when not 0, at most
            /// 999999999 in value).
            fn digits(&self, p: &mut usize, min_digits: usize, max_digits: usize) -> Option<i64> {
                let start = *p;
                let mut q = *p;
                let mut value = 0i64;
                while self.at(q).is_ascii_digit() {
                    if value >= 100_000_000 {
                        return None;
                    }
                    value = value * 10 + i64::from(self.at(q) - b'0');
                    q += 1;
                    if q - start == max_digits {
                        break;
                    }
                }
                if q - start < min_digits {
                    return None;
                }
                *p = q;
                Some(value)
            }

            /// An optional fraction of a second, truncated to milliseconds.
            fn milliseconds(&self, p: &mut usize) -> Option<i64> {
                let c = self.at(*p);
                if c != b'.' && c != b',' {
                    return None;
                }
                let start = *p + 1;
                let mut q = start;
                let mut mul = 100;
                let mut ms = 0;
                while self.at(q).is_ascii_digit() {
                    ms += i64::from(self.at(q) - b'0') * mul;
                    mul /= 10;
                    q += 1;
                    if q - start == 9 {
                        break;
                    }
                }
                if q > start {
                    *p = q;
                    Some(ms)
                } else {
                    None
                }
            }

            fn tz_offset(&self, p: &mut usize, strict: bool) -> Option<i64> {
                let mut q = *p;
                let sign = self.at(q);
                q += 1;
                let offset = if sign == b'+' || sign == b'-' {
                    let start = q;
                    let mut hh = self.digits(&mut q, 1, 0)?;
                    let mut n = q - start;
                    if strict && n != 2 && n != 4 {
                        return None;
                    }
                    while n > 4 {
                        n -= 2;
                        hh /= 100;
                    }
                    let mm = if n > 2 {
                        let mm = hh % 100;
                        hh /= 100;
                        mm
                    } else if self.skip_char(&mut q, b':') {
                        self.digits(&mut q, 2, 2)?
                    } else {
                        0
                    };
                    if hh > 23 || mm > 59 {
                        return None;
                    }
                    let offset = hh * 60 + mm;
                    if sign == b'+' { offset } else { -offset }
                } else if sign == b'Z' {
                    0
                } else {
                    return None;
                };
                *p = q;
                Some(offset)
            }

            fn matches(&self, p: &mut usize, word: &str) -> bool {
                let mut q = *p;
                for &b in word.as_bytes() {
                    if !self.at(q).eq_ignore_ascii_case(&b) {
                        return false;
                    }
                    q += 1;
                }
                *p = q;
                true
            }

            fn month(&self, p: &mut usize) -> Option<i64> {
                for (n, name) in MONTH_NAMES.chunks(3).enumerate() {
                    if (0..3).all(|i| self.at(*p + i).eq_ignore_ascii_case(&name[i])) {
                        *p += 3;
                        return Some(n as i64 + 1);
                    }
                }
                None
            }

            fn tz_abbreviation(&self, p: &mut usize) -> Option<i64> {
                TZ_ABBREVIATIONS
                    .iter()
                    .find(|(name, _)| self.matches(p, name))
                    .map(|&(_, offset)| i64::from(offset))
            }
        }

        fn two_digit_year(value: i64) -> i64 {
            value + i64::from(value < 100) * 1900 + i64::from(value < 50) * 100
        }

        /// `js_date_parse_isostring`: the Date Time String Format with any
        /// number of fraction digits.
        fn iso_string(input: &Input) -> Option<Fields> {
            let mut f = Fields {
                day: 1,
                ..Default::default()
            };
            let mut p = 0;
            let sign = input.at(p);
            if sign == b'-' || sign == b'+' {
                p += 1;
                let year = input.digits(&mut p, 6, 6)?;
                if sign == b'-' {
                    if year == 0 {
                        return None;
                    }
                    f.year = -year;
                } else {
                    f.year = year;
                }
            } else {
                f.year = input.digits(&mut p, 4, 4)?;
            }
            if input.skip_char(&mut p, b'-') {
                f.month = input.digits(&mut p, 2, 2)?;
                if f.month < 1 {
                    return None;
                }
                f.month -= 1;
                if input.skip_char(&mut p, b'-') {
                    f.day = input.digits(&mut p, 2, 2)?;
                    if f.day < 1 {
                        return None;
                    }
                }
            }
            if input.skip_char(&mut p, b'T') {
                f.is_local = true;
                let hour_and_minute = input.digits(&mut p, 2, 2).and_then(|hour| {
                    input.skip_char(&mut p, b':').then_some(())?;
                    Some((hour, input.digits(&mut p, 2, 2)?))
                });
                let Some((hour, minute)) = hour_and_minute else {
                    // A date followed by a 'T' and no time is invalid, and
                    // no other format is tried.
                    f.hour = 100;
                    return Some(f);
                };
                f.hour = hour;
                f.minute = minute;
                if input.skip_char(&mut p, b':') {
                    f.second = input.digits(&mut p, 2, 2)?;
                    if let Some(ms) = input.milliseconds(&mut p) {
                        f.millisecond = ms;
                    }
                }
            }
            if input.at(p) != 0 {
                f.is_local = false;
                f.offset = input.tz_offset(&mut p, true)?;
            }
            (input.at(p) == 0).then_some(f)
        }

        /// `js_date_parse_otherstring`: toString, toUTCString and the
        /// other formats QuickJS reads.
        fn other_string(input: &Input) -> Option<Fields> {
            let mut f = Fields {
                year: 2001,
                month: 1,
                day: 1,
                is_local: true,
                ..Default::default()
            };
            let mut num = [0i64; 3];
            let mut num_index = 0;
            let (mut has_year, mut has_mon, mut has_time) = (false, false, false);
            let mut p = 0;
            while input.skip_spaces(&mut p) != 0 {
                let start = p;
                let c = input.at(p);
                if c == b'+' || c == b'-' {
                    let mut q = p;
                    if has_time && let Some(offset) = input.tz_offset(&mut q, false) {
                        f.offset = offset;
                        f.is_local = false;
                        p = q;
                    } else {
                        p += 1;
                        if let Some(value) = input.digits(&mut p, 1, 0) {
                            if c == b'-' {
                                if value == 0 {
                                    return None;
                                }
                                f.year = -value;
                            } else {
                                f.year = value;
                            }
                            has_year = true;
                        }
                    }
                } else if let Some(value) = input.digits(&mut p, 1, 0) {
                    if input.skip_char(&mut p, b':') {
                        f.hour = value;
                        f.minute = input.digits(&mut p, 1, 2)?;
                        if input.skip_char(&mut p, b':') {
                            f.second = input.digits(&mut p, 1, 2)?;
                            if let Some(ms) = input.milliseconds(&mut p) {
                                f.millisecond = ms;
                            }
                        } else if input.at(p) != 0 && input.at(p) != b' ' {
                            return None;
                        }
                        has_time = true;
                    } else if p - start > 2 {
                        f.year = value;
                        has_year = true;
                    } else if !(1..=31).contains(&value) {
                        f.year = two_digit_year(value);
                        has_year = true;
                    } else {
                        if num_index == 3 {
                            return None;
                        }
                        num[num_index] = value;
                        num_index += 1;
                    }
                } else if let Some(month) = input.month(&mut p) {
                    f.month = month;
                    has_mon = true;
                    input.skip_until(&mut p, b"0123456789 -/(");
                } else if has_time && input.matches(&mut p, "PM") {
                    if f.hour != 12 {
                        f.hour += 12;
                    }
                    continue;
                } else if has_time && input.matches(&mut p, "AM") {
                    if f.hour > 12 {
                        return None;
                    }
                    if f.hour == 12 {
                        f.hour -= 12;
                    }
                    continue;
                } else if let Some(offset) = input.tz_abbreviation(&mut p) {
                    f.offset = offset;
                    f.is_local = false;
                    continue;
                } else if c == b'(' {
                    // A parenthesized phrase, such as toString's zone name.
                    let mut level = 0;
                    loop {
                        let c = input.at(p);
                        if c == 0 {
                            break;
                        }
                        p += 1;
                        if c == b'(' {
                            level += 1;
                        } else if c == b')' {
                            level -= 1;
                        }
                        if level == 0 {
                            break;
                        }
                    }
                    if level > 0 {
                        return None;
                    }
                } else if c == b')' {
                    return None;
                } else {
                    if has_year || has_mon || has_time || num_index > 0 {
                        return None;
                    }
                    // A word before the date, such as a weekday.
                    input.skip_until(&mut p, b" -/(");
                }
                input.skip_separators(&mut p);
            }
            if num_index + usize::from(has_year) + usize::from(has_mon) > 3 {
                return None;
            }
            match num_index {
                0 => {
                    if !has_year {
                        return None;
                    }
                }
                1 => {
                    if has_mon {
                        f.day = num[0];
                    } else {
                        f.month = num[0];
                    }
                }
                2 => {
                    if has_year {
                        f.month = num[0];
                        f.day = num[1];
                    } else if has_mon {
                        f.year = two_digit_year(num[1]);
                        f.day = num[0];
                    } else {
                        f.month = num[0];
                        f.day = num[1];
                    }
                }
                _ => {
                    f.year = two_digit_year(num[2]);
                    f.month = num[0];
                    f.day = num[1];
                }
            }
            if f.month < 1 || f.day < 1 {
                return None;
            }
            f.month -= 1;
            Some(f)
        }

        pub(super) fn parse(agent: &Agent, date: &str) -> Option<f64> {
            let input = Input::new(date);
            let f = iso_string(&input).or_else(|| other_string(&input))?;
            // The fields' maximum values, and 24:00:00.000 as the only 24th hour.
            if f.month > 11 || f.day > 31 || f.hour > 24 || f.minute > 59 || f.second > 59 {
                return None;
            }
            if f.hour == 24 && (f.minute != 0 || f.second != 0 || f.millisecond != 0) {
                return None;
            }
            let date = make_date(
                make_day(f.year as f64, f.month as f64, f.day as f64),
                make_time(
                    f.hour as f64,
                    f.minute as f64,
                    f.second as f64,
                    f.millisecond as f64,
                ),
            );
            let date = if f.is_local {
                utc(agent, date)
            } else {
                date - (f.offset as f64) * MS_PER_MINUTE
            };
            let clipped = DateValue::time_clip(date).get_i64()?;
            Some(clipped as f64)
        }
    }

    /// Parses a date string according to the [`Date Time String Format`][spec].
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-date-time-string-format
    struct DateParser<'a> {
        agent: &'a Agent,
        input: std::iter::Peekable<std::slice::Iter<'a, u8>>,
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
        millisecond: u32,
        offset: i64,
    }

    // Copied from https://github.com/RoDmitry/atoi_simd/blob/master/src/fallback.rs,
    // which is based on https://rust-malaysia.github.io/code/2020/07/11/faster-integer-parsing.html.
    #[doc(hidden)]
    #[allow(clippy::inline_always)]
    mod fast_atoi {
        #[inline(always)]
        pub const fn process_8(mut val: u64, len: usize) -> u64 {
            val <<= 64_usize.saturating_sub(len << 3); // << 3 - same as mult by 8
            val = (val & 0x0F0F_0F0F_0F0F_0F0F).wrapping_mul(0xA01) >> 8;
            val = (val & 0x00FF_00FF_00FF_00FF).wrapping_mul(0x64_0001) >> 16;
            (val & 0x0000_FFFF_0000_FFFF).wrapping_mul(0x2710_0000_0001) >> 32
        }

        #[inline(always)]
        pub const fn process_4(mut val: u32, len: usize) -> u32 {
            val <<= 32_usize.saturating_sub(len << 3); // << 3 - same as mult by 8
            val = (val & 0x0F0F_0F0F).wrapping_mul(0xA01) >> 8;
            (val & 0x00FF_00FF).wrapping_mul(0x64_0001) >> 16
        }
    }

    impl<'a> DateParser<'a> {
        fn new(agent: &'a Agent, s: &'a str) -> Self {
            Self {
                agent,
                input: s.as_bytes().iter().peekable(),
                year: 0,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
                millisecond: 0,
                offset: 0,
            }
        }

        fn next_expect(&mut self, expect: u8) -> Option<()> {
            self.input
                .next()
                .and_then(|c| if *c == expect { Some(()) } else { None })
        }

        fn next_ascii_digit(&mut self) -> Option<u8> {
            self.input
                .next()
                .and_then(|c| if c.is_ascii_digit() { Some(*c) } else { None })
        }

        fn next_n_ascii_digits<const N: usize>(&mut self) -> Option<[u8; N]> {
            let mut digits = [0; N];
            for digit in &mut digits {
                *digit = self.next_ascii_digit()?;
            }
            Some(digits)
        }

        fn parse_n_ascii_digits<const N: usize>(&mut self) -> Option<u64> {
            assert!(N <= 8, "parse_n_ascii_digits parses no more than 8 digits");
            if N == 0 {
                return None;
            }
            let ascii_digits = self.next_n_ascii_digits::<N>()?;
            match N {
                1..4 => {
                    // When N is small, process digits naively.
                    let mut res = 0;
                    for digit in ascii_digits {
                        res = res * 10 + u64::from(digit & 0xF);
                    }
                    Some(res)
                }
                4 => {
                    // Process digits as an u32 block.
                    let mut src = [0; 4];
                    src[..N].copy_from_slice(&ascii_digits);
                    let val = u32::from_le_bytes(src);
                    Some(u64::from(fast_atoi::process_4(val, N)))
                }
                _ => {
                    // Process digits as an u64 block.
                    let mut src = [0; 8];
                    src[..N].copy_from_slice(&ascii_digits);
                    let val = u64::from_le_bytes(src);
                    Some(fast_atoi::process_8(val, N))
                }
            }
        }

        fn finish(&mut self) -> Option<i64> {
            if self.input.peek().is_some() {
                return None;
            }
            if self.hour == 24 && (self.minute != 0 || self.second != 0 || self.millisecond != 0) {
                return None;
            }

            let date = make_date(
                make_day(self.year.into(), (self.month - 1).into(), self.day.into()),
                make_time(
                    self.hour.into(),
                    self.minute.into(),
                    self.second.into(),
                    self.millisecond.into(),
                ),
            );

            let date = date + (self.offset as f64) * MS_PER_MINUTE;

            DateValue::time_clip(date).get_i64()
        }

        fn finish_local(&mut self) -> Option<i64> {
            if self.input.peek().is_some() {
                return None;
            }
            if self.hour == 24 && (self.minute != 0 || self.second != 0 || self.millisecond != 0) {
                return None;
            }

            let date = make_date(
                make_day(self.year.into(), (self.month - 1).into(), self.day.into()),
                make_time(
                    self.hour.into(),
                    self.minute.into(),
                    self.second.into(),
                    self.millisecond.into(),
                ),
            );

            DateValue::time_clip(utc(self.agent, date)).get_i64()
        }

        fn parse(&mut self) -> Option<i64> {
            self.parse_year()?;
            match self.input.peek() {
                Some(b'T') => return self.parse_time(),
                None => return self.finish(),
                _ => {}
            }
            self.next_expect(b'-')?;
            self.month = self.parse_n_ascii_digits::<2>()? as u32;
            if self.month < 1 || self.month > 12 {
                return None;
            }
            match self.input.peek() {
                Some(b'T') => return self.parse_time(),
                None => return self.finish(),
                _ => {}
            }
            self.next_expect(b'-')?;
            self.day = self.parse_n_ascii_digits::<2>()? as u32;
            if self.day < 1 || self.day > 31 {
                return None;
            }
            match self.input.peek() {
                Some(b'T') => self.parse_time(),
                _ => self.finish(),
            }
        }

        fn parse_year(&mut self) -> Option<()> {
            if let &&sign @ (b'+' | b'-') = self.input.peek()? {
                // Consume the sign.
                self.input.next();
                let year = self.parse_n_ascii_digits::<6>()? as i32;
                let neg = sign == b'-';
                if neg && year == 0 {
                    return None;
                }
                self.year = if neg { -year } else { year };
            } else {
                self.year = self.parse_n_ascii_digits::<4>()? as i32;
            }
            Some(())
        }

        fn parse_time(&mut self) -> Option<i64> {
            self.next_expect(b'T')?;
            self.hour = self.parse_n_ascii_digits::<2>()? as u32;
            if self.hour > 24 {
                return None;
            }
            self.next_expect(b':')?;
            self.minute = self.parse_n_ascii_digits::<2>()? as u32;
            if self.minute > 59 {
                return None;
            }
            match self.input.peek() {
                Some(b':') => self.input.next(),
                None => return self.finish_local(),
                _ => {
                    self.parse_timezone()?;
                    return self.finish();
                }
            };
            self.second = self.parse_n_ascii_digits::<2>()? as u32;
            if self.second > 59 {
                return None;
            }
            match self.input.peek() {
                Some(b'.') => self.input.next(),
                None => return self.finish_local(),
                _ => {
                    self.parse_timezone()?;
                    return self.finish();
                }
            };
            self.millisecond = self.parse_n_ascii_digits::<3>()? as u32;
            if self.input.peek().is_some() {
                self.parse_timezone()?;
                self.finish()
            } else {
                self.finish_local()
            }
        }

        fn parse_timezone(&mut self) -> Option<()> {
            match self.input.next() {
                Some(b'Z') => return Some(()),
                Some(sign @ (b'+' | b'-')) => {
                    let neg = *sign == b'-';
                    let offset_hour = self.parse_n_ascii_digits::<2>()? as i64;
                    if offset_hour > 23 {
                        return None;
                    }
                    self.offset = if neg { offset_hour } else { -offset_hour } * 60;
                    if self.input.peek().is_none() {
                        return Some(());
                    }
                    self.next_expect(b':')?;
                    let offset_minute = self.parse_n_ascii_digits::<2>()? as i64;
                    if offset_minute > 59 {
                        return None;
                    }
                    self.offset += if neg { offset_minute } else { -offset_minute };
                }
                _ => return None,
            }
            Some(())
        }
    }
}
