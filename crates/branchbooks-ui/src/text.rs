//! Words and numbers for people.

use branchbooks_core::date::{Date, Month};
use rust_decimal::Decimal;
use std::str::FromStr;

/// Two decimals with thousands separators: "1,050.00". Never "-0.00".
pub fn fmt(value: Decimal) -> String {
    branchbooks_core::money::fmt(value, 2)
}

/// "Expenses:Food:Dining" -> "Food:Dining".
pub fn short(account: &str) -> &str {
    ["Expenses:", "Liabilities:", "Assets:", "Income:", "Equity:"]
        .iter()
        .find_map(|p| account.strip_prefix(p))
        .unwrap_or(account)
}

/// A name part for people: "Repair-And-Maintenance" -> "Repair And Maintenance".
pub fn words(part: &str) -> String {
    part.replace('-', " ")
}

/// The reverse of `words`, for a name part typed by a person: "chase  visa" -> "Chase-Visa". What it
/// can't fix, such as "&", is left for the account name check to refuse.
pub fn ledger_part(typed: &str) -> String {
    typed
        .split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join("-")
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// "2026-01" -> "January 2026".
pub fn month_label(month: Month) -> String {
    let name = MONTHS[usize::from(month.number()) - 1];
    format!("{name} {:04}", month.year())
}

/// Envelope entries for a month are dated today in the current month, else at its edge.
pub fn envelope_date(month: Month, today: Date) -> Date {
    match month.cmp(&today.month()) {
        std::cmp::Ordering::Equal => today,
        std::cmp::Ordering::Greater => month.first_day(),
        std::cmp::Ordering::Less => month.last_day(),
    }
}

/// More than 50 years from `today`: a mistyped year, not a date anyone means.
pub fn is_far_date(date: Date, today: Date) -> bool {
    (date.year() - today.year()).abs() > 50
}

/// What someone typed as an amount, commas allowed, or what to tell them.
pub fn parse_amount(text: &str) -> Result<Decimal, String> {
    Decimal::from_str(&norm_amount(text))
        .map_err(|_| format!("{:?} is not an amount, such as 1,500.00.", text.trim()))
}

/// "20" -> "20.00", "1,050.5" -> "1050.50"; anything with more decimals, or not a plain number,
/// is left for the write path to judge.
pub fn norm_amount(text: &str) -> String {
    let t = text.trim().replace(',', "");
    let digits = t.strip_prefix('-').unwrap_or(&t);
    let (int, frac) = digits.split_once('.').unwrap_or((digits, ""));
    let plain = !int.is_empty()
        && int.bytes().all(|b| b.is_ascii_digit())
        && frac.len() <= 2
        && frac.bytes().all(|b| b.is_ascii_digit());
    match Decimal::from_str_exact(&t) {
        Ok(d) if plain => format!("{d:.2}"),
        _ => t,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_words_become_a_name_part_and_back() {
        assert_eq!(ledger_part("chase  visa "), "Chase-Visa");
        assert_eq!(ledger_part("401k"), "401k");
        assert_eq!(ledger_part("  "), "");
        assert_eq!(
            words(&ledger_part("Repair And Maintenance")),
            "Repair And Maintenance"
        );
    }

    #[test]
    fn formats_money_and_months() {
        let d = |s| Decimal::from_str(s).unwrap();
        assert_eq!(fmt(d("1050")), "1,050.00");
        assert_eq!(fmt(d("-1234567.5")), "-1,234,567.50");
        assert_eq!(fmt(d("-0.001")), "0.00");
        assert_eq!(fmt(d("999.995")), "1,000.00");
        let (m, day) = (
            |s: &str| s.parse::<Month>().unwrap(),
            |s: &str| s.parse::<Date>().unwrap(),
        );
        assert_eq!(month_label(m("2026-01")), "January 2026");
        let today = day("2026-10-27");
        assert_eq!(envelope_date(m("2026-10"), today), today);
        assert_eq!(envelope_date(m("2026-11"), today), day("2026-11-01"));
        assert_eq!(envelope_date(m("2026-02"), today), day("2026-02-28"));
        assert!(!is_far_date(day("2076-12-31"), today) && !is_far_date(day("1976-01-01"), today));
        assert!(is_far_date(day("2077-01-01"), today) && is_far_date(day("0026-10-27"), today));
        assert_eq!(short("Expenses:Food:Dining"), "Food:Dining");
        assert_eq!(norm_amount(" 20 "), "20.00");
        assert_eq!(norm_amount("1,050.5"), "1050.50");
        assert_eq!(norm_amount("-3.25"), "-3.25");
        assert_eq!(norm_amount("1.005"), "1.005");
        assert_eq!(norm_amount("abc"), "abc");
        assert_eq!(parse_amount(" 1,500 "), Ok(d("1500.00")));
        assert_eq!(parse_amount("-1,050.5"), Ok(d("-1050.50")));
        for bad in ["", "$20", "abc"] {
            let e = parse_amount(bad).unwrap_err();
            assert!(e.contains("not an amount"), "{bad}: {e}");
        }
    }
}
