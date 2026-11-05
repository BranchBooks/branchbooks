//! Placeholder accounts (`budget: "placeholder"`): they group their sub-accounts, hold no transactions
//! and are no envelope. What reaches one by a hand edit still counts, and is flagged.

mod common;

use branchbooks_core::write::render::{
    AssignInput, BudgetFlag, EntryInput, OpenInput, PostingInput, TransactionInput,
};
use branchbooks_core::write::store::MemoryStore;
use branchbooks_core::write::workspace::{ApplyError, Operation, OperationErrorCode, Workspace};
use common::*;

const OPENS: &str = "2026-10-01 open Expenses:Group\n  budget: \"placeholder\"\n\
    2026-10-01 open Expenses:Group:Inside\n\
    2026-10-01 open Assets:Holdings\n  budget: \"placeholder\"\n";

fn workspace(extra: &str) -> Workspace<MemoryStore> {
    let text = format!("{}{OPENS}{extra}", *BASE);
    let mut n = 0;
    Workspace::open(
        MemoryStore::new([("main.beancount", text.as_str())]),
        None,
        Box::new(move || {
            n += 1;
            format!("op-{n}")
        }),
    )
    .unwrap()
}

fn spend(account: &str) -> EntryInput {
    EntryInput::Txn(TransactionInput {
        date: "2026-11-05".into(),
        narration: "Spend".into(),
        postings: vec![
            PostingInput {
                account: account.into(),
                amount: Some("10.00".into()),
                ..Default::default()
            },
            PostingInput {
                account: "Assets:Checking".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    })
}

fn refused(r: Result<impl std::fmt::Debug, ApplyError>, says: &str) {
    match r {
        Err(ApplyError::Operation(e)) => {
            assert_eq!(e.code, OperationErrorCode::Invalid, "{e}");
            assert!(e.message.contains(says), "{}", e.message);
        }
        other => panic!("not refused: {other:?}"),
    }
}

#[test]
fn a_placeholder_has_no_envelope_and_changes_no_number() {
    let v = book_of(OPENS).budget(m("2026-11"), None);
    assert!(v.category("Expenses:Group").is_none());
    assert!(v.category("Expenses:Group:Inside").is_some());
    eq(v.unassigned, 1080);
    assert_eq!(book_of(OPENS).budget_diagnostics(), vec![]);
    assert_eq!(book_of(OPENS).errors(), vec![]);
}

#[test]
fn a_posting_written_by_hand_still_counts_and_is_flagged() {
    let extra = format!(
        "{OPENS}2026-11-05 * \"By hand\"\n  Expenses:Group  10.00 USD\n  Assets:Checking\n\
         2026-11-06 * \"Off budget\"\n  Assets:Holdings  5.00 USD\n  Equity:Opening\n\
         2026-10-01 open Equity:Opening\n"
    );
    let mut book = book_of(&extra);
    assert_eq!(book.errors(), vec![], "valid Beancount, so no ledger error");
    let v = book.budget(m("2026-11"), None);
    // The row appears because money is there: it never goes missing.
    eq(v.category("Expenses:Group").unwrap().available, -10);
    eq(v.unassigned, 1080);
    let flagged: Vec<&str> = v
        .flags
        .iter()
        .filter(|f| f.code == "placeholder")
        .map(|f| f.message.as_str())
        .collect();
    assert_eq!(flagged.len(), 2, "{:?}", v.flags);
    assert!(flagged[0].contains("Expenses:Group is a placeholder"));
    assert!(flagged[1].contains("Assets:Holdings is a placeholder"));
}

#[test]
fn an_assignment_written_by_hand_changes_nothing_and_is_flagged() {
    let extra = format!("{OPENS}2026-11-05 custom \"envelope-assign\" Expenses:Group 50.00 USD\n");
    let mut book = book_of(&extra);
    eq(book.budget(m("2026-11"), None).unassigned, 1080);
    assert!(
        book.budget_diagnostics()
            .iter()
            .any(|d| d.message == "Expenses:Group is a placeholder: use one of its sub-accounts")
    );
}

#[test]
fn the_write_path_refuses_new_postings_and_assignments() {
    let mut ws = workspace("");
    refused(
        ws.apply(&Operation::Add(spend("Expenses:Group")), None, 0),
        "is a placeholder",
    );
    let assign = EntryInput::Assign(AssignInput {
        date: "2026-11-05".into(),
        category: "Expenses:Group".into(),
        amount: "5.00".into(),
        currency: None,
    });
    refused(
        ws.apply(&Operation::Add(assign), None, 0),
        "use one of its sub-accounts",
    );
    ws.apply(&Operation::Add(spend("Expenses:Group:Inside")), None, 0)
        .unwrap();
}

#[test]
fn an_old_posting_can_be_kept_while_editing_and_fixed() {
    let mut ws =
        workspace("2026-11-04 * \"Old\"\n  Expenses:Group  10.00 USD\n  Assets:Checking\n");
    let find = |ws: &Workspace<MemoryStore>| {
        let (_, e) = ws
            .core
            .entries()
            .find(|(_, e)| e.txn().is_some_and(|t| t.narration == "Old"))
            .unwrap();
        ws.core.ref_of(e)
    };
    let at = find(&ws);
    let mut same = spend("Expenses:Group");
    if let EntryInput::Txn(t) = &mut same {
        t.date = "2026-11-04".into();
        t.narration = "Old".into();
    }
    ws.apply(&Operation::Edit(at, same.clone()), None, 0)
        .unwrap();
    // And it moves to a sub-account in an edit like any other.
    let at = find(&ws);
    let mut moved = same;
    if let EntryInput::Txn(t) = &mut moved {
        t.postings[0].account = "Expenses:Group:Inside".into();
    }
    ws.apply(&Operation::Edit(at, moved), None, 0).unwrap();
}

#[test]
fn an_account_with_transactions_cannot_become_a_placeholder() {
    let mut ws = workspace("");
    let open = |account: &str| {
        EntryInput::Open(OpenInput {
            date: "2026-10-01".into(),
            account: account.into(),
            budget: Some(BudgetFlag::Placeholder),
            ..Default::default()
        })
    };
    refused(
        ws.apply(&Operation::Add(open("Assets:Checking")), None, 0),
        "has transactions",
    );
    ws.apply(&Operation::Add(open("Expenses:Unused:Group")), None, 0)
        .unwrap();
}
