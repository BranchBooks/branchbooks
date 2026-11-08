//! Write-path behaviour sync depends on: idempotent
//! Op-Ids, retry after a conflict, and entry references that survive other changes moving their
//! lines.

mod common;

use branchbooks_core::write::render::{AssignInput, EntryInput, PostingInput, TransactionInput};
use branchbooks_core::write::store::{
    CommitInfo, CommitRecord, FileChange, MemoryStore, Snapshot, Store, StoreError,
};
use branchbooks_core::write::workspace::{
    ApplyError, EntryRef, Operation, OperationErrorCode, Workspace,
};
use common::*;

fn assign_input(date: &str, amount: &str) -> EntryInput {
    EntryInput::Assign(AssignInput {
        date: date.into(),
        category: "Expenses:Food:Dining".into(),
        amount: amount.into(),
        currency: None,
    })
}

fn txn(date: &str, narration: &str, category: &str, amount: &str) -> EntryInput {
    EntryInput::Txn(TransactionInput {
        date: date.into(),
        narration: narration.into(),
        postings: vec![
            PostingInput {
                account: category.into(),
                amount: Some(amount.into()),
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

fn grocery(amount: &str) -> EntryInput {
    txn(
        "2026-10-16",
        "Grocery store",
        "Expenses:Food:Groceries",
        amount,
    )
}

fn ids() -> Box<dyn FnMut() -> String> {
    let mut n = 0;
    Box::new(move || {
        n += 1;
        format!("id-{n}")
    })
}

fn open<S: Store>(store: S) -> Workspace<S> {
    Workspace::open(store, None, ids()).unwrap()
}

fn base_store() -> MemoryStore {
    MemoryStore::new([("main.beancount", BASE.as_str())])
}

fn ref_to<S: Store>(ws: &Workspace<S>, narration: &str) -> EntryRef {
    let e = ws
        .core
        .entries()
        .map(|(_, e)| e)
        .find(|x| x.txn().is_some_and(|t| t.narration == narration))
        .unwrap();
    ws.core.ref_of(e)
}

#[test]
fn applying_the_same_op_id_twice_commits_once() {
    let mut ws = open(base_store());
    let op = Operation::Add(assign_input("2026-11-03", "5.00"));
    let first = ws.apply(&op, Some("same"), 0).unwrap();
    let again = ws.apply(&op, Some("same"), 0).unwrap();
    assert!(!first.duplicate);
    assert!(again.duplicate);
    assert_eq!(
        (
            again.op_id.as_str(),
            again.op_type.as_str(),
            again.version.as_str()
        ),
        ("same", "assign", first.version.as_str())
    );
    assert_eq!(ws.store.log.len(), 1);
    eq(
        cat(&ws.book().budget(m("2026-11"), None), "Dining").assigned,
        5,
    );
}

/// The store commits, then the reply never arrives (network failure after the write).
struct LostReply {
    inner: MemoryStore,
    drop_next: bool,
}

impl Store for LostReply {
    fn snapshot(&self) -> Result<Snapshot, StoreError> {
        self.inner.snapshot()
    }
    fn find_op(&self, op_id: &str) -> Result<Option<CommitRecord>, StoreError> {
        self.inner.find_op(op_id)
    }
    fn commit(
        &mut self,
        base: &str,
        changes: &[FileChange],
        info: &CommitInfo,
    ) -> Result<String, StoreError> {
        let v = self.inner.commit(base, changes, info)?;
        if self.drop_next {
            self.drop_next = false;
            return Err(StoreError::Failed("connection reset".into()));
        }
        Ok(v)
    }
}

#[test]
fn a_commit_whose_response_was_lost_is_recognised_on_retry() {
    let mut ws = open(LostReply {
        inner: base_store(),
        drop_next: true,
    });
    let op = Operation::Add(assign_input("2026-11-03", "5.00"));
    let err = ws.apply(&op, Some("op-7"), 0).unwrap_err();
    assert_eq!(
        err,
        ApplyError::Store(StoreError::Failed("connection reset".into()))
    );
    ws.refresh().unwrap();
    let r = ws.apply(&op, Some("op-7"), 0).unwrap();
    assert!(r.duplicate);
    assert_eq!(
        ws.store
            .inner
            .log
            .iter()
            .map(|c| c.info.op_id.as_str())
            .collect::<Vec<_>>(),
        ["op-7"]
    );
}

#[test]
fn without_retries_a_conflict_is_returned_with_retries_the_operation_is_reapplied_on_the_latest_version()
 {
    let mut ws = open(base_store());
    let r = ref_to(&ws, "Grocery store");
    // Another device inserts lines above the entry, so its line number changes.
    let moved = BASE.replace(
        "; Example 1\n",
        "; Example 1\n; synced from the phone\n; (two lines)\n",
    );
    ws.store.external("main.beancount", &moved);
    let op = Operation::Edit(r.clone(), grocery("130.00"));
    assert!(matches!(
        ws.apply(&op, None, 0),
        Err(ApplyError::Store(StoreError::Conflict(_)))
    ));
    let result = ws.apply(&op, None, 2).unwrap();
    assert!(!result.duplicate);
    assert_eq!(result.entry.unwrap().line, r.line + 2);
    assert!(
        ws.text("main.beancount")
            .unwrap()
            .contains("; synced from the phone")
    );
    eq(
        cat(&ws.book().budget(m("2026-10"), None), "Groceries").available,
        -30,
    );
}

#[test]
fn a_retried_edit_of_an_entry_the_other_writer_changed_is_refused_not_applied_to_the_wrong_entry() {
    let mut ws = open(base_store());
    let r = ref_to(&ws, "Grocery store");
    let changed =
        BASE.replacen("120.00 USD", "125.00 USD", 1)
            .replacen("-120.00 USD", "-125.00 USD", 1);
    ws.store.external("main.beancount", &changed);
    match ws.apply(&Operation::Edit(r, grocery("130.00")), None, 1) {
        Err(ApplyError::Operation(e)) => assert_eq!(e.code, OperationErrorCode::Stale),
        other => panic!("expected stale, got {other:?}"),
    }
}

#[test]
fn a_ref_taken_before_an_insert_above_the_entry_still_resolves() {
    let mut ws = open(base_store());
    let r = ref_to(&ws, "Card payment");
    ws.apply(
        &Operation::Add(txn(
            "2026-10-16",
            "Inserted above",
            "Expenses:Food:Dining",
            "1.00",
        )),
        None,
        0,
    )
    .unwrap();
    let result = ws.apply(&Operation::Delete(r), None, 0).unwrap();
    assert_eq!(result.op_type, "delete-transaction");
    assert!(
        !ws.text("main.beancount")
            .unwrap()
            .contains("\"Card payment\"")
    );
}

#[test]
fn with_two_identical_entries_the_one_nearest_the_old_line_is_chosen() {
    let dup = "2026-10-20 custom \"envelope-assign\" Expenses:Food:Dining 1.00 USD\n";
    let text = format!("{}\n{dup}\n; spacer\n\n{dup}", *BASE);
    let mut ws = open(MemoryStore::new([("main.beancount", text.as_str())]));
    let second = ws
        .core
        .entries()
        .map(|(_, e)| e)
        .filter(|e| e.custom().is_some() && e.date == d("2026-10-20"))
        .nth(1)
        .unwrap();
    // Slightly off, as after a shift.
    let r = EntryRef {
        line: second.line + 1,
        ..ws.core.ref_of(second)
    };
    ws.apply(&Operation::Delete(r), None, 0).unwrap();
    let after = ws.text("main.beancount").unwrap();
    let lines: Vec<&str> = after.split('\n').collect();
    assert_eq!(lines.iter().filter(|l| **l == dup.trim()).count(), 1);
    let at = |s: &str| lines.iter().position(|l| *l == s).unwrap();
    assert!(at(dup.trim()) < at("; spacer"));
}
