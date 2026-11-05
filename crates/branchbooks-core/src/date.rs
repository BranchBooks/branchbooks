//! Calendar dates and months: valid by construction and cheap to copy. Text becomes a [`Date`] or a
//! [`Month`] where it enters, by `parse`; past that, nothing can hold an impossible date. Both sort
//! in calendar order and print as Beancount writes them (`2026-10-31`, `2026-10`).

use std::fmt;
use std::str::FromStr;

/// Why text is not a date or a month.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateError {
    /// Not written as `YYYY-MM-DD` or `YYYY-MM`.
    Shape,
    /// Written that way, but no such month or day.
    Range,
}

impl fmt::Display for DateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DateError::Shape => "not written as a date",
            DateError::Range => "no such date",
        })
    }
}

impl std::error::Error for DateError {}

/// A calendar month. Field order gives calendar order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Month {
    year: i32,
    month: u8,
}

/// A calendar day. Field order gives calendar order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    year: i32,
    month: u8,
    day: u8,
}

fn days_in(year: i32, month: u8) -> u8 {
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        _ => 28,
    }
}

/// The digits of a field of exactly `width` ASCII digits.
fn field(text: &str, width: usize) -> Result<u32, DateError> {
    if text.len() == width && text.bytes().all(|b| b.is_ascii_digit()) {
        text.parse().map_err(|_| DateError::Shape)
    } else {
        Err(DateError::Shape)
    }
}

impl Month {
    /// `None` unless `month` is 1 to 12.
    pub fn new(year: i32, month: u8) -> Option<Self> {
        (1..=12).contains(&month).then_some(Month { year, month })
    }

    pub fn year(self) -> i32 {
        self.year
    }

    /// 1 to 12.
    pub fn number(self) -> u8 {
        self.month
    }

    fn index(self) -> i64 {
        i64::from(self.year) * 12 + i64::from(self.month) - 1
    }

    fn from_index(index: i64) -> Self {
        Month {
            // Only one month from a valid year, which is far inside i32.
            year: i32::try_from(index.div_euclid(12)).unwrap_or(i32::MAX),
            month: u8::try_from(index.rem_euclid(12) + 1).unwrap_or(12),
        }
    }

    pub fn next(self) -> Self {
        Self::from_index(self.index() + 1)
    }

    pub fn prev(self) -> Self {
        Self::from_index(self.index() - 1)
    }

    /// Months from `self` to `to`, counting both (2026-01 to 2026-06 is 6); zero or less when `to`
    /// is earlier.
    pub fn months_until(self, to: Month) -> i64 {
        to.index() - self.index() + 1
    }

    pub fn first_of_year(self) -> Date {
        Date {
            year: self.year,
            month: 1,
            day: 1,
        }
    }

    pub fn first_day(self) -> Date {
        Date {
            year: self.year,
            month: self.month,
            day: 1,
        }
    }

    pub fn last_day(self) -> Date {
        Date {
            year: self.year,
            month: self.month,
            day: days_in(self.year, self.month),
        }
    }
}

impl Date {
    /// `None` when there is no such day.
    pub fn new(year: i32, month: u8, day: u8) -> Option<Self> {
        let in_month = (1..=12).contains(&month) && (1..=days_in(year, month)).contains(&day);
        in_month.then_some(Date { year, month, day })
    }

    pub fn year(self) -> i32 {
        self.year
    }

    pub fn day(self) -> u8 {
        self.day
    }

    /// The calendar month this day is in.
    pub fn month(self) -> Month {
        Month {
            year: self.year,
            month: self.month,
        }
    }

    pub fn next_day(self) -> Self {
        if self.day < days_in(self.year, self.month) {
            Date {
                day: self.day + 1,
                ..self
            }
        } else {
            self.month().next().first_day()
        }
    }
}

impl FromStr for Month {
    type Err = DateError;

    /// Exactly `YYYY-MM`.
    fn from_str(s: &str) -> Result<Self, DateError> {
        let (year, month) = s.split_once('-').ok_or(DateError::Shape)?;
        let year = i32::try_from(field(year, 4)?).map_err(|_| DateError::Shape)?;
        let month = u8::try_from(field(month, 2)?).map_err(|_| DateError::Shape)?;
        Month::new(year, month).ok_or(DateError::Range)
    }
}

impl FromStr for Date {
    type Err = DateError;

    /// Exactly `YYYY-MM-DD`, and a real day. The year can be any four digits.
    fn from_str(s: &str) -> Result<Self, DateError> {
        let (month, day) = s.rsplit_once('-').ok_or(DateError::Shape)?;
        let day = u8::try_from(field(day, 2)?).map_err(|_| DateError::Shape)?;
        let month: Month = month.parse()?;
        Date::new(month.year, month.month, day).ok_or(DateError::Range)
    }
}

impl fmt::Display for Month {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}", self.year, self.month)
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Date {
        s.parse().unwrap()
    }

    fn m(s: &str) -> Month {
        s.parse().unwrap()
    }

    #[test]
    fn parses_only_real_dates() {
        assert_eq!(d("2024-02-29").to_string(), "2024-02-29");
        assert_eq!(d("0000-01-01").to_string(), "0000-01-01");
        assert_eq!("2026-02-29".parse::<Date>(), Err(DateError::Range));
        assert_eq!("2026-13-01".parse::<Date>(), Err(DateError::Range));
        assert_eq!("2026-04-31".parse::<Date>(), Err(DateError::Range));
        assert_eq!("2026-01-00".parse::<Date>(), Err(DateError::Range));
        for bad in [
            "",
            "2026",
            "2026-1-1",
            "2026-01-1",
            "26-01-01",
            "2026/01/01",
            "2026-01-01 ",
            "+026-01-01",
            "२०२६-01-01",
            "2026-01-015",
        ] {
            assert_eq!(bad.parse::<Date>(), Err(DateError::Shape), "{bad}");
        }
    }

    #[test]
    fn parses_only_real_months() {
        assert_eq!(m("2026-10").to_string(), "2026-10");
        assert_eq!("2026-13".parse::<Month>(), Err(DateError::Range));
        assert_eq!("2026-00".parse::<Month>(), Err(DateError::Range));
        for bad in ["", "2026", "2026-1", "2026-100", "2026-10-01", "é026-10"] {
            assert_eq!(bad.parse::<Month>(), Err(DateError::Shape), "{bad}");
        }
    }

    #[test]
    fn sorts_in_calendar_order() {
        assert!(d("2026-09-30") < d("2026-10-01"));
        assert!(d("2025-12-31") < d("2026-01-01"));
        assert!(m("2025-12") < m("2026-01"));
    }

    #[test]
    fn months_step_across_years() {
        assert_eq!(m("2026-12").next(), m("2027-01"));
        assert_eq!(m("2026-01").prev(), m("2025-12"));
        assert_eq!(m("2026-01").months_until(m("2026-06")), 6);
        assert_eq!(m("2026-06").months_until(m("2026-06")), 1);
        assert_eq!(m("2026-06").months_until(m("2026-05")), 0);
        assert_eq!(m("2025-12").months_until(m("2026-01")), 2);
    }

    #[test]
    fn days_step_across_months_and_leap_years() {
        assert_eq!(d("2026-10-30").next_day(), d("2026-10-31"));
        assert_eq!(d("2026-10-31").next_day(), d("2026-11-01"));
        assert_eq!(d("2026-12-31").next_day(), d("2027-01-01"));
        assert_eq!(d("2024-02-28").next_day(), d("2024-02-29"));
        assert_eq!(d("2026-02-28").next_day(), d("2026-03-01"));
        assert_eq!(m("2024-02").last_day(), d("2024-02-29"));
        assert_eq!(m("1900-02").last_day(), d("1900-02-28"));
        assert_eq!(m("2000-02").last_day(), d("2000-02-29"));
        assert_eq!(d("2026-10-15").month(), m("2026-10"));
    }
}
