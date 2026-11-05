//! Importing entries from other Beancount files:
//! new entries only, in place, in one commit.

mod common;

use branchbooks_core::date::Date;
use branchbooks_core::write::import::{ImportKind, import_root};
use branchbooks_core::write::store::{MemoryStore, Store};
use branchbooks_core::write::workspace::{ApplyError, OperationErrorCode, Workspace};
use common::{d, fixtures, m};
use indexmap::IndexMap;

fn multi_files() -> Vec<(String, String)> {
    ["main.beancount", "2025.beancount", "2026.beancount"]
        .map(|f| {
            (
                f.to_owned(),
                std::fs::read_to_string(fixtures().join("multifile").join(f)).unwrap(),
            )
        })
        .into()
}

fn ids() -> Box<dyn FnMut() -> String> {
    let mut n = 0;
    Box::new(move || {
        n += 1;
        format!("op-{n}")
    })
}

fn multi() -> Workspace<MemoryStore> {
    let files = multi_files();
    Workspace::open(
        MemoryStore::new(files.iter().map(|(a, b)| (a.as_str(), b.as_str()))),
        None,
        ids(),
    )
    .unwrap()
}

fn files(list: &[(&str, &str)]) -> IndexMap<String, String> {
    list.iter()
        .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
        .collect()
}

/// A "bank export": one payment the ledger already has (worded differently, amount elided the
/// other way round), one new payment, and a new account.
const BANK: &str = r#"option "title" "Bank export"
option "operating_currency" "USD"

2026-01-05 * "VISA PAYMENT"
  Assets:Checking            -80.25 USD
  Liabilities:Visa

2026-01-20 * "Corner Shop" "Milk"
  Expenses:Food:Groceries     12.40 USD
  Assets:Checking

2026-01-21 * "Bus"
  Expenses:Transport:Bus       2.75 USD
  Assets:Checking
"#;

fn bank() -> IndexMap<String, String> {
    files(&[("bank.beancount", BANK)])
}

#[test]
fn picks_the_root_of_a_set_of_files() {
    assert_eq!(
        import_root(&files(&[("a.beancount", "")])).as_deref(),
        Some("a.beancount")
    );
    let with_include = files(&[
        ("x.beancount", "include \"y.beancount\"\n"),
        ("y.beancount", ""),
    ]);
    assert_eq!(import_root(&with_include).as_deref(), Some("x.beancount"));
    assert_eq!(
        import_root(&files(&[("main.beancount", ""), ("y.beancount", "")])).as_deref(),
        Some("main.beancount")
    );
    assert_eq!(
        import_root(&files(&[("a.beancount", ""), ("b.beancount", "")])),
        None
    );
}

#[test]
fn previews_skips_what_the_ledger_has_opens_accounts_on_first_use_ignores_options() {
    let mut ws = multi();
    let p = ws.preview_import(&bank());
    assert_eq!(p.errors, vec![]);
    assert_eq!(p.duplicates, 1);
    assert_eq!(p.opens, ["Expenses:Transport:Bus"]);
    let add: Vec<(ImportKind, Date)> = p.add.iter().map(|i| (i.kind, i.date)).collect();
    assert_eq!(
        add,
        [
            (ImportKind::Txn, d("2026-01-20")),
            (ImportKind::Txn, d("2026-01-21")),
            (ImportKind::Open, d("2026-01-21"))
        ]
    );
    assert_eq!(
        p.ignored,
        ["option \"title\"", "option \"operating_currency\""]
    );
}

#[test]
fn places_entries_in_date_order_in_their_files_and_commits_once() {
    let mut ws = multi();
    let before = ws.store.snapshot().unwrap();
    let (_, result) = ws.import_files(&bank(), 0).unwrap();
    let result = result.unwrap();
    assert_eq!(result.op_type, "import");
    let mut changed = result.changed.clone();
    changed.sort();
    assert_eq!(changed, ["2026.beancount", "main.beancount"]);
    let after = ws.store.snapshot().unwrap();
    let y = &after.files["2026.beancount"];
    assert!(y.find("\"Corner Shop\" \"Milk\"").unwrap() > y.find("Paycheck").unwrap());
    assert!(y.find("\"Bus\"").unwrap() > y.find("\"Corner Shop\"").unwrap());
    // Every line of the old file is still there, in order.
    let old: Vec<&str> = before.files["2026.beancount"].split('\n').collect();
    let mut i = 0;
    for line in y.split('\n') {
        if old.get(i) == Some(&line) {
            i += 1;
        }
    }
    assert_eq!(i, old.len());
    assert!(after.files["main.beancount"].contains("2026-01-21 open Expenses:Transport:Bus\n"));
    assert_eq!(ws.book().errors(), vec![]);
    assert_eq!(ws.core.version, after.version);
    assert_eq!(
        ws.store.log[0].info.summary,
        "Import 2 transactions, 1 account opening from bank.beancount"
    );
}

#[test]
fn importing_the_same_file_again_adds_nothing() {
    let mut ws = multi();
    ws.import_files(&bank(), 0).unwrap();
    let (preview, result) = ws.import_files(&bank(), 0).unwrap();
    assert!(result.is_none());
    assert_eq!(preview.duplicates, 3);
}

#[test]
fn a_balance_assertion_imported_again_is_a_duplicate() {
    let mut ws = multi();
    let once = "2025-12-01 balance Assets:Checking  0.00 USD\n";
    let (preview, result) = ws
        .import_files(&files(&[("a.beancount", once)]), 0)
        .unwrap();
    assert_eq!(preview.add[0].kind, ImportKind::Balance);
    assert!(result.is_some());
    // The same date, account and amount, spaced differently and with a comment.
    let again = "2025-12-01 balance   Assets:Checking 0.00 USD ; from the bank\n";
    let (preview, result) = ws
        .import_files(&files(&[("b.beancount", again)]), 0)
        .unwrap();
    assert!(result.is_none());
    assert_eq!((preview.duplicates, preview.add.len()), (1, 0));
}

#[test]
fn two_identical_payments_against_one_in_the_ledger_adds_one() {
    let mut ws = multi();
    let twice = "2026-01-05 * \"Card\"\n  Liabilities:Visa  80.25 USD\n  Assets:Checking\n\n2026-01-05 * \"Card\"\n  Liabilities:Visa  80.25 USD\n  Assets:Checking\n";
    let p = ws.preview_import(&files(&[("x.beancount", twice)]));
    assert_eq!((p.duplicates, p.add.len()), (1, 1));
}

#[test]
fn creates_a_year_file_and_its_include_for_a_new_year() {
    let mut ws = multi();
    let old = files(&[(
        "old.beancount",
        "2024-06-01 * \"Old\"\n  Expenses:Food:Groceries 5.00 USD\n  Assets:Checking\n",
    )]);
    // Assets:Checking opens in 2025-12: an earlier payment is a ledger error, and nothing is
    // written.
    assert!(ws.import_files(&old, 0).is_err());
    assert!(
        !ws.store
            .snapshot()
            .unwrap()
            .files
            .contains_key("2024.beancount")
    );

    let later = "2027-02-01 * \"Later\"\n  Expenses:Food:Groceries 5.00 USD\n  Assets:Checking\n";
    ws.import_files(&files(&[("later.beancount", later)]), 0)
        .unwrap();
    let snap = ws.store.snapshot().unwrap();
    assert_eq!(snap.files["2027.beancount"], later);
    assert!(
        snap.files["main.beancount"]
            .contains("include \"2026.beancount\"\ninclude \"2027.beancount\"\n")
    );
    assert_eq!(ws.book().errors(), vec![]);
}

#[test]
fn refuses_files_with_syntax_errors_and_entries_that_break_the_ledger() {
    let mut ws = multi();
    let bad = files(&[(
        "bad.beancount",
        "2026-01-01 * \"x\"\n  Assets:Checking 1.00 USD\n  ???\n",
    )]);
    assert!(matches!(
        ws.import_files(&bad, 0),
        Err(ApplyError::Operation(_))
    ));
    let early = files(&[(
        "early.beancount",
        "2024-06-01 * \"Old\"\n  Expenses:Food:Groceries 5.00 USD\n  Assets:Checking\n",
    )]);
    match ws.import_files(&early, 0) {
        Err(ApplyError::Operation(e)) => assert_eq!(e.code, OperationErrorCode::Rejected),
        other => panic!("expected rejected, got {other:?}"),
    }
}

#[test]
fn imports_a_multi_file_ledger_into_an_empty_ish_one_including_opens_and_budget_entries() {
    let store = MemoryStore::new([("main.beancount", "option \"operating_currency\" \"USD\"\n")]);
    let mut ws = Workspace::open(store, None, ids()).unwrap();
    let all: IndexMap<String, String> = multi_files().into_iter().collect();
    let (preview, result) = ws.import_files(&all, 0).unwrap();
    assert_eq!(preview.duplicates, 0);
    assert!(result.is_some());
    assert_eq!(ws.book().errors(), vec![]);
    let mut original = multi();
    assert_eq!(
        ws.book().transactions().len(),
        original.book().transactions().len()
    );
    assert_eq!(
        ws.book().budget(m("2026-01"), None).unassigned,
        original.book().budget(m("2026-01"), None).unassigned
    );
}
