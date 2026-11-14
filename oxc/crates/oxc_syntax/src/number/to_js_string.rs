// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/// ### [6.1.6.1.20 Number::toString ( x, 10 )](https://tc39.es/ecma262/#sec-numeric-types-number-tostring)
pub(super) fn f64_to_js_string(x: f64) -> String {
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
