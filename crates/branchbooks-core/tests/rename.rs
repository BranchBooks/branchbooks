//! Renaming an account on fixtures/examples-1-5.bean: every place it is written changes in one
//! commit, the numbers stay the same, and a rename that would collide or change the account's
//! type is refused.

mod common;

use std::cell::Cell;
use std::rc::Rc;

use branchbooks_core::write::store::MemoryStore;
use branchbooks_core::write::workspace::{ApplyError, Operation, OperationErrorCode, Workspace};
use common::{d, fixtures, m};

fn open() -> Workspace<MemoryStore> {
    let text = std::fs::read_to_string(fixtures().join("examples-1-5.bean")).unwrap();
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

fn rename(from: &str, to: &str) -> Operation {
    Operation::RenameAccount {
        from: from.into(),
        to: to.into(),
    }
}

fn refused(ws: &mut Workspace<MemoryStore>, from: &str, to: &str) -> String {
    match ws.apply(&rename(from, to), None, 0) {
        Err(ApplyError::Operation(e)) => {
            assert_eq!(e.code, OperationErrorCode::Invalid);
            e.message
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_rename_changes_every_mention_in_one_commit_and_keeps_the_numbers() {
    let mut ws = open();
    let before = ws.book().budget(m("2026-11"), Some(d("2026-11-02")));
    // Investing is named in its open, an assignment and a transfer's `category:`.
    let r = ws
        .apply(
            &rename("Expenses:Savings:Investing", "Expenses:Savings:Brokerage"),
            None,
            0,
        )
        .unwrap();
    assert_eq!(r.op_type, "rename-account");
    assert_eq!(ws.store.log.len(), 1);
    let text = ws.core.text("main.beancount").unwrap().to_owned();
    assert!(!text.contains("Investing"), "{text}");
    assert_eq!(text.matches("Expenses:Savings:Brokerage").count(), 3);
    let after = ws.book().budget(m("2026-11"), Some(d("2026-11-02")));
    assert_eq!(after.unassigned, before.unassigned);
    let row = after.category("Expenses:Savings:Brokerage").unwrap();
    assert_eq!(
        row.available,
        before
            .category("Expenses:Savings:Investing")
            .unwrap()
            .available
    );
    assert!(ws.book().errors().is_empty());
}

#[test]
fn refuses_a_taken_name_another_type_or_a_bad_name() {
    let mut ws = open();
    assert!(
        refused(&mut ws, "Expenses:Food:Dining", "Expenses:Food:Groceries")
            .contains("already exists")
    );
    assert!(
        refused(&mut ws, "Assets:Checking", "Expenses:Checking").contains("must stay under Assets")
    );
    assert!(refused(&mut ws, "Assets:Checking", "Assets:checking").contains("not an account name"));
    assert!(ws.store.log.is_empty());
}
