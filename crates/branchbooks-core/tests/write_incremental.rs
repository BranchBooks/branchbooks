//! The incremental write path must
//! leave the Book exactly as a fresh load of the committed files would: entries and their line
//! numbers, errors, balances, blocked entries and every month's budget.
//!
//! Operations come from proptest, so a failure shrinks to a short sequence. A fixed-seed run also
//! checks that both outcomes, applied and refused, are exercised.

mod common;

use branchbooks_core::book::Book;
use branchbooks_core::budget::MonthView;
use branchbooks_core::date::Month;
use branchbooks_core::loader::{MemorySource, load};
use branchbooks_core::money::plain;
use branchbooks_core::types::{Diagnostic, EntryType};
use branchbooks_core::write::render::{
    AssignInput, EntryInput, MoveInput, PostingInput, TargetInput, TransactionInput,
};
use branchbooks_core::write::store::{MemoryStore, Store};
use branchbooks_core::write::workspace::{ApplyError, Operation, Workspace};
use common::{BASE, fixtures};
use proptest::prelude::*;

#[derive(Debug, PartialEq)]
struct State {
    entries: Vec<String>,
    balances: Vec<String>,
    errors: Vec<Diagnostic>,
    budget: Vec<Diagnostic>,
    blocked: Vec<Diagnostic>,
    views: Vec<MonthView>,
}

fn state(book: &mut Book, months: &[Month]) -> State {
    let mut entries: Vec<String> = book
        .entries()
        .map(|(_, e)| {
            format!(
                "{}:{}-{} {} {} {}",
                e.file,
                e.line,
                e.end_line,
                e.seq,
                e.entry_type().name(),
                e.date
            )
        })
        .collect();
    entries.sort();
    let mut balances: Vec<String> = book
        .balances()
        .iter()
        .flat_map(|(a, m)| {
            m.iter()
                .filter(|(_, v)| !v.is_zero())
                .map(move |(c, v)| format!("{a} {c} {}", plain(*v)))
        })
        .collect();
    balances.sort();
    State {
        entries,
        balances,
        errors: book.errors(),
        budget: book.budget_diagnostics(),
        blocked: book.blocked(None),
        views: months.iter().map(|&m| book.budget(m, None)).collect(),
    }
}

fn fresh(store: &MemoryStore) -> Book {
    let source = MemorySource(store.snapshot().unwrap().files.into_iter().collect());
    Book::from_load(load("main.beancount", &source, None))
}

struct Setup {
    files: Vec<(String, String)>,
    categories: &'static [&'static str],
    funding: &'static [&'static str],
    years: &'static [u32],
    new_year: u32,
}

fn single_file() -> Setup {
    Setup {
        files: vec![("main.beancount".into(), BASE.clone())],
        categories: &[
            "Expenses:Food:Groceries",
            "Expenses:Food:Dining",
            "Expenses:Housing:Rent",
            "Expenses:Savings:Investing",
        ],
        funding: &["Assets:Checking", "Liabilities:CreditCard"],
        years: &[2026],
        new_year: 2026,
    }
}

fn year_files() -> Setup {
    let files = ["main.beancount", "2025.beancount", "2026.beancount"]
        .map(|f| {
            (
                f.to_owned(),
                std::fs::read_to_string(fixtures().join("multifile").join(f)).unwrap(),
            )
        })
        .into();
    Setup {
        files,
        categories: &[
            "Expenses:Food:Groceries",
            "Expenses:Housing:Rent",
            "Expenses:Old:Category",
        ],
        funding: &["Assets:Checking", "Liabilities:Visa"],
        years: &[2025, 2026],
        new_year: 2027,
    }
}

/// One random entry, as raw choices: resolved against a setup by `entry`.
#[derive(Clone, Debug)]
struct EntrySpec {
    kind: u8,
    date: DateSpec,
    category: usize,
    other: usize,
    funding: usize,
    cents: u32,
    negative: bool,
    narration: u32,
}

#[derive(Clone, Debug)]
struct DateSpec {
    new_year: bool,
    year: usize,
    month: u32,
    day: u32,
}

#[derive(Clone, Debug)]
enum Step {
    Add(EntrySpec),
    /// Edit the nth editable entry, keeping its date or using a new one.
    Edit(usize, bool, EntrySpec),
    Delete(usize),
}

fn date_of(s: &Setup, d: &DateSpec) -> String {
    let y = if d.new_year {
        s.new_year
    } else {
        s.years[d.year % s.years.len()]
    };
    let m = if y == 2025 { 12 } else { 1 + d.month % 12 };
    format!("{y}-{m:02}-{:02}", 1 + d.day % 28)
}

fn entry(s: &Setup, e: &EntrySpec, date: String) -> EntryInput {
    let amount = format!("{}.{:02}", e.cents / 100 % 400, e.cents % 100);
    let cat = |i: usize| s.categories[i % s.categories.len()].to_owned();
    match e.kind % 20 {
        0..5 => EntryInput::Assign(AssignInput {
            date,
            category: cat(e.category),
            amount,
            currency: None,
        }),
        5..7 => EntryInput::Move(MoveInput {
            date,
            from: cat(e.category),
            to: s.categories[s.categories.len() - 1 - e.other % s.categories.len()].to_owned(),
            amount,
            currency: None,
        }),
        // Targets: one-time with a due date in the entry's year, or monthly.
        7..9 => EntryInput::Target(TargetInput {
            due: e
                .other
                .is_multiple_of(2)
                .then(|| format!("{}-{:02}-15", &date[..4], 1 + e.other % 12)),
            date,
            category: cat(e.category),
            amount,
            ..Default::default()
        }),
        _ => EntryInput::Txn(TransactionInput {
            date,
            narration: format!("Step {}", e.narration),
            postings: vec![
                PostingInput {
                    account: cat(e.category),
                    amount: Some(format!("{}{amount}", if e.negative { "-" } else { "" })),
                    ..Default::default()
                },
                PostingInput {
                    account: s.funding[e.funding % s.funding.len()].to_owned(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }),
    }
}

fn date_spec() -> impl Strategy<Value = DateSpec> {
    (
        prop::bool::weighted(0.04),
        any::<usize>(),
        any::<u32>(),
        any::<u32>(),
    )
        .prop_map(|(new_year, year, month, day)| DateSpec {
            new_year,
            year,
            month,
            day,
        })
}

fn entry_spec() -> impl Strategy<Value = EntrySpec> {
    (
        any::<u8>(),
        date_spec(),
        any::<usize>(),
        any::<usize>(),
        any::<usize>(),
        0u32..40_000,
        prop::bool::weighted(0.15),
        any::<u32>(),
    )
        .prop_map(
            |(kind, date, category, other, funding, cents, negative, narration)| EntrySpec {
                kind,
                date,
                category,
                other,
                funding,
                cents,
                negative,
                narration,
            },
        )
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        11 => entry_spec().prop_map(Step::Add),
        5 => (any::<usize>(), any::<bool>(), entry_spec()).prop_map(|(i, keep, e)| Step::Edit(i, keep, e)),
        4 => any::<usize>().prop_map(Step::Delete),
    ]
}

/// Applies the steps, comparing the workspace's Book with a fresh load after each one. Returns
/// how many were applied and how many refused.
fn run(setup: &Setup, steps: &[Step]) -> (usize, usize) {
    let store = MemoryStore::new(setup.files.iter().map(|(a, b)| (a.as_str(), b.as_str())));
    let mut n = 0;
    let ids = Box::new(move || {
        n += 1;
        format!("op-{n}")
    });
    let mut ws = Workspace::open(store, None, ids).unwrap();
    let mut months = std::collections::BTreeSet::new();
    let (mut applied, mut refused) = (0, 0);
    for (i, s) in steps.iter().enumerate() {
        let editable: Vec<_> = ws
            .core
            .entries()
            .filter(|(_, e)| matches!(e.entry_type(), EntryType::Txn | EntryType::Custom))
            .map(|(_, e)| e.clone())
            .collect();
        let op = match s {
            Step::Edit(k, keep, e) if !editable.is_empty() => {
                let target = &editable[k % editable.len()];
                let date = if *keep {
                    target.date.to_string()
                } else {
                    date_of(setup, &e.date)
                };
                Operation::Edit(ws.core.ref_of(target), entry(setup, e, date))
            }
            Step::Delete(k) if !editable.is_empty() => {
                Operation::Delete(ws.core.ref_of(&editable[k % editable.len()]))
            }
            Step::Add(e) | Step::Edit(_, _, e) => {
                Operation::Add(entry(setup, e, date_of(setup, &e.date)))
            }
            Step::Delete(_) => continue,
        };
        match ws.apply(&op, None, 0) {
            Ok(_) => applied += 1,
            Err(ApplyError::Operation(_)) => refused += 1,
            Err(e) => panic!("store error: {e:?}"),
        }
        months.extend(ws.core.entries().map(|(_, e)| e.date.month()));
        let (first, last) = (*months.first().unwrap(), months.last().unwrap().next());
        let mut range = Vec::new();
        let mut m = first;
        while m <= last {
            range.push(m);
            m = m.next();
        }
        let want = state(&mut fresh(&ws.store), &range);
        assert_eq!(state(ws.book(), &range), want, "step {i} ({op:?})");
    }
    (applied, refused)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

    #[test]
    fn single_file_random_adds_edits_and_deletes(steps in prop::collection::vec(step(), 1..40)) {
        run(&single_file(), &steps);
    }

    #[test]
    fn year_files_random_adds_edits_and_deletes(steps in prop::collection::vec(step(), 1..40)) {
        run(&year_files(), &steps);
    }
}

/// Fixed seeds, so the run is the same every time: it must exercise both outcomes.
#[test]
fn both_outcomes_are_exercised() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
    for setup in [single_file(), year_files()] {
        let (mut applied, mut refused) = (0, 0);
        for seed in 1..=6u8 {
            let rng = TestRng::from_seed(RngAlgorithm::ChaCha, &[seed; 32]);
            let mut runner = TestRunner::new_with_rng(Config::default(), rng);
            let steps = prop::collection::vec(step(), 40)
                .new_tree(&mut runner)
                .unwrap()
                .current();
            let (a, r) = run(&setup, &steps);
            applied += a;
            refused += r;
        }
        assert!(applied > 120, "applied {applied}");
        assert!(refused > 3, "refused {refused}");
    }
}
