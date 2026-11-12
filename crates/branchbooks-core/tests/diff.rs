//! Budget diffs between two Books: the intent (target rules) and the effect (allocations,
//! Available, shortfalls and Unassigned).

mod common;

use branchbooks_core::book::Book;
use branchbooks_core::budget::Recurrence;
use branchbooks_core::date::Month;
use branchbooks_core::diff::{Effect, diff_books};
use branchbooks_core::loader::MemorySource;
use branchbooks_core::money::plain;
use branchbooks_core::write::render::{EntryInput, TargetInput};
use branchbooks_core::write::store::MemoryStore;
use branchbooks_core::write::workspace::{ApplyError, Operation, OperationErrorCode, Workspace};
use common::*;

const LEDGER: &str = r#"option "operating_currency" "USD"
2026-01-01 open Assets:Checking USD
  budget: "on"
2026-01-01 open Income:Salary USD
2026-01-01 open Expenses:Housing:Rent USD
2026-01-01 open Expenses:Savings:CarFund USD
2026-01-01 open Expenses:Food:Groceries USD

2026-01-01 * "Paycheck"
  Income:Salary  -5000.00 USD
  Assets:Checking

2026-01-01 custom "envelope-assign" Expenses:Food:Groceries 400.00 USD
2026-01-01 custom "target" Expenses:Housing:Rent 1400.00 USD
2026-01-01 custom "target" Expenses:Savings:CarFund 6000.00 USD
  due: 2026-06-01

2026-01-03 * "Rent"
  Expenses:Housing:Rent  1400.00 USD
  Assets:Checking
"#;

fn book(text: &str) -> Book {
    Book::load(
        "/main.beancount",
        &MemorySource::new([("/main.beancount", text)]),
    )
}

fn effect(assigned: i64, target_allocated: i64, available: i64, shortfall: i64) -> Effect {
    Effect {
        assigned: dec(&assigned.to_string()),
        target_allocated: dec(&target_allocated.to_string()),
        available: dec(&available.to_string()),
        shortfall: dec(&shortfall.to_string()),
    }
}

#[test]
fn a_moved_deadline_shows_the_rule_change_and_the_allocations_it_moves() {
    let mut trunk = book(LEDGER);
    // The scenario: "Car Fund: 6,000 by June" becomes "6,000 by December".
    let mut branch = book(&LEDGER.replace("due: 2026-06-01", "due: 2026-12-01"));
    let d = diff_books(&mut trunk, &mut branch, m("2026-01"), m("2026-02"));

    // Intent: one rule, changed for both months.
    assert_eq!(d.rules.len(), 1);
    let r = &d.rules[0];
    assert_eq!(
        (r.category.as_str(), r.from, r.to),
        ("Expenses:Savings:CarFund", m("2026-01"), m("2026-02"))
    );
    let (before, after) = (r.before.as_ref().unwrap(), r.after.as_ref().unwrap());
    assert_eq!(
        (before.due, after.due),
        ("2026-06-01".parse().ok(), "2026-12-01".parse().ok())
    );
    assert_eq!(after.recurrence, Recurrence::Once);

    // Effect: the monthly allocation drops from 1,000 to 500. In February trunk runs short (no
    // paycheck), the branch does not.
    let changes: Vec<(Month, &str, Effect, Effect)> = d
        .categories
        .iter()
        .map(|c| (c.month, c.category.as_str(), c.before, c.after))
        .collect();
    assert_eq!(
        changes,
        [
            (
                m("2026-01"),
                "Expenses:Savings:CarFund",
                effect(0, 1000, 1000, 0),
                effect(0, 500, 500, 0)
            ),
            (
                m("2026-02"),
                "Expenses:Savings:CarFund",
                effect(0, 800, 1800, 200),
                effect(0, 500, 1000, 0)
            ),
        ]
    );
    let unassigned: Vec<(Month, String, String)> = d
        .unassigned
        .iter()
        .map(|c| (c.month, plain(c.before), plain(c.after)))
        .collect();
    assert_eq!(
        unassigned,
        [
            (m("2026-01"), "2200".into(), "2700".into()),
            (m("2026-02"), "0".into(), "800".into())
        ]
    );
}

#[test]
fn identical_books_have_no_diff_and_a_rule_dated_differently_is_the_same_rule() {
    let d = diff_books(
        &mut book(LEDGER),
        &mut book(LEDGER),
        m("2026-01"),
        m("2026-03"),
    );
    assert_eq!(
        (d.rules.len(), d.categories.len(), d.unassigned.len()),
        (0, 0, 0)
    );
    // The same Rent target written on January 2 instead of January 1 changes nothing.
    let later = LEDGER.replace(
        "2026-01-01 custom \"target\" Expenses:Housing:Rent",
        "2026-01-02 custom \"target\" Expenses:Housing:Rent",
    );
    let d = diff_books(
        &mut book(LEDGER),
        &mut book(&later),
        m("2026-01"),
        m("2026-03"),
    );
    assert_eq!(d.rules, vec![]);
}

#[test]
fn an_added_and_an_ended_target_are_rule_changes_from_their_months() {
    let ended = format!("{LEDGER}2026-02-01 custom \"target\" Expenses:Housing:Rent 0.00 USD\n");
    let d = diff_books(
        &mut book(LEDGER),
        &mut book(&ended),
        m("2026-01"),
        m("2026-03"),
    );
    assert_eq!(d.rules.len(), 1);
    let r = &d.rules[0];
    assert_eq!(
        (r.category.as_str(), r.from, r.to),
        ("Expenses:Housing:Rent", m("2026-02"), m("2026-03"))
    );
    assert!(r.before.is_some() && r.after.is_none());
}

fn ws() -> Workspace<MemoryStore> {
    let mut n = 0;
    Workspace::open(
        MemoryStore::new([("main.beancount", LEDGER)]),
        None,
        Box::new(move || {
            n += 1;
            format!("op-{n}")
        }),
    )
    .unwrap()
}

fn target(date: &str, category: &str, amount: &str, due: Option<&str>) -> EntryInput {
    EntryInput::Target(TargetInput {
        date: date.into(),
        category: category.into(),
        amount: amount.into(),
        due: due.map(Into::into),
        ..Default::default()
    })
}

#[test]
fn targets_are_set_edited_and_ended_through_the_write_path() {
    let mut ws = ws();
    let r = ws
        .apply(
            &Operation::Add(target(
                "2026-02-01",
                "Expenses:Food:Groceries",
                "450.00",
                None,
            )),
            None,
            0,
        )
        .unwrap();
    assert_eq!(r.op_type, "set-target");
    assert_eq!(
        ws.store.log[0].info.summary,
        "Target 450.00 USD for Expenses:Food:Groceries from 2026-02-01"
    );
    assert!(
        ws.text("main.beancount")
            .unwrap()
            .contains("2026-02-01 custom \"target\" Expenses:Food:Groceries 450.00 USD\n")
    );
    // Groceries carried 400 into February, so the refill is 50.
    eq(
        cat(&ws.book().budget(m("2026-02"), None), "Groceries").target_allocated,
        50,
    );

    let e = ws
        .core
        .entries()
        .map(|(_, e)| e)
        .find(|e| {
            e.custom().is_some_and(|c| c.name == "target")
                && e.date == "2026-01-01".parse().unwrap()
                && e.custom().unwrap().values.len() == 2
                && e.meta.get("due").is_some()
        })
        .unwrap()
        .clone();
    let r = ws.core.ref_of(&e);
    let moved = target(
        "2026-01-01",
        "Expenses:Savings:CarFund",
        "6000.00",
        Some("2026-12-01"),
    );
    let result = ws.apply(&Operation::Edit(r, moved), None, 0).unwrap();
    assert_eq!(result.op_type, "edit-target");
    assert!(
        ws.text("main.beancount")
            .unwrap()
            .contains("6000.00 USD\n  due: 2026-12-01\n")
    );
    eq(
        cat(&ws.book().budget(m("2026-01"), None), "CarFund").target_allocated,
        500,
    );

    let r = ws
        .apply(
            &Operation::Add(target(
                "2026-03-01",
                "Expenses:Food:Groceries",
                "0.00",
                None,
            )),
            None,
            0,
        )
        .unwrap();
    assert_eq!(r.op_type, "end-target");
    assert!(
        cat(&ws.book().budget(m("2026-03"), None), "Groceries")
            .target
            .is_none()
    );
}

#[test]
fn an_invalid_target_is_refused_and_nothing_is_written() {
    let mut ws = ws();
    let op = Operation::Add(target("2026-02-01", "Income:Salary", "10.00", None));
    match ws.apply(&op, None, 0) {
        Err(ApplyError::Operation(e)) => {
            assert_eq!(e.code, OperationErrorCode::Invalid);
            assert_eq!(
                e.message,
                "Income:Salary is not a budgeted category or liability"
            );
        }
        other => panic!("expected invalid, got {other:?}"),
    }
    assert!(ws.store.log.is_empty());
}
