//! Acceptance 3: a full recompute always
//! equals the incremental result.

mod common;

use branchbooks_core::book::Book;
use branchbooks_core::budget::MonthView;
use branchbooks_core::date::{Date, Month};
use branchbooks_core::ledger::sort_diagnostics;
use branchbooks_core::loader::{FileSource, LoadResult, MemorySource, load};
use branchbooks_core::money::plain;
use branchbooks_core::types::{Diagnostic, Entry, EntryType};
use common::fixtures;

/// A 32-bit LCG, so runs repeat. Floats only pick test cases, never touch money.
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        f64::from(self.0) / 4_294_967_296.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() * n as f64) as usize
    }
}

fn shuffle<T: Clone>(xs: &[T], r: &mut Rng) -> Vec<T> {
    let mut a = xs.to_vec();
    for i in (1..a.len()).rev() {
        let j = r.below(i + 1);
        a.swap(i, j);
    }
    a
}

fn months(entries: &[Entry]) -> Vec<Month> {
    let mut dates: Vec<Date> = entries.iter().map(|e| e.date).collect();
    dates.sort();
    let (Some(first), Some(last)) = (dates.first(), dates.last()) else {
        return Vec::new();
    };
    let last = last.month().next();
    let mut out = Vec::new();
    let mut m = first.month();
    while m <= last {
        out.push(m);
        m = m.next();
    }
    out
}

#[derive(Debug, PartialEq)]
struct Snapshot {
    balances: Vec<String>,
    errors: Vec<Diagnostic>,
    budget: Vec<Diagnostic>,
    views: Vec<MonthView>,
    /// Unassigned as of the last month, from every month view.
    as_of_last: Vec<MonthView>,
}

fn snapshot(book: &mut Book, ms: &[Month]) -> Snapshot {
    let mut balances: Vec<String> = book
        .balances()
        .iter()
        .flat_map(|(a, by_cur)| {
            by_cur
                .iter()
                .filter(|(_, v)| !v.is_zero())
                .map(move |(c, v)| format!("{a} {c} {}", plain(*v)))
        })
        .collect();
    balances.sort();
    let today = ms.last().map(|m| m.first_day());
    Snapshot {
        balances,
        errors: sort_diagnostics(book.errors()),
        budget: book.budget_diagnostics(),
        views: ms.iter().map(|&m| book.budget(m, None)).collect(),
        as_of_last: ms.iter().map(|&m| book.budget(m, today)).collect(),
    }
}

fn full(entries: &[Entry], loaded: &LoadResult) -> Book {
    let mut entries = entries.to_vec();
    entries.sort_by_key(|e| e.seq);
    Book::from_load(LoadResult {
        entries,
        errors: Vec::new(),
        ..loaded.clone()
    })
}

/// Adds opens first, then everything else in a shuffled order, checking against a full recompute
/// as it goes.
fn check(loaded: &LoadResult, seed: u32, check_every: usize) {
    let mut r = Rng(seed);
    let is_open = |e: &&Entry| matches!(e.entry_type(), EntryType::Open | EntryType::Close);
    let opens: Vec<Entry> = loaded.entries.iter().filter(is_open).cloned().collect();
    let rest: Vec<Entry> = loaded
        .entries
        .iter()
        .filter(|e| !is_open(e))
        .cloned()
        .collect();
    let rest = shuffle(&rest, &mut r);
    let ms = months(&loaded.entries);
    let mut inc = Book::empty(loaded.options.clone());
    let mut added = Vec::new();
    for e in opens {
        inc.add_entry(e.clone());
        added.push(e);
    }
    // Force the rebuild after opens, so the rest is truly incremental.
    inc.budget(ms.first().copied().unwrap_or(common::m("2026-01")), None);
    let n = rest.len();
    for (i, e) in rest.into_iter().enumerate() {
        inc.add_entry(e.clone());
        added.push(e);
        // Query one month so snapshots exist and later inserts must invalidate them correctly.
        let m = ms[r.below(ms.len())];
        inc.budget(m, None);
        if (i + 1) % check_every == 0 || i == n - 1 {
            assert_eq!(
                snapshot(&mut inc, &ms),
                snapshot(&mut full(&added, loaded), &ms),
                "seed {seed}, step {i}"
            );
        }
    }
}

struct DiskSource;

impl FileSource for DiskSource {
    fn read(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }
}

#[test]
fn incremental_equals_full_recompute_on_every_fixture() {
    let mut files = vec![
        "examples-1-5.bean".to_owned(),
        "multifile/main.beancount".to_owned(),
    ];
    let mut scenarios: Vec<String> = std::fs::read_dir(fixtures().join("scenarios"))
        .unwrap()
        .map(|e| format!("scenarios/{}", e.unwrap().file_name().to_string_lossy()))
        .collect();
    scenarios.sort();
    files.extend(scenarios);
    for f in &files {
        let loaded = load(fixtures().join(f).to_str().unwrap(), &DiskSource, None);
        for seed in 1..=5 {
            check(&loaded, seed, 1);
        }
    }
}

#[test]
fn random_synthetic_ledger_entries_in_random_order() {
    let mut r = Rng(42);
    let cats = ["Expenses:A", "Expenses:B", "Expenses:C", "Expenses:D"];
    let funding = ["Assets:Checking", "Liabilities:Visa", "Liabilities:Amex"];
    let mut text = String::from("option \"operating_currency\" \"USD\"\n");
    text += "2026-01-01 open Assets:Checking USD\n  budget: \"on\"\n2026-01-01 open Liabilities:Visa USD\n  budget: \"on\"\n";
    text += "2026-01-01 open Liabilities:Amex USD\n  budget: \"on\"\n2026-01-01 open Assets:Savings USD\n  budget: \"off\"\n";
    text += "2026-01-01 open Income:Job USD\n";
    for c in cats {
        text += &format!("2026-01-01 open {c} USD\n");
    }
    for _ in 0..160 {
        let date = format!("2026-0{}-{:02}", 1 + r.below(5), 1 + r.below(28));
        let k = r.next();
        let mut amt = || format!("{}.{:02}", r.below(300), r.below(100));
        let a = amt();
        let mut pick = |xs: &[&'static str]| xs[r.below(xs.len())];
        text += &if k < 0.08 {
            format!("{date} * \"pay\"\n  Income:Job  -{a} USD\n  Assets:Checking\n")
        } else if k < 0.3 {
            format!(
                "{date} custom \"envelope-assign\" {} {a} USD\n",
                pick(&cats)
            )
        } else if k < 0.38 {
            format!(
                "{date} custom \"envelope-move\" {} {} {a} USD\n",
                pick(&cats),
                pick(&cats)
            )
        } else if k < 0.48 {
            format!(
                "{date} * \"card payment\"\n  {}  {a} USD\n  Assets:Checking\n",
                pick(&funding[1..])
            )
        } else if k < 0.53 {
            format!(
                "{date} * \"save\"\n  category: \"{}\"\n  Assets:Savings  {a} USD\n  Assets:Checking\n",
                pick(&cats)
            )
        } else if k < 0.6 {
            format!(
                "{date} * \"refund\"\n  {}  -{a} USD\n  {}\n",
                pick(&cats),
                pick(&funding)
            )
        } else if k < 0.66 {
            // Targets: monthly, or one-time due within the year (T1 to T5).
            let months = [
                "01", "02", "03", "04", "05", "06", "07", "08", "09", "10", "11", "12",
            ];
            let due = if k < 0.63 {
                format!("  due: 2026-{}-15\n", pick(&months))
            } else {
                String::new()
            };
            format!("{date} custom \"target\" {} {a} USD\n{due}", pick(&cats))
        } else {
            format!(
                "{date} * \"spend\"\n  {}  {a} USD\n  {}\n",
                pick(&cats),
                pick(&funding)
            )
        };
    }
    let loaded = load(
        "/s.bean",
        &MemorySource::new([("/s.bean", text.as_str())]),
        None,
    );
    assert_eq!(loaded.errors, vec![]);
    for seed in 1..=3 {
        check(&loaded, seed, 10);
    }
}
