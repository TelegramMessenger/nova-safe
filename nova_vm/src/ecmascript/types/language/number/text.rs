// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Conversions between Numbers and text: Number::toString with and without a
//! radix, Number.prototype.toFixed, StringToNumber and parseFloat. They build
//! on std's float formatting and parsing (shortest round-trip digits, exact
//! digits, correct rounding) and add only the JavaScript syntax around them.

/// ### [6.1.6.1.20 Number::toString ( x, 10 )](https://tc39.es/ecma262/#sec-numeric-types-number-tostring)
pub(crate) fn f64_to_js_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x == 0.0 {
        return "0".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    let scientific = format!("{:e}", x.abs());
    let (mantissa, exponent) = scientific.split_once('e').expect("{:e} writes an exponent");
    let exponent: i32 = exponent.parse().expect("{:e} writes an integer exponent");
    let mut digits: Vec<u8> = mantissa.bytes().filter(|&b| b != b'.').collect();
    let k = digits.len() as i32;
    let n = exponent + 1;
    if digits.last().is_some_and(|d| d % 2 == 1)
        && let Some(even) = even_tie_alternative(x.abs(), &digits, n)
    {
        digits = even;
    }
    let mut out = String::with_capacity(k.max(n).max(0) as usize + 8);
    if x < 0.0 {
        out.push('-');
    }
    let digits = core::str::from_utf8(&digits).expect("ASCII digits");
    if k <= n && n <= 21 {
        out.push_str(digits);
        out.extend(core::iter::repeat_n('0', (n - k) as usize));
    } else if 0 < n && n <= 21 {
        out.push_str(&digits[..n as usize]);
        out.push('.');
        out.push_str(&digits[n as usize..]);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        out.extend(core::iter::repeat_n('0', (-n) as usize));
        out.push_str(digits);
    } else {
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if n - 1 >= 0 { '+' } else { '-' });
        out.push_str(&(n - 1).unsigned_abs().to_string());
    }
    out
}

/// The digits of Number.prototype.toFixed: n / 10^f is the closest to x,
/// the larger n on a tie; |x| >= 10^21 is ToString(x).
pub(crate) fn f64_to_fixed(x: f64, fraction_digits: usize) -> String {
    if !x.is_finite() || x.abs() >= 1e21 {
        return f64_to_js_string(x);
    }
    let magnitude = x.abs();
    let mut digits = format!("{:.*}", fraction_digits, magnitude).into_bytes();
    let bits = magnitude.to_bits();
    let biased = ((bits >> 52) & 0x7FF) as i64;
    let mantissa = bits & ((1u64 << 52) - 1);
    let (significand, exponent) = if biased == 0 {
        (mantissa, -1074i64)
    } else {
        (mantissa | (1u64 << 52), biased - 1075)
    };
    let fraction_bits = if significand == 0 {
        0
    } else {
        (-(exponent + significand.trailing_zeros() as i64)).max(0)
    };
    if fraction_bits == fraction_digits as i64 + 1 {
        let exact = format!("{:.*}", fraction_digits + 1, magnitude).into_bytes();
        let mut rounded = exact[..exact.len() - 1].to_vec();
        if rounded.last() == Some(&b'.') {
            rounded.pop();
        }
        let mut i = rounded.len();
        loop {
            if i == 0 {
                rounded.insert(0, b'1');
                break;
            }
            i -= 1;
            match rounded[i] {
                b'.' => continue,
                b'9' => rounded[i] = b'0',
                d => {
                    rounded[i] = d + 1;
                    break;
                }
            }
        }
        digits = rounded;
    }
    let mut out = String::with_capacity(digits.len() + 1);
    if x < 0.0 {
        out.push('-');
    }
    out.push_str(core::str::from_utf8(&digits).expect("ASCII digits"));
    out
}

/// The StrUnsignedDecimalLiteral part of StringToNumber, with an optional
/// sign; `Infinity` is left to the caller.
pub(crate) fn string_to_f64_decimal(s: &str) -> Option<f64> {
    if !s
        .bytes()
        .all(|b| matches!(b, b'0'..=b'9' | b'+' | b'-' | b'.' | b'e' | b'E'))
    {
        return None;
    }
    s.parse::<f64>().ok()
}

/// The value of the longest prefix of `s` that is a StrDecimalLiteral without
/// `Infinity` (parseFloat step 4), or None if there is none.
pub(crate) fn parse_float_prefix(s: &str) -> Option<f64> {
    let bytes = s.as_bytes();
    let mut i = 0;
    if let Some(b'+' | b'-') = bytes.first() {
        i += 1;
    }
    let int_start = i;
    while bytes.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
    }
    let mut digit_count = i - int_start;
    if bytes.get(i) == Some(&b'.') {
        let frac_start = i + 1;
        let mut j = frac_start;
        while bytes.get(j).is_some_and(u8::is_ascii_digit) {
            j += 1;
        }
        if digit_count > 0 || j > frac_start {
            digit_count += j - frac_start;
            i = j;
        }
    }
    if digit_count == 0 {
        return None;
    }
    if let Some(b'e' | b'E') = bytes.get(i) {
        let mut j = i + 1;
        if let Some(b'+' | b'-') = bytes.get(j) {
            j += 1;
        }
        let exp_start = j;
        while bytes.get(j).is_some_and(u8::is_ascii_digit) {
            j += 1;
        }
        if j > exp_start {
            i = j;
        }
    }
    Some(
        s[..i]
            .parse::<f64>()
            .expect("a StrDecimalLiteral prefix parses"),
    )
}

/// The shortest digits Rust prints round a tie between two candidates up; the
/// specification picks the even one. A tie needs the exact value to have one
/// significant digit more than the shortest form, ending in 5, which is only
/// possible for `m * 2^e` (m odd) with -26 <= e <= 22, small enough for u128.
fn even_tie_alternative(magnitude: f64, digits: &[u8], n: i32) -> Option<Vec<u8>> {
    let bits = magnitude.to_bits();
    let biased = ((bits >> 52) & 0x7FF) as i32;
    let fraction = bits & ((1u64 << 52) - 1);
    let (mut m, mut e) = if biased == 0 {
        (fraction, -1074)
    } else {
        (fraction | (1u64 << 52), biased - 1075)
    };
    let shift = m.trailing_zeros();
    m >>= shift;
    e += shift as i32;
    if !(-26..=22).contains(&e) {
        return None;
    }
    let exact: u128 = if e >= 0 {
        (m as u128) << e
    } else {
        (m as u128) * 5u128.pow((-e) as u32)
    };
    let exact = exact.to_string();
    let significant = exact.trim_end_matches('0').as_bytes();
    if significant.len() != digits.len() + 1 || significant.last() != Some(&b'5') {
        return None;
    }
    let k = digits.len();
    let lower: u128 = core::str::from_utf8(&significant[..k]).ok()?.parse().ok()?;
    let chosen: u128 = core::str::from_utf8(digits).ok()?.parse().ok()?;
    let alternative = if chosen == lower + 1 {
        lower
    } else if chosen == lower {
        lower + 1
    } else {
        return None;
    };
    let alternative = alternative.to_string();
    if alternative.len() != k {
        return None;
    }
    let candidate = format!("{}e{}", alternative, n - k as i32);
    (candidate.parse::<f64>().ok()? == magnitude).then(|| alternative.into_bytes())
}

const RADIX_DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

pub(crate) fn i64_to_radix_string(x: i64, radix: u32) -> String {
    let mut magnitude = x.unsigned_abs();
    let radix = radix as u64;
    let mut digits = Vec::with_capacity(65);
    loop {
        digits.push(RADIX_DIGITS[(magnitude % radix) as usize]);
        magnitude /= radix;
        if magnitude == 0 {
            break;
        }
    }
    if x < 0 {
        digits.push(b'-');
    }
    digits.reverse();
    String::from_utf8(digits).expect("ASCII digits")
}

fn binary_exponent(x: f64) -> i32 {
    let biased = ((x.to_bits() >> 52) & 0x7FF) as i32;
    if biased == 0 { -1074 } else { biased - 1075 }
}

/// Number::toString(x, radix) for a radix other than 10, as V8 writes it:
/// fraction digits until the next digit is below the precision of x, with
/// round-half-even on the last one, and the integer digits below the
/// precision of x written as zeros.
pub(crate) fn f64_to_radix_string(x: f64, radix: u32) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x == 0.0 {
        return "0".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    let value = x.abs();
    let base = radix as f64;
    let mut integer = value.floor();
    let mut fraction = value - integer;
    let mut delta = if value == f64::MAX {
        0.5 * (value - f64::from_bits(value.to_bits() - 1))
    } else {
        0.5 * (f64::from_bits(value.to_bits() + 1) - value)
    };
    delta = delta.max(f64::from_bits(1));
    let mut fraction_digits: Vec<u8> = Vec::new();
    if fraction > delta {
        loop {
            fraction *= base;
            delta *= base;
            let digit = fraction as u32;
            fraction_digits.push(digit as u8);
            fraction -= digit as f64;
            if (fraction > 0.5 || (fraction == 0.5 && digit & 1 != 0)) && fraction + delta > 1.0 {
                loop {
                    match fraction_digits.pop() {
                        None => {
                            integer += 1.0;
                            break;
                        }
                        Some(digit) if (digit as u32) + 1 < radix => {
                            fraction_digits.push(digit + 1);
                            break;
                        }
                        Some(_) => {}
                    }
                }
                break;
            }
            if delta >= fraction {
                break;
            }
        }
    }
    while fraction_digits.last() == Some(&0) {
        fraction_digits.pop();
    }
    let mut integer_digits: Vec<u8> = Vec::new();
    while binary_exponent(integer / base) > 0 {
        integer /= base;
        integer_digits.push(b'0');
    }
    loop {
        let remainder = integer % base;
        integer_digits.push(RADIX_DIGITS[remainder as usize]);
        integer = (integer - remainder) / base;
        if integer <= 0.0 {
            break;
        }
    }
    let mut out = String::with_capacity(integer_digits.len() + fraction_digits.len() + 2);
    if x < 0.0 {
        out.push('-');
    }
    out.extend(integer_digits.iter().rev().map(|&b| b as char));
    if !fraction_digits.is_empty() {
        out.push('.');
        out.extend(
            fraction_digits
                .iter()
                .map(|&d| RADIX_DIGITS[d as usize] as char),
        );
    }
    out
}
