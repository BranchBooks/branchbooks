//! Shared test helpers.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use branchbooks_core::book::Book;
use branchbooks_core::budget::{CategoryRow, LiabilityRow, MonthView};
use branchbooks_core::date::{Date, Month};
use branchbooks_core::ledger::balance_of;
use branchbooks_core::loader::{FileSource, MemorySource};
use branchbooks_core::money::{Decimal, parse_decimal, plain};

/// A month, written as in a ledger.
pub fn m(s: &str) -> Month {
    s.parse().unwrap()
}

/// A date, written as in a ledger.
pub fn d(s: &str) -> Date {
    s.parse().unwrap()
}

pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

pub static BASE: LazyLock<String> =
    LazyLock::new(|| fs::read_to_string(fixtures().join("examples-1-5.bean")).unwrap());

/// The examples fixture up to and including Example n.
pub fn up_to(n: u32) -> String {
    let marker = format!("; Example {}\n", n + 1);
    match BASE.find(&marker) {
        Some(i) => BASE[..i].to_owned(),
        None => BASE.clone(),
    }
}

/// Loads `base` plus extra ledger text through the real loader (the extra text includes the
/// base).
pub fn book_of_with(extra: &str, base: &str) -> Book {
    let main = format!("include \"base.bean\"\n{extra}");
    Book::load(
        "/main.bean",
        &MemorySource::new([("/base.bean", base), ("/main.bean", main.as_str())]),
    )
}

pub fn book_of(extra: &str) -> Book {
    book_of_with(extra, &BASE)
}

struct DiskSource;

impl FileSource for DiskSource {
    fn read(&self, path: &str) -> Option<String> {
        fs::read_to_string(path).ok()
    }
}

pub fn fixture(path: &str) -> Book {
    Book::load(fixtures().join(path).to_str().unwrap(), &DiskSource)
}

#[track_caller]
pub fn cat<'a>(v: &'a MonthView, suffix: &str) -> &'a CategoryRow {
    let tail = format!(":{suffix}");
    v.categories
        .iter()
        .find(|c| c.category == suffix || c.category.ends_with(&tail))
        .unwrap_or_else(|| {
            panic!(
                "no category {suffix} in {:?}",
                v.categories.iter().map(|c| &c.category).collect::<Vec<_>>()
            )
        })
}

#[track_caller]
pub fn liability(v: &MonthView) -> &LiabilityRow {
    liability_named(v, "Liabilities:CreditCard")
}

#[track_caller]
pub fn liability_named<'a>(v: &'a MonthView, name: &str) -> &'a LiabilityRow {
    v.liability(name)
        .unwrap_or_else(|| panic!("no liability {name}"))
}

pub fn dec(s: &str) -> Decimal {
    parse_decimal(s).unwrap_or_else(|| panic!("bad decimal {s}"))
}

/// Exact decimal equality, with readable failures.
#[track_caller]
pub fn eq(actual: Decimal, expected: impl ToString) {
    let expected = expected.to_string();
    assert_eq!(plain(actual), plain(dec(&expected)), "expected {expected}");
}

pub fn balance(book: &mut Book, account: &str) -> Decimal {
    balance_of(book.balances(), account, "USD")
}

pub fn groceries(date: &str, amount: &str) -> String {
    format!("{date} * \"Groceries\"\n  Expenses:Food:Groceries  {amount} USD\n  Assets:Checking\n")
}

pub fn dining(date: &str, amount: &str, from: &str) -> String {
    format!("{date} * \"Dining\"\n  Expenses:Food:Dining  {amount} USD\n  {from}\n")
}

pub fn assign(date: &str, category: &str, amount: &str) -> String {
    format!("{date} custom \"envelope-assign\" {category} {amount} USD\n")
}
