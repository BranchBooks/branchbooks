//! Exact decimal money. Every amount in the core is a `Decimal`; floats never touch money.

pub use rust_decimal::Decimal;
use rust_decimal::RoundingStrategy;

pub const ZERO: Decimal = Decimal::ZERO;

/// The most integer digits a written amount may have. Sums of up to 10^8 such amounts stay inside
/// `Decimal`'s 28 digits, so ledger arithmetic can never overflow (docs/DESIGN.md).
pub const MAX_INTEGER_DIGITS: usize = 20;

/// Number of digits after the decimal point as written (1.50 -> 2, 7 -> 0).
pub fn scale_of(text: &str) -> u32 {
    text.find('.')
        .map_or(0, |dot| (text.len() - dot - 1) as u32)
}

/// Parses a plain decimal as written: optional sign, digits, optional fraction (`-12.50`, `.5`,
/// `5.`). Keeps the written scale. `None` when malformed, or when it has more than
/// [`MAX_INTEGER_DIGITS`] integer digits or more than 28 significant or fractional digits.
pub fn parse_decimal(text: &str) -> Option<Decimal> {
    let (negative, body) = match text.as_bytes().first()? {
        b'-' => (true, &text[1..]),
        b'+' => (false, &text[1..]),
        _ => (false, text),
    };
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    if int.is_empty() && frac.is_empty() {
        return None;
    }
    if !int.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit()) {
        return None;
    }
    if int.trim_start_matches('0').len() > MAX_INTEGER_DIGITS || frac.len() > 28 {
        return None;
    }
    let mut mantissa: i128 = 0;
    for b in int.bytes().chain(frac.bytes()) {
        mantissa = mantissa
            .checked_mul(10)?
            .checked_add(i128::from(b - b'0'))?;
    }
    if negative {
        mantissa = -mantissa;
    }
    Decimal::try_from_i128_with_scale(mantissa, frac.len() as u32).ok()
}

/// Digits after the point once trailing zeros are dropped (1.50 -> 1, 7.00 -> 0).
pub fn normalized_scale(value: Decimal) -> u32 {
    value.normalize().scale()
}

/// Plain decimal text without trailing zeros and never in exponent form (1.50 -> 1.5, 0.00 -> 0).
pub fn plain(value: Decimal) -> String {
    value.normalize().to_string()
}

/// Text with exactly `places` decimals. Rounds half away from zero; zero never has a sign.
pub fn fixed(value: Decimal, places: u32) -> String {
    let mut rounded = value.round_dp_with_strategy(places, RoundingStrategy::MidpointAwayFromZero);
    rounded.rescale(places);
    if rounded.is_zero() {
        rounded = rounded.abs();
    }
    rounded.to_string()
}

/// Fixed-point display with thousands separators. Display only, never fed back into math.
pub fn fmt(value: Decimal, places: u32) -> String {
    let text = fixed(value, places);
    let (sign, body) = match text.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", text.as_str()),
    };
    let (int, frac) = match body.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (body, None),
    };
    let mut grouped = String::with_capacity(int.len() + int.len() / 3);
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    match frac {
        Some(f) => format!("{sign}{grouped}.{f}"),
        None => format!("{sign}{grouped}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Decimal {
        parse_decimal(s).unwrap()
    }

    #[test]
    fn parses_as_written() {
        assert_eq!(d("-1234.50").scale(), 2);
        assert_eq!(d(".5").to_string(), "0.5");
        assert_eq!(d("5.").to_string(), "5");
        assert_eq!(d("+5").to_string(), "5");
        assert!(parse_decimal("-").is_none());
        assert!(parse_decimal(".").is_none());
        assert!(parse_decimal("1e5").is_none());
        assert!(parse_decimal("123456789012345678901").is_none());
        assert!(parse_decimal("0000000000123456789012345678901").is_none());
        assert!(parse_decimal("00012345678901234567890.25").is_some());
    }

    #[test]
    fn plain_strings() {
        assert_eq!(plain(d("-1234.50")), "-1234.5");
        assert_eq!(plain(d("0.00")), "0");
        assert_eq!(plain(d("-0.00")), "0");
        assert_eq!(plain(d("0.000001")), "0.000001");
        assert_eq!(plain(d("0.0000001")), "0.0000001");
        assert_eq!(plain(d("-0.00000015")), "-0.00000015");
        assert_eq!(plain(d("10000000000000000000.00")), "10000000000000000000");
        assert_eq!(
            plain(Decimal::from_i128_with_scale(15 * 10i128.pow(20), 0)),
            "1500000000000000000000"
        );
    }

    #[test]
    fn fixed_and_formatted() {
        assert_eq!(fixed(d("1.005"), 2), "1.01");
        assert_eq!(fixed(d("-1.005"), 2), "-1.01");
        assert_eq!(fixed(d("-0.001"), 2), "0.00");
        assert_eq!(fixed(d("0"), 2), "0.00");
        assert_eq!(fixed(d("7"), 0), "7");
        assert_eq!(fmt(d("-1234567.5"), 2), "-1,234,567.50");
        assert_eq!(fmt(d("-0.001"), 2), "0.00");
        assert_eq!(fmt(d("999"), 2), "999.00");
    }
}
