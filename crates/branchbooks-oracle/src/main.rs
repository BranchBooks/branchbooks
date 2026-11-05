//! The oracle for ledger behaviour (docs/DESIGN.md, one oracle per question): runs every fixture
//! through real Beancount 3.2.3 (a separate Python process, `tools/oracle/beancount_dump.py`; never
//! linked) and through the `BranchBooks` core, and compares account balances and errors.
//!   cargo run -q -p branchbooks-oracle [ledger ...]
//! With no arguments: every fixture, every ledger in private/ (real ledgers, never committed or in
//! CI, which also get the checks in `private_checks`), and the ledgers the write path produces
//! (`writeback`). Set `BEANCOUNT_PYTHON` to the interpreter that has beancount 3.2.3; by default the
//! virtualenv in tools/oracle/.venv, else python3.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

mod writeback;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::str::FromStr;

use branchbooks_core::book::Book;
use branchbooks_core::date::Month;
use branchbooks_core::loader::FileSource;
use branchbooks_core::write::store::MemoryStore;
use branchbooks_core::write::workspace::Workspace;
use indexmap::IndexMap;
use rust_decimal::Decimal;
use serde_json::Value;

struct Disk;

impl FileSource for Disk {
    fn read(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }
}

fn repo() -> PathBuf {
    // Not canonicalize: on Windows it gives \\?\ paths, which the loader cannot follow includes from.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the repository")
        .to_path_buf()
}

/// Each ledger under `dir`: every .bean file, and each folder's main.beancount (its year files
/// are included from it).
fn ledgers(dir: &Path) -> Vec<PathBuf> {
    let mut names: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    names.sort();
    let mut out = Vec::new();
    for p in names {
        if p.is_dir() {
            out.extend(ledgers(&p));
        } else if p.extension().is_some_and(|x| x == "bean")
            || p.file_name().is_some_and(|n| n == "main.beancount")
        {
            out.push(p);
        }
    }
    out
}

fn python(repo: &Path) -> PathBuf {
    if let Some(p) = std::env::var_os("BEANCOUNT_PYTHON") {
        return p.into();
    }
    let venv = repo.join("tools/oracle/.venv");
    [venv.join("bin/python"), venv.join("Scripts/python.exe")]
        .into_iter()
        .find(|p| p.exists())
        .unwrap_or_else(|| "python3".into())
}

/// Beancount's error, in the code `BranchBooks` uses for the same condition.
fn beancount_code(kind: &str, message: &str) -> String {
    let has = |s: &str| message.contains(s);
    if kind.starts_with("Parser") || kind.starts_with("Lexer") {
        "syntax"
    } else if has("does not balance") {
        "unbalanced"
    } else if message.starts_with("Balance failed") {
        "balance"
    } else if has("unknown account") {
        "unknown-account"
    } else if has("inactive account") {
        "inactive-account"
    } else if has("Invalid currency") {
        "currency-constraint"
    } else if has("Duplicate open") {
        "duplicate-open"
    } else if has("Duplicate close") {
        "duplicate-close"
    } else if has("Unopened account") && has("is being closed") {
        "close-unopened"
    } else if [
        "auto-posting",
        "missing numbers",
        "interpolat",
        "Interpolat",
    ]
    .iter()
    .any(|s| has(s))
    {
        "booking"
    } else {
        return format!("other:{kind}");
    }
    .to_owned()
}

/// Nonzero balances as "account currency" → amount.
type Balances = HashMap<String, Decimal>;

/// Where `BranchBooks` and Beancount differ on one ledger.
fn compare(book: &mut Book, theirs: &Value, rel: &dyn Fn(&str) -> String) -> Vec<String> {
    let mut problems = Vec::new();
    let mut ours = Balances::new();
    for (account, by) in book.balances() {
        for (currency, v) in by {
            if !v.is_zero() {
                ours.insert(format!("{account} {currency}"), *v);
            }
        }
    }
    let mut refs = Balances::new();
    for (account, by) in theirs["balances"].as_object().into_iter().flatten() {
        for (currency, v) in by.as_object().into_iter().flatten() {
            let v = Decimal::from_str(v.as_str().unwrap_or("0")).unwrap_or_default();
            if !v.is_zero() {
                refs.insert(format!("{account} {currency}"), v);
            }
        }
    }
    let mut keys: Vec<&String> = ours.keys().chain(refs.keys()).collect();
    keys.sort();
    keys.dedup();
    for k in keys {
        let (a, b) = (ours.get(k), refs.get(k));
        if a != b {
            let show =
                |v: Option<&Decimal>| v.map_or("-".to_owned(), |d| d.normalize().to_string());
            problems.push(format!(
                "balance {k}: branchbooks {} beancount {}",
                show(a),
                show(b)
            ));
        }
    }

    // Errors: the same multiset of (file, line, code).
    let mut count: IndexMap<String, (i32, Option<String>)> = IndexMap::new();
    for e in book.errors() {
        count
            .entry(format!("{}:{} {}", rel(&e.file), e.line, e.code))
            .or_default()
            .0 += 1;
    }
    for e in theirs["errors"].as_array().into_iter().flatten() {
        let line = e["line"].as_u64().map_or("?".to_owned(), |l| l.to_string());
        let message = e["message"].as_str().unwrap_or("");
        let code = beancount_code(e["type"].as_str().unwrap_or(""), message);
        let slot = count
            .entry(format!(
                "{}:{line} {code}",
                rel(e["file"].as_str().unwrap_or("?"))
            ))
            .or_default();
        slot.0 -= 1;
        slot.1
            .get_or_insert_with(|| message.lines().next().unwrap_or("").to_owned());
    }
    for (k, (diff, msg)) in count {
        if diff != 0 {
            let msg = msg.map_or(String::new(), |m| format!(" (beancount: {m})"));
            problems.push(format!(
                "error {k}: branchbooks {} beancount{msg}",
                diff_text(diff)
            ));
        }
    }
    problems
}

fn diff_text(diff: i32) -> String {
    if diff > 0 {
        format!("{diff} more")
    } else {
        format!("{} fewer", -diff)
    }
}

/// A real ledger's own checks: no ledger errors, a budget for every month it covers, and an
/// import into an empty ledger that ends with the same balances.
fn private_checks(book: &mut Book, path: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    for e in book.errors() {
        problems.push(format!("ledger error {}:{}: {}", e.file, e.line, e.message));
    }
    let mut months: Vec<Month> = book.entries().map(|(_, e)| e.date.month()).collect();
    months.sort();
    if let (Some(&first), Some(&last)) = (months.first(), months.last()) {
        let end = last.next();
        let mut m = first;
        while m <= end {
            if book.budget(m, None).month != m {
                problems.push(format!("no budget for {m}"));
            }
            m = m.next();
        }
    }
    let dir = path.parent().unwrap_or(Path::new("."));
    let files: IndexMap<String, String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|x| x == "beancount" || x == "bean")
        })
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            Some((name, std::fs::read_to_string(e.path()).ok()?))
        })
        .collect();
    let empty = format!("option \"operating_currency\" \"{}\"\n", book.currency());
    let mut n = 0;
    let ids = Box::new(move || {
        n += 1;
        format!("import-{n}")
    });
    let mut ws = Workspace::open(
        MemoryStore::new([("main.beancount", empty.as_str())]),
        None,
        ids,
    )
    .expect("an empty store opens");
    match ws.import_files(&files, 0) {
        Ok((_, Some(_))) => {
            let flat = |b: &mut Book| {
                let mut v: Vec<String> = b
                    .balances()
                    .iter()
                    .flat_map(|(a, by)| {
                        by.iter()
                            .filter(|(_, v)| !v.is_zero())
                            .map(move |(c, v)| format!("{a} {c} {}", v.normalize()))
                    })
                    .collect();
                v.sort();
                v
            };
            if flat(ws.book()) != flat(book) {
                problems.push("an import into an empty ledger ends with other balances".into());
            }
            if !ws.book().errors().is_empty() {
                problems.push("an import into an empty ledger has ledger errors".into());
            }
        }
        Ok((_, None)) => problems.push("an import into an empty ledger added nothing".into()),
        Err(e) => problems.push(format!("an import into an empty ledger was refused: {e:?}")),
    }
    problems
}

fn main() -> ExitCode {
    let repo = repo();
    let args: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let private = ledgers(&repo.join("private"));
    let files: Vec<PathBuf> = if args.is_empty() {
        if !private.is_empty() {
            println!(
                "Also checking {} ledger(s) in private/ (not committed)",
                private.len()
            );
        }
        let mut f = ledgers(&repo.join("fixtures"));
        f.extend(private.iter().cloned());
        f.extend(writeback::write(&repo.join("out/writeback")));
        f
    } else {
        args.iter()
            .map(|a| std::path::absolute(a).expect("a path"))
            .collect()
    };
    let out = Command::new(python(&repo))
        .arg("-I")
        .arg(repo.join("tools/oracle/beancount_dump.py"))
        .args(&files)
        .output()
        .expect("python runs");
    if !out.status.success() {
        eprintln!(
            "beancount_dump.py failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        return ExitCode::FAILURE;
    }
    let dump: Value = serde_json::from_slice(&out.stdout).expect("JSON from beancount_dump.py");
    println!(
        "Oracle: Beancount {}",
        dump["beancount"].as_str().unwrap_or("?")
    );
    let root = format!("{}/", repo.display());
    let rel = |p: &str| p.strip_prefix(&root).unwrap_or(p).replace('\\', "/");
    let mut failed = 0;
    for f in &files {
        let path = f.to_string_lossy().replace('\\', "/");
        let theirs = &dump["results"][f.to_string_lossy().as_ref()];
        let mut book = Book::load(&path, &Disk);
        let mut problems = compare(&mut book, theirs, &rel);
        if private.contains(f) {
            problems.extend(private_checks(&mut book, f));
        }
        let errs = theirs["errors"].as_array().map_or(0, Vec::len);
        let verdict = if problems.is_empty() { "PASS" } else { "FAIL" };
        println!("{verdict}  {}  ({errs} beancount error(s))", rel(&path));
        for p in &problems {
            println!("      {p}");
        }
        failed += usize::from(!problems.is_empty());
    }
    if failed > 0 {
        println!("{failed} of {} ledgers differ", files.len());
        ExitCode::FAILURE
    } else {
        println!("All {} ledgers match Beancount", files.len());
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beancount_errors_map_to_branchbooks_codes() {
        assert_eq!(beancount_code("ParserError", "x"), "syntax");
        assert_eq!(
            beancount_code("ValidationError", "Transaction does not balance: 1 USD"),
            "unbalanced"
        );
        assert_eq!(
            beancount_code("BalanceError", "Balance failed for 'Assets:Cash'"),
            "balance"
        );
        assert_eq!(
            beancount_code(
                "ValidationError",
                "Unopened account Assets:X is being closed"
            ),
            "close-unopened"
        );
        assert_eq!(beancount_code("Odd", "something else"), "other:Odd");
    }
}
