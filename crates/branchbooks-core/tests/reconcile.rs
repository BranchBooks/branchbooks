//! Reconcile (R1 to R3) on fixtures/reconcile/statement.bean: a statement that matches, one off
//! by a pending transaction, and one off by an amount nothing explains. The reconciled ledger is
//! fixtures/reconcile/reconciled.bean, which the oracle loads in Beancount.

mod common;

use std::cell::Cell;
use std::rc::Rc;

use branchbooks_core::book::Book;
use branchbooks_core::loader::MemorySource;
use branchbooks_core::write::store::MemoryStore;
use branchbooks_core::write::workspace::{
    ApplyError, EntryRef, Operation, OperationErrorCode, ReconcileInput, Workspace,
};
use common::fixtures;

type Ws = Workspace<MemoryStore>;

fn open() -> Ws {
    let text = std::fs::read_to_string(fixtures().join("reconcile/statement.bean")).unwrap();
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

/// The pending transactions up to the statement date, by payee.
fn pending(ws: &mut Ws, payees: &[&str]) -> Vec<EntryRef> {
    let core = &ws.core;
    payees
        .iter()
        .map(|p| {
            let (_, e) = core
                .entries()
                .find(|(_, e)| e.txn().is_some_and(|t| t.narration == *p))
                .unwrap();
            core.ref_of(e)
        })
        .collect()
}

fn reconcile(
    ws: &mut Ws,
    balance: &str,
    clear: &[&str],
) -> Result<String, (OperationErrorCode, String)> {
    let clear = pending(ws, clear);
    let op = Operation::Reconcile(ReconcileInput {
        account: "Assets:Checking".into(),
        date: "2026-01-31".into(),
        balance: balance.into(),
        currency: None,
        clear,
    });
    match ws.apply(&op, None, 0) {
        Ok(r) => Ok(ws.store.log[0].info.summary.clone() + " / " + &r.op_type),
        Err(ApplyError::Operation(e)) => Err((e.code, e.message)),
        Err(e) => panic!("{e:?}"),
    }
}

#[test]
fn finds_the_cleared_balance_and_the_pending_transactions() {
    let mut ws = open();
    let rec = ws
        .book()
        .reconcile("Assets:Checking", "2026-01-31".parse().unwrap(), "USD");
    assert_eq!(rec.cleared.to_string(), "800.00");
    let pending: Vec<(&str, String)> = rec
        .pending
        .iter()
        .map(|(e, a)| (e.txn().unwrap().narration.as_str(), a.to_string()))
        .collect();
    assert_eq!(
        pending,
        [
            ("Corner Shop", "-45.20".into()),
            ("Market", "-30.00".into())
        ]
    );
}

#[test]
fn a_matching_statement_clears_and_asserts_in_one_commit() {
    let mut ws = open();
    assert_eq!(
        reconcile(&mut ws, "724.80", &["Corner Shop", "Market"]),
        Ok("Reconcile Assets:Checking at 724.80 USD on 2026-01-31, clearing 2 transactions / reconcile".into())
    );
    assert_eq!(ws.store.log.len(), 1);
    let expected = std::fs::read_to_string(fixtures().join("reconcile/reconciled.bean")).unwrap();
    let text = ws.text("main.beancount").unwrap().to_owned();
    assert_eq!(text, expected);
    assert!(ws.book().errors().is_empty());
    // The Book after the commit equals a full load of what was written.
    let mut full = Book::load(
        "main.beancount",
        &MemorySource::new([("main.beancount", text.as_str())]),
    );
    assert_eq!(
        full.reconcile("Assets:Checking", "2026-01-31".parse().unwrap(), "USD")
            .cleared
            .to_string(),
        "724.80"
    );
    assert_eq!(ws.book().errors(), full.errors());
}

#[test]
fn a_statement_off_by_a_pending_transaction_is_refused_with_the_reason() {
    let mut ws = open();
    // The Market purchase is on the statement but not ticked.
    let (code, message) = reconcile(&mut ws, "724.80", &["Corner Shop"]).unwrap_err();
    assert_eq!(code, OperationErrorCode::Invalid);
    assert_eq!(
        message,
        "the statement is -30.00 USD from the cleared balance of 754.80 USD"
    );
    // Not on the statement, so the statement balance is 754.80: Beancount's assertion would still
    // count it.
    let (_, message) = reconcile(&mut ws, "754.80", &["Corner Shop"]).unwrap_err();
    assert!(
        message.starts_with("1 pending transaction(s) up to 2026-01-31 are not on the statement"),
        "{message}"
    );
    assert!(ws.store.log.is_empty());
}

#[test]
fn a_statement_off_by_an_unknown_amount_is_refused() {
    let mut ws = open();
    let (code, message) = reconcile(&mut ws, "720.00", &["Corner Shop", "Market"]).unwrap_err();
    assert_eq!(code, OperationErrorCode::Invalid);
    assert_eq!(
        message,
        "the statement is -4.80 USD from the cleared balance of 724.80 USD"
    );
    // Only pending transactions up to the statement date can be cleared.
    let (_, message) =
        reconcile(&mut ws, "716.30", &["Corner Shop", "Market", "Cafe"]).unwrap_err();
    assert_eq!(
        message,
        "only pending transactions in Assets:Checking up to 2026-01-31 can be cleared"
    );
    assert!(ws.store.log.is_empty());
}

#[test]
fn clearing_an_entry_written_with_the_txn_keyword_leaves_its_line_valid() {
    let text = "2026-01-01 open Assets:Checking USD\n2026-01-01 open Income:Pay\n\n2026-01-05 txn \"Keyword\"\n  Assets:Checking  10.00 USD\n  Income:Pay\n\n2026-01-06 ! \"Pending\"\n  Assets:Checking  5.00 USD\n  Income:Pay\n";
    let mut ws = Workspace::open(
        MemoryStore::new([("main.beancount", text)]),
        None,
        Box::new(|| "op".to_owned()),
    )
    .unwrap();
    let keyword = ws
        .book()
        .transactions()
        .into_iter()
        .find(|e| e.txn().is_some_and(|t| t.narration == "Keyword"))
        .cloned()
        .unwrap();
    let pending = ws
        .book()
        .transactions()
        .into_iter()
        .find(|e| e.txn().is_some_and(|t| t.narration == "Pending"))
        .cloned()
        .unwrap();
    let clear = vec![ws.core.ref_of(&keyword), ws.core.ref_of(&pending)];
    let input = ReconcileInput {
        account: "Assets:Checking".into(),
        date: "2026-01-31".into(),
        balance: "15.00".into(),
        currency: None,
        clear,
    };
    let _ = ws.apply(&Operation::Reconcile(input), None, 0);
    let after = ws.text("main.beancount").unwrap().to_owned();
    assert!(!after.contains("*xn"), "corrupted the line:\n{after}");
    assert_eq!(ws.book().errors(), vec![]);
}
