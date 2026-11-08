//! Benchmark: synthetic ledgers of 5,000, 20,000 and 100,000 transactions in the planned storage
//! layout (main.beancount with options and opens, one include per year). Times loading, the full recompute and write operations.
//!   cargo bench -p branchbooks-core [-- sizes...]
//! The files are generated in memory, so "parse" has no disk reads.

use std::hint::black_box;
use std::time::Instant;

use branchbooks_core::book::Book;
use branchbooks_core::loader::{MemorySource, load};
use branchbooks_core::money::fixed;
use branchbooks_core::write::render::{AssignInput, EntryInput, PostingInput, TransactionInput};
use branchbooks_core::write::store::MemoryStore;
use branchbooks_core::write::workspace::{Operation, Workspace};

/// Beancount 3.2.3, from the brief.
const REFERENCE: [(usize, f64); 3] = [(5000, 0.3), (20000, 2.5), (100000, 15.4)];

const CATEGORIES: [&str; 8] = [
    "Expenses:Food:Groceries",
    "Expenses:Food:Dining",
    "Expenses:Housing:Rent",
    "Expenses:Transport:Fuel",
    "Expenses:Fun:Games",
    "Expenses:Health:Pharmacy",
    "Expenses:Home:Supplies",
    "Expenses:Gifts",
];

/// A 32-bit LCG, so every run benchmarks identical files. Floats only pick test data.
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

/// `new Date(Date.UTC(year, 0, 1 + day)).toISOString().slice(0, 10)`.
fn day_of_year(year: i64, day: i64) -> String {
    // Days from 1970-01-01 to `year`-01-01, then the civil date (Howard Hinnant's algorithm).
    let y = year - 1;
    let days = 365 * (y - 1969) + (y / 4 - 1969 / 4) - (y / 100 - 1969 / 100)
        + (y / 400 - 1969 / 400)
        + day;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(m <= 2);
    format!("{year:04}-{m:02}-{d:02}")
}

/// The ledger's files, root first.
fn generate(n: usize) -> Vec<(String, String)> {
    let mut r = Rng(n as u32);
    let per_year = 5000;
    let years = n.div_ceil(per_year);
    let start_year = 2026 - years as i64 + 1;
    let mut head =
        format!("option \"title\" \"Synthetic {n}\"\noption \"operating_currency\" \"USD\"\n\n");
    let mut open = |a: &str, flag: Option<&str>| {
        head += &format!("{start_year}-01-01 open {a} USD\n");
        if let Some(f) = flag {
            head += &format!("  budget: \"{f}\"\n");
        }
    };
    open("Assets:Checking", Some("on"));
    open("Liabilities:Visa", Some("on"));
    open("Assets:Brokerage", Some("off"));
    open("Income:Salary", None);
    open("Expenses:Tax", Some("excluded"));
    for c in CATEGORIES {
        open(c, None);
    }
    head += "\n";
    let mut files = Vec::new();
    let mut written = 0;
    for y in 0..years {
        let year = start_year + y as i64;
        let count = per_year.min(n - written);
        let mut lines = Vec::with_capacity(count + count / 50);
        for i in 0..count {
            let day = (i as f64 / count as f64 * 365.0) as i64;
            let d = day_of_year(year, day);
            let amt = format!("{}.{:02}", 1 + r.below(200), r.below(100));
            match i % 50 {
                0 => lines.push(format!(
                    "{d} * \"Employer\" \"Paycheck\"\n  Income:Salary  -5000.00 USD\n  Expenses:Tax  1000.00 USD\n  Assets:Checking  4000.00 USD\n"
                )),
                1 => lines.push(format!("{d} * \"Card payment\"\n  Liabilities:Visa  {amt} USD\n  Assets:Checking  -{amt} USD\n")),
                2 => lines.push(format!(
                    "{d} * \"Invest\"\n  category: \"Expenses:Gifts\"\n  Assets:Brokerage  {amt} USD\n  Assets:Checking  -{amt} USD\n"
                )),
                _ => {
                    let c = CATEGORIES[r.below(CATEGORIES.len())];
                    let from = if r.next() < 0.5 { "Assets:Checking" } else { "Liabilities:Visa" };
                    lines.push(format!("{d} * \"Shop {i}\" \"Purchase\"\n  {c}  {amt} USD\n  {from}  -{amt} USD\n"));
                }
            }
            if i % 50 == 25 {
                let c = CATEGORIES[r.below(CATEGORIES.len())];
                lines.push(format!("{d} custom \"envelope-assign\" {c} 500.00 USD\n"));
            }
        }
        files.push((format!("{year}.beancount"), lines.join("\n")));
        head += &format!("include \"{year}.beancount\"\n");
        written += count;
    }
    files.insert(0, ("main.beancount".to_owned(), head));
    files
}

struct Run {
    parse: f64,
    full: f64,
    unassigned: String,
    txns: usize,
}

fn time_load(source: &MemorySource) -> Run {
    let t0 = Instant::now();
    let loaded = load("main.beancount", source, None);
    let parse = t0.elapsed().as_secs_f64();
    let t1 = Instant::now();
    let mut book = Book::from_load(loaded);
    let errors = book.errors();
    assert!(
        errors.is_empty(),
        "synthetic ledger has errors: {:?}",
        &errors[..errors.len().min(3)]
    );
    let txns = book.transactions();
    let (count, last) = (
        txns.len(),
        txns.last()
            .expect("benchmark ledgers have transactions")
            .date
            .month(),
    );
    let view = book.budget(last, None);
    let full = parse + t1.elapsed().as_secs_f64();
    Run {
        parse,
        full,
        unassigned: fixed(view.unassigned, 2),
        txns: count,
    }
}

/// Write path: one operation = text edit + validation + commit to an in-memory store. Measures
/// `apply()` only, not the initial open. Back-dated entries make the budget re-check every later
/// month.
fn time_writes(files: &[(String, String)], ops: usize, backdated: bool) -> Vec<f64> {
    let store = MemoryStore::new(files.iter().map(|(a, b)| (a.as_str(), b.as_str())));
    let mut n = 0;
    let mut ws = Workspace::open(
        store,
        None,
        Box::new(move || {
            n += 1;
            format!("bench-{n}")
        }),
    )
    .unwrap();
    let date = {
        let txns = ws.book().transactions();
        let t = if backdated { txns.first() } else { txns.last() };
        t.expect("transactions").date.to_string()
    };
    (0..ops)
        .map(|i| {
            let category = CATEGORIES[i % CATEGORIES.len()].to_owned();
            let entry = if i % 2 == 0 {
                EntryInput::Assign(AssignInput {
                    date: date.clone(),
                    category,
                    amount: "10.00".into(),
                    currency: None,
                })
            } else {
                EntryInput::Txn(TransactionInput {
                    date: date.clone(),
                    narration: format!("Bench {i}"),
                    postings: vec![
                        PostingInput {
                            account: category,
                            amount: Some("1.00".into()),
                            ..Default::default()
                        },
                        PostingInput {
                            account: "Assets:Checking".into(),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                })
            };
            let t0 = Instant::now();
            black_box(
                ws.apply(&Operation::Add(entry), None, 0)
                    .expect("bench writes apply"),
            );
            t0.elapsed().as_secs_f64()
        })
        .collect()
}

fn median(xs: &[f64]) -> f64 {
    let mut s = xs.to_vec();
    s.sort_by(f64::total_cmp);
    s[s.len() / 2]
}

fn main() {
    // The calendar at its edges.
    assert_eq!(day_of_year(2024, 59), "2024-02-29");
    assert_eq!(day_of_year(2023, 365), "2024-01-01");
    // `cargo bench` passes --bench; any numbers are sizes.
    let sizes: Vec<usize> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    let sizes = if sizes.is_empty() {
        REFERENCE.map(|(n, _)| n).to_vec()
    } else {
        sizes
    };
    let (runs, ops) = (5, 6);
    println!("Rust core, {runs} runs each, median shown (first run in brackets)");
    println!(
        "Times in seconds. \"parse\" = parse incl. includes, from memory; \"full\" = parse + ledger + budget"
    );
    println!(
        "engine + one month view; \"write op\" = one Workspace::apply (render, validate the entry, budget"
    );
    println!(
        "check, commit) dated at the end of the ledger; \"back-dated\" = the same, dated in the first month."
    );
    println!("Median of {ops}. Unassigned is Unassigned in the last month.");
    println!();
    println!(
        "{:<8}{:>13}{:>13}{:>13}{:>13}{:>13}{:>18}{:>14}",
        "txns",
        "parse",
        "full",
        "[first full]",
        "write op",
        "back-dated",
        "beancount (brief)",
        "Unassigned"
    );
    for n in sizes {
        let files = generate(n);
        let source = MemorySource(files.iter().cloned().collect());
        let results: Vec<Run> = (0..runs).map(|_| time_load(&source)).collect();
        let writes = time_writes(&files, ops, false);
        let backdated = time_writes(&files, ops, true);
        let reference = REFERENCE
            .iter()
            .find(|(m, _)| *m == n)
            .map_or("-".to_owned(), |(_, s)| format!("{s:.1}"));
        println!(
            "{:<8}{:>13.3}{:>13.3}{:>13}{:>13.3}{:>13.3}{:>18}{:>14}",
            results[0].txns,
            median(&results.iter().map(|r| r.parse).collect::<Vec<_>>()),
            median(&results.iter().map(|r| r.full).collect::<Vec<_>>()),
            format!("[{:.3}]", results[0].full),
            median(&writes),
            median(&backdated),
            reference,
            results[0].unassigned,
        );
    }
}
