//! Balance assertions: the checks Beancount 3.2.3 makes (fixtures/balance/assertions.bean, which
//! the oracle also compares), and the write path refusing a change that breaks one.

mod common;

use std::cell::Cell;
use std::rc::Rc;

use branchbooks_core::book::Book;
use branchbooks_core::ledger::build_ledger;
use branchbooks_core::loader::{MemorySource, load};
use branchbooks_core::parser::parse;
use branchbooks_core::types::Diagnostic;
use branchbooks_core::write::render::{EntryInput, PostingInput, TransactionInput};
use branchbooks_core::write::store::MemoryStore;
use branchbooks_core::write::workspace::{ApplyError, Operation, OperationErrorCode, Workspace};
use common::fixtures;

type Ws = Workspace<MemoryStore>;

const LEDGER: &str = r#"option "operating_currency" "USD"
2026-01-01 open Assets:Checking USD
  budget: "on"
2026-01-01 open Income:Salary USD
2026-01-01 open Expenses:Food USD

2026-01-02 * "Paycheck"
  Assets:Checking  1000.00 USD
  Income:Salary

2026-02-01 balance Assets:Checking  ASSERTED USD
"#;

fn lines_and_codes(errors: &[Diagnostic]) -> Vec<(u32, &str)> {
    errors.iter().map(|d| (d.line, d.code.as_str())).collect()
}

fn open(asserted: &str) -> Ws {
    let text = LEDGER.replace("ASSERTED", asserted);
    let n = Rc::new(Cell::new(0));
    let ids = Box::new(move || {
        n.set(n.get() + 1);
        format!("op-{}", n.get())
    });
    Workspace::open(
        MemoryStore::new([("main.beancount", text.as_str())]),
        None,
        ids,
    )
    .unwrap()
}

fn spend(ws: &mut Ws, date: &str, amount: &str) -> Result<(), OperationErrorCode> {
    let posting = |account: &str, amount: Option<&str>| PostingInput {
        account: account.into(),
        amount: amount.map(Into::into),
        ..Default::default()
    };
    let entry = EntryInput::Txn(TransactionInput {
        date: date.into(),
        narration: "Groceries".into(),
        postings: vec![
            posting("Expenses:Food", Some(amount)),
            posting("Assets:Checking", None),
        ],
        ..Default::default()
    });
    match ws.apply(&Operation::Add(entry), None, 0) {
        Ok(_) => Ok(()),
        Err(ApplyError::Operation(e)) => Err(e.code),
        Err(e) => panic!("{e:?}"),
    }
}

/// The incremental Book after the operations equals a full load of the files they wrote.
fn same_as_a_full_load(ws: &mut Ws) {
    let text = ws.text("main.beancount").unwrap().to_owned();
    let mut full = Book::load(
        "main.beancount",
        &MemorySource::new([("main.beancount", text.as_str())]),
    );
    assert_eq!(ws.book().errors(), full.errors());
}

#[test]
fn the_fixture_has_exactly_the_errors_beancount_reports() {
    let path = fixtures().join("balance/assertions.bean");
    let text = std::fs::read_to_string(&path).unwrap();
    let source = MemorySource::new([("assertions.bean", text.as_str())]);
    let expected = [
        (46, "balance"),
        (52, "balance"),
        (55, "balance"),
        (61, "currency-constraint"),
        (64, "unknown-account"),
        (64, "unknown-account"),
        (67, "inactive-account"),
    ];
    let mut book = Book::load("assertions.bean", &source);
    let errors = book.errors();
    assert_eq!(lines_and_codes(&errors), expected);
    assert_eq!(
        errors[0].message,
        "Balance failed for 'Assets:Checking': expected 640.000 USD != accumulated 639.996 USD (0.004 too little)"
    );
    assert!(errors[2].message.ends_with("(50.00 too much)"));
    let ledger = build_ledger(&load("assertions.bean", &source, None));
    assert_eq!(lines_and_codes(&ledger.errors), expected);
}

#[test]
fn parses_the_amount_and_an_optional_tolerance() {
    let r = parse("2026-01-01 balance Assets:A  1.50 ~ 0.25 USD\n", "a.bean");
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let b = r.entries[0].balance().unwrap();
    assert_eq!(
        (b.amount.number.to_string(), b.amount.scale),
        ("1.50".into(), 2)
    );
    assert_eq!(b.tolerance.map(|t| t.to_string()).as_deref(), Some("0.25"));
    let r = parse("2026-01-01 balance Assets:A  1.50\n", "a.bean");
    assert_eq!(r.errors[0].code, "syntax");
}

#[test]
fn the_write_path_refuses_a_change_that_breaks_an_assertion() {
    let mut ws = open("1000.00");
    // Before the assertion's day: it would no longer hold.
    assert_eq!(
        spend(&mut ws, "2026-01-15", "20.00"),
        Err(OperationErrorCode::Rejected)
    );
    // On its day the assertion is checked before the day's transactions.
    assert_eq!(spend(&mut ws, "2026-02-01", "20.00"), Ok(()));
    // Deleting the paycheck breaks it too.
    let paycheck = ws
        .core
        .entries()
        .map(|(_, e)| e)
        .find(|e| e.txn().is_some())
        .unwrap()
        .clone();
    let r = ws.core.ref_of(&paycheck);
    assert!(matches!(
        ws.apply(&Operation::Delete(r), None, 0),
        Err(ApplyError::Operation(_))
    ));
    assert!(ws.book().errors().is_empty());
    same_as_a_full_load(&mut ws);
}

#[test]
fn an_assertion_that_already_fails_does_not_block_other_changes() {
    let mut ws = open("900.00");
    assert_eq!(lines_and_codes(&ws.book().errors()), [(11, "balance")]);
    // Still failing afterwards, by a different amount: not a new error, so not refused.
    assert_eq!(spend(&mut ws, "2026-01-15", "50.00"), Ok(()));
    // And the change that makes it hold is accepted too.
    assert_eq!(spend(&mut ws, "2026-01-16", "50.00"), Ok(()));
    assert!(ws.book().errors().is_empty());
    same_as_a_full_load(&mut ws);
}
