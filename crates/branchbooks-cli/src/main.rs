//! branchbooks CLI.
//!
//! ```text
//!   branchbooks budget <file> --month YYYY-MM [--today YYYY-MM-DD]
//!                                               Available per envelope; Unassigned as of today
//!   branchbooks check <file> [--json]           ledger errors and account balances
//!   branchbooks assign|move|txn|open|close|delete|log <repo> ...   one operation, one git commit (see write.rs)
//! ```
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

mod write;

use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use branchbooks_core::book::Book;
use branchbooks_core::budget::MonthView;
use branchbooks_core::date::{Date, Month};
use branchbooks_core::loader::FileSource;
use branchbooks_core::money::{Decimal, fmt, plain};
use indexmap::IndexMap;
use serde_json::{Value, json};

/// Options by key, each with every value given.
pub type Opts = IndexMap<String, Vec<String>>;

struct DiskSource;

impl FileSource for DiskSource {
    fn read(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }
}

fn usage() -> ExitCode {
    eprintln!(
        "usage:\n  branchbooks budget <file> --month YYYY-MM [--today YYYY-MM-DD]\n  branchbooks check <file> [--json]\n{}",
        write::WRITE_USAGE
    );
    ExitCode::from(2)
}

/// The path made absolute against the working directory, without following symlinks, so the
/// file names in diagnostics are the ones the user typed.
fn absolute(p: &str) -> PathBuf {
    let joined = std::env::current_dir().unwrap_or_default().join(p);
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

fn to_posix(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// `s` padded to `n` characters.
fn pad(s: &str, n: usize, right: bool) -> String {
    if right {
        format!("{s:>n$}")
    } else {
        format!("{s:<n$}")
    }
}

pub fn render_budget(v: &MonthView) -> String {
    let mut out: Vec<String> = Vec::new();
    let name_w = v
        .categories
        .iter()
        .map(|c| c.category.chars().count())
        .chain(v.cards.iter().map(|c| c.card.chars().count()))
        .fold(8, usize::max);
    let num = |d: Decimal| pad(&fmt(d, 2), 12, true);
    // The Target column appears only for ledgers with target budgeting.
    let targets = v
        .categories
        .iter()
        .any(|c| c.target.is_some() || !c.target_allocated.is_zero());
    out.push(format!(
        "Budget for {} ({}); Unassigned as of {}",
        v.month, v.currency, v.today
    ));
    out.push(String::new());
    let target_head = if targets {
        format!(" {}", pad("Target", 12, true))
    } else {
        String::new()
    };
    out.push(format!(
        "{} {} {}{} {} {} {}  Overspent",
        pad("Envelope", name_w, false),
        pad("Carried", 12, true),
        pad("Assigned", 12, true),
        target_head,
        pad("Moved", 12, true),
        pad("Activity", 12, true),
        pad("Available", 12, true),
    ));
    for c in &v.categories {
        let over = if c.cash_overspent > Decimal::ZERO || c.credit_overspent > Decimal::ZERO {
            format!(
                "  cash {} / credit {}",
                fmt(c.cash_overspent, 2),
                fmt(c.credit_overspent, 2)
            )
        } else {
            String::new()
        };
        let target = if targets {
            format!(" {}", num(c.target_allocated))
        } else {
            String::new()
        };
        out.push(format!(
            "{} {} {}{} {} {} {}{}",
            pad(&c.category, name_w, false),
            num(c.carried_in),
            num(c.assigned),
            target,
            num(c.moved),
            num(c.activity),
            num(c.available),
            over
        ));
    }
    if !v.cards.is_empty() {
        out.push(String::new());
        out.push(format!(
            "{} {} {} {}",
            pad("Card payment envelope", name_w, false),
            pad("Available", 12, true),
            pad("Owed", 12, true),
            pad("Underfunded", 12, true)
        ));
        for c in &v.cards {
            out.push(format!(
                "{} {} {} {}",
                pad(&c.card, name_w, false),
                num(c.available),
                num(c.owed),
                num(c.underfunded)
            ));
            // Rule 7a: more set aside than the card owes.
            if c.overfunded > Decimal::ZERO {
                out.push(format!(
                    "  {} more than it owes: lower Set aside to return it",
                    fmt(c.overfunded, 2)
                ));
            }
            // Rule 7: credit overspending still in categories is covered there, not on the card.
            for (category, amount) in &c.in_categories {
                out.push(format!(
                    "  {} overspent in {category}: cover it there",
                    fmt(*amount, 2)
                ));
            }
        }
    }
    out.push(String::new());
    out.push(format!(
        "{} {}",
        pad("Assigned to future months", name_w, false),
        num(v.assigned_to_future)
    ));
    out.push(format!(
        "{} {}",
        pad("Unassigned", name_w, false),
        num(v.unassigned)
    ));
    let notes: Vec<String> = v
        .flags
        .iter()
        .map(|f| ("flag", f))
        .chain(v.warnings.iter().map(|w| ("warning", w)))
        .map(|(kind, d)| format!("{kind}: {}:{}: {}", d.file, d.line, d.message))
        .chain(
            v.shortfalls
                .iter()
                .map(|s| format!("shortfall: {}", s.message)),
        )
        .collect();
    if !notes.is_empty() {
        out.push(String::new());
        out.extend(notes);
    }
    out.join("\n")
}

/// Splits arguments into positionals and options; every --key takes a value except boolean flags.
fn parse_args(args: &[String]) -> (Vec<String>, Opts) {
    const BOOLEAN: [&str; 1] = ["json"];
    let mut positional = Vec::new();
    let mut opts = Opts::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let Some(key) = a.strip_prefix("--") else {
            positional.push(a.clone());
            continue;
        };
        let value = if BOOLEAN.contains(&key) {
            String::new()
        } else {
            it.next().cloned().unwrap_or_default()
        };
        opts.entry(key.to_owned()).or_default().push(value);
    }
    (positional, opts)
}

pub fn first<'a>(opts: &'a Opts, key: &str) -> Option<&'a str> {
    opts.get(key)?.first().map(String::as_str)
}

fn budget(book: &mut Book, opts: &Opts) -> ExitCode {
    let today = first(opts, "today").map_or_else(
        || chrono::Local::now().format("%Y-%m-%d").to_string(),
        str::to_owned,
    );
    let (Some(month), Ok(today)) = (
        first(opts, "month").and_then(|m| m.parse::<Month>().ok()),
        today.parse::<Date>(),
    ) else {
        return usage();
    };
    let errors = book.errors();
    for e in &errors {
        eprintln!("error: {}:{}: {}", e.file, e.line, e.message);
    }
    for d in book.budget_diagnostics() {
        eprintln!("budget: {}:{}: {}", d.file, d.line, d.message);
    }
    println!("{}", render_budget(&book.budget(month, Some(today))));
    ExitCode::from(u8::from(!errors.is_empty()))
}

fn check(book: &mut Book, opts: &Opts) -> ExitCode {
    let errors = book.errors();
    let mut accounts: Vec<(&String, &IndexMap<String, Decimal>)> = book.balances().iter().collect();
    accounts.sort_by(|a, b| a.0.cmp(b.0));
    let balances: Vec<(String, Vec<(String, String)>)> = accounts
        .into_iter()
        .map(|(acct, by_cur)| {
            let values = by_cur
                .iter()
                .map(|(cur, v)| (cur.clone(), plain(*v)))
                .collect();
            (acct.clone(), values)
        })
        .collect();
    if opts.contains_key("json") {
        let balances: serde_json::Map<String, Value> = balances
            .iter()
            .map(|(acct, values)| {
                let values = values.iter().map(|(c, v)| (c.clone(), json!(v))).collect();
                (acct.clone(), Value::Object(values))
            })
            .collect();
        let errors: Vec<Value> = errors
            .iter()
            .map(|e| json!({ "file": &*e.file, "line": e.line, "code": e.code, "message": e.message }))
            .collect();
        let doc = json!({ "balances": balances, "errors": errors });
        println!(
            "{}",
            serde_json::to_string_pretty(&doc).expect("JSON values serialize")
        );
    } else {
        for e in &errors {
            println!("{}:{}: {}: {}", e.file, e.line, e.code, e.message);
        }
        for (acct, values) in &balances {
            for (cur, v) in values {
                println!("{} {} {cur}", pad(acct, 40, false), pad(v, 14, true));
            }
        }
    }
    ExitCode::from(u8::from(!errors.is_empty()))
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some((cmd, rest)) = argv.split_first() else {
        return usage();
    };
    let (positional, opts) = parse_args(rest);
    let Some(target) = positional.first() else {
        return usage();
    };
    if write::WRITE_COMMANDS.contains(&cmd.as_str()) {
        return write::run(cmd, &absolute(target), &opts);
    }
    let mut book = Book::load(&to_posix(&absolute(target)), &DiskSource);
    match cmd.as_str() {
        "budget" => budget(&mut book, &opts),
        "check" => check(&mut book, &opts),
        _ => usage(),
    }
}
