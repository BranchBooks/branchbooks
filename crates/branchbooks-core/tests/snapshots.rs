//! Snapshots of the core: each `tests/snapshots/*.txt` is everything the core reports for one
//! ledger. A change that alters one is a change in behaviour, to be made on purpose: rerun with
//! `UPDATE_SNAPSHOTS=1 cargo test -p branchbooks-core --test snapshots` and review the diff.
//! Real Beancount (`branchbooks-oracle`) remains the outside check on balances and errors.

use std::fs;
use std::path::{Path, PathBuf};

use branchbooks_core::book::Book;
use branchbooks_core::date::Date;
use branchbooks_core::ledger::{build_ledger, compute_balances};
use branchbooks_core::loader::{FileSource, load};
use branchbooks_core::money::Decimal;
use branchbooks_core::reports::{balance_sheet, income_statement};
use branchbooks_core::types::{Diagnostic, Entry};

fn q(s: &str) -> String {
    format!("{s:?}")
}

fn n(v: Decimal) -> String {
    v.normalize().to_string()
}

fn diag(d: &Diagnostic) -> String {
    format!("{} {} {} {}", q(&d.file), d.line, d.code, q(&d.message))
}

fn nums(vs: &[Decimal]) -> String {
    vs.iter().map(|v| n(*v)).collect::<Vec<_>>().join(" ")
}

/// The Book's view: its balances, reports, budget diagnostics, and every month's budget.
fn book_lines(book: &mut Book, lines: &mut Vec<String>) {
    for (account, by_cur) in book.balances() {
        for (cur, v) in by_cur {
            lines.push(format!("book-balance {account} {cur} {}", n(*v)));
        }
    }
    let currency = book.currency().to_owned();
    let is = income_statement(book.transactions(), &currency, None, None);
    lines.push(format!(
        "income {} expenses {} net {}",
        n(is.income),
        n(is.expenses),
        n(is.net)
    ));
    for l in is.income_lines.iter().chain(&is.expense_lines) {
        lines.push(format!("  line {} {}", l.account, n(l.amount)));
    }
    let bs = balance_sheet(book.transactions(), &currency, None);
    lines.push(format!(
        "assets {} liabilities {} equity {} net-worth {}",
        n(bs.assets),
        n(bs.liabilities),
        n(bs.equity),
        n(bs.net_worth)
    ));
    for l in bs.asset_lines.iter().chain(&bs.liability_lines) {
        lines.push(format!("  line {} {}", l.account, n(l.amount)));
    }
    for d in book.budget_diagnostics() {
        lines.push(format!("budget-diagnostic {}", diag(&d)));
    }
    for d in book.blocked(None) {
        lines.push(format!("blocked {}", diag(&d)));
    }

    // Every month from the first entry's to the one after the last, so carries show too.
    let mut dates: Vec<Date> = book.entries().map(|(_, e)| e.date).collect();
    dates.sort();
    let (Some(first), Some(last)) = (dates.first(), dates.last()) else {
        return;
    };
    let (first, last) = (first.month(), last.month().next());
    let mut m = first;
    while m <= last {
        let v = book.budget(m, None);
        lines.push(format!(
            "month {} {} today {} unassigned {} future {} cash {}",
            v.month,
            v.currency,
            v.today,
            n(v.unassigned),
            n(v.assigned_to_future),
            n(v.cash)
        ));
        for c in &v.categories {
            let values = [
                c.carried_in,
                c.assigned,
                c.moved,
                c.activity,
                c.available,
                c.cash_overspent,
                c.credit_overspent,
            ];
            lines.push(format!("  category {} {}", c.category, nums(&values)));
        }
        for c in &v.cards {
            lines.push(format!(
                "  card {} {}",
                c.card,
                nums(&[c.available, c.owed, c.underfunded])
            ));
            if !c.overfunded.is_zero() {
                lines.push(format!("    overfunded {}", n(c.overfunded)));
            }
            for (category, v) in &c.in_categories {
                lines.push(format!("    waiting {category} {}", n(*v)));
            }
        }
        for d in &v.warnings {
            lines.push(format!("  warning {}", diag(d)));
        }
        for d in &v.flags {
            lines.push(format!("  flag {}", diag(d)));
        }
        m = m.next();
    }
    // Unassigned seen from a later today: future assignments drop out as their months pass.
    let today: Date = format!("{last}-15").parse().unwrap();
    let unassigned = book.budget(first, Some(today)).unassigned;
    lines.push(format!("unassigned-at {today} {}", n(unassigned)));
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Reads paths relative to the repository, which is how snapshots name their ledgers.
struct RepoSource;

impl FileSource for RepoSource {
    fn read(&self, path: &str) -> Option<String> {
        fs::read_to_string(repo().join(path)).ok()
    }
}

fn entry_lines(e: &Entry, out: &mut Vec<String>) {
    out.push(format!(
        "entry {} {} {} {} {} {}",
        e.entry_type().name(),
        q(&e.file),
        e.line,
        e.end_line,
        e.seq,
        e.date
    ));
    for p in e.txn().map_or(&[][..], |t| &t.postings) {
        let units = p.units.as_ref().map_or("-".to_owned(), |u| {
            format!("{} {} {}", n(u.number), u.currency, u.scale)
        });
        out.push(format!("  posting {} {} {units}", p.account, p.line));
    }
}

fn dump(path: &str) -> String {
    let loaded = load(path, &RepoSource, None);
    let ledger = build_ledger(&loaded);
    let mut lines = vec![format!("# {path}")];
    lines.push(format!(
        "files {}",
        loaded
            .files
            .iter()
            .map(|f| q(f))
            .collect::<Vec<_>>()
            .join(" ")
    ));
    for (k, v) in &loaded.options {
        lines.push(format!(
            "option {} {}",
            q(k),
            v.iter().map(|s| q(s)).collect::<Vec<_>>().join(" ")
        ));
    }
    for e in &ledger.entries {
        entry_lines(e, &mut lines);
    }
    for d in &ledger.errors {
        lines.push(format!(
            "error {} {} {} {}",
            q(&d.file),
            d.line,
            d.code,
            q(&d.message)
        ));
    }
    for (account, by_cur) in compute_balances(ledger.transactions(), None) {
        for (cur, v) in by_cur {
            lines.push(format!("balance {account} {cur} {}", n(v)));
        }
    }
    book_lines(
        &mut Book::from_load(load(path, &RepoSource, None)),
        &mut lines,
    );
    lines.join("\n") + "\n"
}

#[test]
fn matches_every_snapshot() {
    let update = std::env::var_os("UPDATE_SNAPSHOTS").is_some();
    let dir = repo().join("crates/branchbooks-core/tests/snapshots");
    let mut snapshots: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "txt"))
        .collect();
    snapshots.sort();
    assert!(
        snapshots.len() >= 20,
        "expected the fixture snapshots, found {}",
        snapshots.len()
    );

    let mut failures = Vec::new();
    for snap in &snapshots {
        let expected = fs::read_to_string(snap).unwrap();
        let path = expected
            .strip_prefix("# ")
            .and_then(|rest| rest.lines().next())
            .unwrap();
        let actual = dump(path);
        if actual != expected && update {
            fs::write(snap, &actual).unwrap();
        } else if actual != expected {
            let first = expected
                .lines()
                .zip(actual.lines())
                .position(|(a, b)| a != b);
            let at = first.unwrap_or(expected.lines().count().min(actual.lines().count()));
            failures.push(format!(
                "{path}: first difference at line {}\n  expected: {}\n  actual:   {}",
                at + 1,
                expected.lines().nth(at).unwrap_or("<end>"),
                actual.lines().nth(at).unwrap_or("<end>")
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} differ:\n{}",
        failures.len(),
        snapshots.len(),
        failures.join("\n")
    );
}
