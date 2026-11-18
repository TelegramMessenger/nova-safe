// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Conversions between Numbers and text: Number::toString with and without a
//! radix, Number.prototype.toFixed, StringToNumber and parseFloat. They build
//! on std's float formatting and parsing (shortest round-trip digits, exact
//! digits, correct rounding) and add only the JavaScript syntax around them.

/// The text of a Number where it is made, in the native stack: digits and
/// the few characters of JavaScript's syntax around them. Converting a
/// Number to a String is what a script that shows numbers does most; the
/// text was put together in three strings of the heap for one of them.
#[derive(Clone)]
pub(crate) struct NumberText<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

/// Room for Number::toString(x, 10): at most 25 bytes ("-0.00000" and 17
/// digits), and for the 22 of what `{:e}` writes.
pub(crate) type ShortNumberText = NumberText<32>;

/// Room for [`f64_fixed_text`]: a sign, 37 digits and a point.
pub(crate) type FixedNumberText = NumberText<48>;

impl<const N: usize> NumberText<N> {
    const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }

    /// Appends what there is room for; every caller writes less than `N`.
    fn push_bytes(&mut self, bytes: &[u8]) {
        let end = (self.len + bytes.len()).min(N);
        self.bytes[self.len..end].copy_from_slice(&bytes[..end - self.len]);
        self.len = end;
    }

    fn push(&mut self, byte: u8) {
        self.push_bytes(&[byte]);
    }

    fn push_zeros(&mut self, count: usize) {
        for _ in 0..count {
            self.push(b'0');
        }
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    /// The text: ASCII, whatever wrote it here.
    pub(crate) fn as_str(&self) -> &str {
        core::str::from_utf8(self.as_bytes()).unwrap_or_default()
    }
}

impl<const N: usize> core::fmt::Write for NumberText<N> {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        if self.len + text.len() > N {
            return Err(core::fmt::Error);
        }
        self.push_bytes(text.as_bytes());
        Ok(())
    }
}

/// The decimal digits of `value`, written at the end of `digits`: where
/// they start.
fn decimal_digits(mut value: u128, digits: &mut [u8; 40]) -> usize {
    let mut at = digits.len();
    // Nearly every value fits 64 bits, whose division is the cheap one.
    while value > u64::MAX as u128 {
        at -= 1;
        digits[at] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    let mut value = value as u64;
    loop {
        at -= 1;
        digits[at] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            return at;
        }
    }
}

/// Number::toString(x, 10) of an integral Number.
pub(crate) fn i64_text(x: i64) -> ShortNumberText {
    let mut out = ShortNumberText::new();
    if x < 0 {
        out.push(b'-');
    }
    let mut digits = [b'0'; 40];
    let at = decimal_digits(u128::from(x.unsigned_abs()), &mut digits);
    out.push_bytes(&digits[at..]);
    out
}

/// ### [6.1.6.1.20 Number::toString ( x, 10 )](https://tc39.es/ecma262/#sec-numeric-types-number-tostring)
pub(crate) fn f64_to_js_string(x: f64) -> String {
    f64_text(x).as_str().to_owned()
}

/// ### [6.1.6.1.20 Number::toString ( x, 10 )](https://tc39.es/ecma262/#sec-numeric-types-number-tostring)
///
/// The shortest digits that give x back are std's, written into the stack;
/// the syntax around them is put there too.
pub(crate) fn f64_text(x: f64) -> ShortNumberText {
    let mut out = ShortNumberText::new();
    if x.is_nan() {
        out.push_bytes(b"NaN");
        return out;
    }
    if x == 0.0 {
        out.push(b'0');
        return out;
    }
    if x.is_infinite() {
        out.push_bytes(if x > 0.0 { b"Infinity" } else { b"-Infinity" });
        return out;
    }
    let mut scientific = ShortNumberText::new();
    // At most 22 bytes ("1.7976931348623157e308"): the write cannot fail.
    let _ = core::fmt::Write::write_fmt(&mut scientific, format_args!("{:e}", x.abs()));
    let scientific = scientific.as_bytes();
    let split = scientific
        .iter()
        .position(|&byte| byte == b'e')
        .unwrap_or(scientific.len());
    let (mantissa, exponent) = scientific.split_at(split);
    let exponent = exponent.get(1..).unwrap_or_default();
    let (negative, exponent) = match exponent.split_first() {
        Some((b'-', rest)) => (true, rest),
        _ => (false, exponent),
    };
    let exponent = exponent
        .iter()
        .fold(0i32, |value, &digit| value * 10 + i32::from(digit - b'0'));
    let exponent = if negative { -exponent } else { exponent };
    let mut digits = [0u8; 17];
    let mut k = 0;
    for &byte in mantissa {
        if byte != b'.' && k < digits.len() {
            digits[k] = byte;
            k += 1;
        }
    }
    let n = exponent + 1;
    if digits[..k].last().is_some_and(|d| d % 2 == 1)
        && let Some(even) = even_tie_alternative(x.abs(), &digits[..k], n)
        && even.len() == k
    {
        digits[..k].copy_from_slice(&even);
    }
    let digits = &digits[..k];
    let k = k as i32;
    if x < 0.0 {
        out.push(b'-');
    }
    if k <= n && n <= 21 {
        out.push_bytes(digits);
        out.push_zeros((n - k) as usize);
    } else if 0 < n && n <= 21 {
        out.push_bytes(&digits[..n as usize]);
        out.push(b'.');
        out.push_bytes(&digits[n as usize..]);
    } else if -6 < n && n <= 0 {
        out.push_bytes(b"0.");
        out.push_zeros((-n) as usize);
        out.push_bytes(digits);
    } else {
        out.push_bytes(&digits[..1]);
        if k > 1 {
            out.push(b'.');
            out.push_bytes(&digits[1..]);
        }
        out.push(b'e');
        out.push(if n - 1 >= 0 { b'+' } else { b'-' });
        let mut exponent = [b'0'; 40];
        let at = decimal_digits(u128::from((n - 1).unsigned_abs()), &mut exponent);
        out.push_bytes(&exponent[at..]);
    }
    out
}

/// Most fraction digits for which [`f64_fixed_text`] has the digits.
const FIXED_TEXT_DIGITS: usize = 20;

/// The digits of Number.prototype.toFixed where one integer holds them
/// exactly: |x| under 2^53 and at most 20 fraction digits, which is what a
/// script that shows numbers asks for. x is m * 2^e with e <= 0 there, and
/// m * 10^f is under 2^120: n, the integer for which n / 10^f is the closest
/// to x and the larger one on a tie, is that product shifted down and
/// rounded by what the shift drops. None leaves any other x to
/// [`f64_to_fixed`].
pub(crate) fn f64_fixed_text(x: f64, fraction_digits: usize) -> Option<FixedNumberText> {
    if !x.is_finite() || fraction_digits > FIXED_TEXT_DIGITS {
        return None;
    }
    let bits = x.abs().to_bits();
    let biased = ((bits >> 52) & 0x7FF) as u32;
    let mantissa = bits & ((1u64 << 52) - 1);
    let (significand, shift) = if biased == 0 {
        (mantissa, 1074)
    } else if biased <= 1075 {
        (mantissa | (1u64 << 52), 1075 - biased)
    } else {
        return None;
    };
    let scaled = u128::from(significand) * 10u128.pow(fraction_digits as u32);
    let n = if shift == 0 {
        scaled
    } else if shift >= u128::BITS {
        // Under 2^120 / 2^128: nearer to 0 than to 1.
        0
    } else {
        let dropped = scaled & ((1u128 << shift) - 1);
        (scaled >> shift) + u128::from(dropped >= 1u128 << (shift - 1))
    };
    let mut digits = [b'0'; 40];
    let at = decimal_digits(n, &mut digits);
    // At least one digit before the point: the array is zeros before `at`.
    let count = (digits.len() - at).max(fraction_digits + 1);
    let (whole, fraction) = digits[digits.len() - count..].split_at(count - fraction_digits);
    let mut out = FixedNumberText::new();
    if x < 0.0 {
        out.push(b'-');
    }
    out.push_bytes(whole);
    if fraction_digits > 0 {
        out.push(b'.');
        out.push_bytes(fraction);
    }
    Some(out)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Number::toString(x, 10) as it was written before the text was put
    /// together in the stack: what [`f64_text`] is held to.
    fn f64_to_js_string_in_the_heap(x: f64) -> String {
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
        let (mantissa, exponent) = scientific.split_once('e').unwrap();
        let exponent: i32 = exponent.parse().unwrap();
        let mut digits: Vec<u8> = mantissa.bytes().filter(|&b| b != b'.').collect();
        let k = digits.len() as i32;
        let n = exponent + 1;
        if digits.last().is_some_and(|d| d % 2 == 1)
            && let Some(even) = even_tie_alternative(x.abs(), &digits, n)
        {
            digits = even;
        }
        let mut out = String::new();
        if x < 0.0 {
            out.push('-');
        }
        let digits = core::str::from_utf8(&digits).unwrap();
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

    /// Numbers of every kind a script may show, the same ones every run.
    fn samples(count: usize) -> Vec<f64> {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut all = vec![
            0.0,
            -0.0,
            0.5,
            1.5,
            2.5,
            -2.5,
            0.125,
            0.375,
            1.005,
            1.045,
            8.345,
            0.000001,
            0.0000001,
            1e21,
            1e20,
            999999999999999900000.0,
            123456789012345680000.0,
            9007199254740991.0,
            9007199254740992.0,
            9007199254740993.0,
            4503599627370496.5,
            4503599627370495.5,
            0.1 + 0.2,
            5e-324,
            2.2250738585072014e-308,
            f64::MAX,
            f64::MIN_POSITIVE,
            f64::EPSILON,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ];
        for round in 0..count {
            let bits = next();
            let value = match round % 8 {
                // Any bits at all.
                0 => f64::from_bits(bits),
                // An integer over a power of ten: what gets shown.
                1 => (bits % 2_000_000) as f64 / 10f64.powi((bits >> 32) as i32 % 8),
                // An odd integer over a power of two: the ties of toFixed.
                2 => ((bits % 100_000) | 1) as f64 / 2f64.powi(1 + (bits >> 40) as i32 % 24),
                // Around the powers of ten.
                3 => {
                    let power = 10f64.powi((bits >> 8) as i32 % 44 - 22);
                    f64::from_bits(power.to_bits().wrapping_add(bits % 7).wrapping_sub(3))
                }
                // Integers of every size, also past 2^53.
                4 => (bits >> (bits % 64)) as f64,
                // Small magnitudes, down to the subnormal.
                5 => f64::from_bits(bits % (1u64 << 62)) * 1e-300,
                // Between 0 and 1000 with every bit of the fraction.
                6 => (bits >> 11) as f64 / (1u64 << 43) as f64,
                // A product, as a script computes one.
                _ => (bits % 100_000) as f64 * 0.37,
            };
            all.push(value);
            all.push(-value);
        }
        all
    }

    #[test]
    fn the_text_of_a_number_is_what_it_was() {
        for x in samples(60_000) {
            assert_eq!(
                f64_text(x).as_str(),
                f64_to_js_string_in_the_heap(x),
                "{x:e}"
            );
            assert_eq!(f64_to_js_string(x), f64_to_js_string_in_the_heap(x));
        }
        for x in [
            0,
            1,
            -1,
            9,
            10,
            -10,
            1234567,
            12345678,
            -99999999,
            i64::from(i32::MIN),
            (1 << 53) - 1,
            -(1 << 53),
            i64::MAX,
            i64::MIN,
        ] {
            assert_eq!(i64_text(x).as_str(), x.to_string());
        }
        for x in samples(20_000) {
            if x.is_finite() {
                let x = x as i64;
                assert_eq!(i64_text(x).as_str(), x.to_string());
            }
        }
    }

    #[test]
    fn the_digits_of_to_fixed_are_what_they_were() {
        let mut taken = 0usize;
        for (at, x) in samples(40_000).into_iter().enumerate() {
            // Every count of digits for some, a few counts for the rest.
            let step = if at % 16 == 0 { 1 } else { 7 };
            for digits in (at % step..=FIXED_TEXT_DIGITS + 2).step_by(step) {
                let Some(text) = f64_fixed_text(x, digits) else {
                    assert!(
                        !x.is_finite() || digits > FIXED_TEXT_DIGITS || x.abs() >= 2f64.powi(53),
                        "{x:e} to {digits} digits was left out"
                    );
                    continue;
                };
                taken += 1;
                assert_eq!(
                    text.as_str(),
                    f64_to_fixed(x, digits),
                    "{x:e} to {digits} digits"
                );
            }
        }
        assert!(taken > 200_000, "{taken}");
        // The larger of two integers that are as close: not the even one.
        for (x, digits, expected) in [
            (0.5, 0, "1"),
            (1.5, 0, "2"),
            (2.5, 0, "3"),
            (-2.5, 0, "-3"),
            (0.125, 2, "0.13"),
            (0.375, 2, "0.38"),
            (1.005, 2, "1.00"),
            (1.045, 2, "1.04"),
            (8.345, 2, "8.35"),
            (-0.0, 2, "0.00"),
            (-0.001, 2, "-0.00"),
            (0.000001, 7, "0.0000010"),
            (5e-324, 20, "0.00000000000000000000"),
            (123.456, 0, "123"),
            (123.456, 10, "123.4560000000"),
            (9007199254740991.0, 2, "9007199254740991.00"),
            (4503599627370495.5, 0, "4503599627370496"),
        ] {
            assert_eq!(
                f64_fixed_text(x, digits).map(|text| text.as_str().to_owned()),
                Some(expected.to_owned()),
                "{x:e} to {digits} digits"
            );
        }
    }
}
