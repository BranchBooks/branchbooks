//! The write path: operations -> minimal text
//! edits -> validation -> one commit per operation.

mod common;

use std::cell::Cell;
use std::rc::Rc;

use branchbooks_core::book::Book;
use branchbooks_core::budget::MonthView;
use branchbooks_core::loader::{MemorySource, load};
use branchbooks_core::money::fixed;
use branchbooks_core::types::{Body, Entry, Meta};
use branchbooks_core::write::render::{
    AssignInput, BudgetFlag, CloseInput, EntryInput, MetaInputValue, MoveInput, OpenInput,
    PostingInput, TransactionInput, render_entry,
};
use branchbooks_core::write::store::{MemoryStore, Store, StoreError};
use branchbooks_core::write::workspace::{
    ApplyError, ApplyResult, Operation, OperationErrorCode, Workspace,
};
use common::*;

type Ws = Workspace<MemoryStore>;

fn multi() -> Vec<(&'static str, String)> {
    ["main.beancount", "2025.beancount", "2026.beancount"]
        .map(|f| {
            (
                f,
                std::fs::read_to_string(fixtures().join("multifile").join(f)).unwrap(),
            )
        })
        .into()
}

fn multi_text(name: &str) -> String {
    multi().into_iter().find(|(f, _)| *f == name).unwrap().1
}

fn ids() -> Box<dyn FnMut() -> String> {
    let n = Rc::new(Cell::new(0));
    Box::new(move || {
        n.set(n.get() + 1);
        format!("op-{}", n.get())
    })
}

fn open_files(files: &[(&str, &str)]) -> Ws {
    Workspace::open(MemoryStore::new(files.iter().copied()), None, ids()).unwrap()
}

fn open_multi() -> Ws {
    let files = multi();
    open_files(
        &files
            .iter()
            .map(|(f, t)| (*f, t.as_str()))
            .collect::<Vec<_>>(),
    )
}

fn base() -> Ws {
    open_files(&[("main.beancount", &BASE)])
}

fn posting(account: &str, amount: Option<&str>) -> PostingInput {
    PostingInput {
        account: account.into(),
        amount: amount.map(Into::into),
        ..Default::default()
    }
}

fn spend_named(
    date: &str,
    category: &str,
    amount: &str,
    from: &str,
    narration: &str,
) -> EntryInput {
    EntryInput::Txn(TransactionInput {
        date: date.into(),
        narration: narration.into(),
        postings: vec![posting(category, Some(amount)), posting(from, None)],
        ..Default::default()
    })
}

fn spend(date: &str, category: &str, amount: &str, from: &str) -> EntryInput {
    spend_named(date, category, amount, from, "Purchase")
}

fn assign_input(date: &str, category: &str, amount: &str) -> EntryInput {
    EntryInput::Assign(AssignInput {
        date: date.into(),
        category: category.into(),
        amount: amount.into(),
        currency: None,
    })
}

fn move_input(date: &str, from: &str, to: &str, amount: &str) -> EntryInput {
    EntryInput::Move(MoveInput {
        date: date.into(),
        from: from.into(),
        to: to.into(),
        amount: amount.into(),
        currency: None,
    })
}

fn add(ws: &mut Ws, entry: EntryInput) -> ApplyResult {
    ws.apply(&Operation::Add(entry), None, 0).unwrap()
}

fn code(r: Result<ApplyResult, ApplyError>) -> OperationErrorCode {
    match r {
        Err(ApplyError::Operation(e)) => e.code,
        other => panic!("expected an operation error, got {other:?}"),
    }
}

fn meta_input(m: &Meta) -> Vec<(String, MetaInputValue)> {
    m.iter()
        .map(|(k, v)| (k.to_owned(), MetaInputValue::Value(v.clone())))
        .collect()
}

fn find(ws: &Ws, pred: impl Fn(&Entry) -> bool) -> Entry {
    ws.core
        .entries()
        .map(|(_, e)| e)
        .find(|e| pred(e))
        .expect("an entry")
        .clone()
}

fn is_custom(e: &Entry, name: &str) -> bool {
    e.custom().is_some_and(|c| c.name == name)
}

/// Budget numbers only: the parts that must not depend on where in which file an entry sits.
fn numbers(v: &MonthView) -> MonthView {
    MonthView {
        warnings: vec![],
        flags: vec![],
        ..v.clone()
    }
}

/// `before` must survive unchanged inside `after`; the only extra lines are `allowed` or blank.
#[track_caller]
fn only_added(before: &str, after: &str, allowed: &[String]) {
    let a: Vec<&str> = before.split('\n').collect();
    let mut i = 0;
    let mut extra = Vec::new();
    for line in after.split('\n') {
        if i < a.len() && line == a[i] {
            i += 1;
        } else {
            extra.push(line);
        }
    }
    assert_eq!(i, a.len(), "every original line is kept, in order");
    let extra: Vec<&str> = extra.into_iter().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(extra, allowed);
}

#[test]
fn operations_reproduce_the_scenario_fixtures() {
    let dining = "Expenses:Food:Dining";
    let card = "Liabilities:CreditCard";
    let cases: Vec<(&str, Vec<EntryInput>)> = vec![
        (
            "scenarios/ex06-credit-overspend.bean",
            vec![spend("2026-11-05", dining, "200.00", card)],
        ),
        (
            "scenarios/ex07-card-refund.bean",
            vec![
                spend("2026-11-08", dining, "60.00", card),
                spend_named("2026-11-12", dining, "-25.00", card, "Refund"),
            ],
        ),
        (
            "scenarios/ex08-future-month.bean",
            vec![assign_input(
                "2026-12-01",
                "Expenses:Housing:Rent",
                "300.00",
            )],
        ),
        (
            "scenarios/ex09-mixed-overspend.bean",
            vec![
                spend("2026-11-05", dining, "200.00", card),
                spend("2026-11-06", dining, "30.00", "Assets:Checking"),
            ],
        ),
        (
            "scenarios/ex10-partial-reimbursement.bean",
            vec![EntryInput::Txn(TransactionInput {
                date: "2026-11-04".into(),
                payee: Some("Roommate".into()),
                narration: "Groceries share plus extra".into(),
                postings: vec![
                    posting("Assets:Checking", Some("200.00")),
                    posting("Expenses:Food:Groceries", Some("-150.00")),
                    posting("Income:Other", Some("-50.00")),
                ],
                ..Default::default()
            })],
        ),
        (
            "scenarios/ex11-move.bean",
            vec![move_input(
                "2026-11-10",
                "Expenses:Housing:Rent",
                "Expenses:Food:Groceries",
                "20.00",
            )],
        ),
        (
            "scenarios/ex12-same-day-assign-first.bean",
            vec![
                spend("2026-11-05", dining, "200.00", card),
                assign_input("2026-11-06", dining, "40.00"),
                spend("2026-11-06", dining, "30.00", "Assets:Checking"),
            ],
        ),
    ];
    for (f, ops) in cases {
        let mut ws = base();
        for entry in ops {
            add(&mut ws, entry);
        }
        assert_eq!(ws.book().errors(), vec![], "{f}");
        let mut expected = fixture(f);
        for m in ["2026-10", "2026-11", "2026-12"] {
            assert_eq!(
                numbers(&ws.book().budget(m.parse().unwrap(), None)),
                numbers(&expected.budget(m.parse().unwrap(), None)),
                "{f} {m}"
            );
        }
    }
}

#[test]
fn appending_keeps_every_existing_byte_and_adds_one_blank_line_and_the_entry() {
    let mut ws = base();
    let entry = spend_named(
        "2026-11-05",
        "Expenses:Food:Dining",
        "200.00",
        "Liabilities:CreditCard",
        "Dinner party",
    );
    let r = add(&mut ws, entry.clone());
    let block = render_entry(&entry, "USD").unwrap();
    assert_eq!(
        ws.text("main.beancount").unwrap(),
        format!("{}\n{}\n", *BASE, block.join("\n"))
    );
    let at = r.entry.unwrap();
    assert_eq!(
        (at.file.as_str(), at.line),
        ("main.beancount", BASE.split('\n').count() as u32 + 1)
    );
    assert_eq!(r.changed, ["main.beancount"]);
}

#[test]
fn an_insert_in_the_middle_lands_in_date_order_and_changes_nothing_else() {
    let mut ws = base();
    let entry = spend_named(
        "2026-10-20",
        "Expenses:Food:Groceries",
        "12.34",
        "Assets:Checking",
        "Corner shop",
    );
    let r = add(&mut ws, entry.clone());
    let after = ws.text("main.beancount").unwrap().to_owned();
    let block = render_entry(&entry, "USD").unwrap();
    only_added(&BASE, &after, &block);
    let lines: Vec<&str> = after.split('\n').collect();
    let line = r.entry.unwrap().line as usize;
    assert_eq!(
        lines[line - 1..line + 2],
        block.iter().map(String::as_str).collect::<Vec<_>>()[..]
    );
    let at = after.find("Corner shop").unwrap();
    assert!(at > after.find("\"Restaurant\"").unwrap());
    assert!(at < after.find("\"Card payment\"").unwrap());
    eq(
        cat(&ws.book().budget(m("2026-10"), None), "Groceries").available,
        "-32.34",
    );
}

#[test]
fn crlf_files_stay_crlf() {
    let crlf = BASE.replace('\n', "\r\n");
    let mut ws = open_files(&[("main.beancount", &crlf)]);
    add(
        &mut ws,
        assign_input("2026-11-03", "Expenses:Food:Dining", "5.00"),
    );
    let text = ws.text("main.beancount").unwrap();
    assert!(text.starts_with(&crlf[..crlf.len() - 2]));
    assert!(
        !text
            .as_bytes()
            .windows(2)
            .any(|w| w[1] == b'\n' && w[0] != b'\r')
    );
}

#[test]
fn a_transaction_goes_to_its_year_file_and_the_other_files_are_untouched() {
    let mut ws = open_multi();
    let r = add(
        &mut ws,
        spend(
            "2026-01-10",
            "Expenses:Food:Groceries",
            "20.00",
            "Liabilities:Visa",
        ),
    );
    assert_eq!(r.changed, ["2026.beancount"]);
    assert_eq!(
        ws.text("main.beancount").unwrap(),
        multi_text("main.beancount")
    );
    assert_eq!(
        ws.text("2025.beancount").unwrap(),
        multi_text("2025.beancount")
    );
    assert_eq!(ws.book().errors(), vec![]);
}

#[test]
fn a_new_year_creates_its_file_and_the_include_in_year_order() {
    let mut ws = open_multi();
    let entry = spend(
        "2027-02-01",
        "Expenses:Food:Groceries",
        "20.00",
        "Assets:Checking",
    );
    let r = add(&mut ws, entry.clone());
    let mut changed = r.changed.clone();
    changed.sort();
    assert_eq!(changed, ["2027.beancount", "main.beancount"]);
    let main = ws.text("main.beancount").unwrap().to_owned();
    only_added(
        &multi_text("main.beancount"),
        &main,
        &["include \"2027.beancount\"".to_owned()],
    );
    assert!(main.contains("include \"2026.beancount\"\ninclude \"2027.beancount\"\n"));
    assert_eq!(
        ws.text("2027.beancount").unwrap(),
        format!("{}\n", render_entry(&entry, "USD").unwrap().join("\n"))
    );
    assert_eq!(ws.book().errors(), vec![]);
}

#[test]
fn opens_go_to_the_root_file_in_date_order() {
    let mut ws = open_multi();
    let entry = EntryInput::Open(OpenInput {
        date: "2026-02-01".into(),
        account: "Expenses:Fun".into(),
        currencies: vec!["USD".into()],
        ..Default::default()
    });
    let r = add(&mut ws, entry);
    assert_eq!(r.changed, ["main.beancount"]);
    let text = ws.text("main.beancount").unwrap().to_owned();
    only_added(
        &multi_text("main.beancount"),
        &text,
        &["2026-02-01 open Expenses:Fun USD".to_owned()],
    );
    assert!(text.find("open Expenses:Fun").unwrap() < text.find("2026-12-31 close").unwrap());
    assert!(
        ws.book()
            .budget(m("2026-02"), None)
            .categories
            .iter()
            .any(|c| c.category == "Expenses:Fun")
    );
}

#[test]
fn editing_an_open_keeps_its_booking_method_and_other_metadata_and_can_change_the_budget_flag() {
    let text = format!(
        "{}\n2026-03-01 open Assets:Savings USD \"FIFO\"\n  budget: \"off\"\n  note: \"joint\"\n  since: 2020-01-01\n",
        *BASE
    );
    let mut ws = open_files(&[("main.beancount", &text)]);
    let e = find(&ws, |x| {
        x.open().is_some_and(|o| o.account == "Assets:Savings")
    });
    let o = e.open().unwrap();
    let entry = EntryInput::Open(OpenInput {
        date: "2026-02-15".into(),
        account: o.account.clone(),
        currencies: o.currencies.clone(),
        booking: o.booking.clone(),
        budget: Some(BudgetFlag::On),
        meta: meta_input(&e.meta),
    });
    let r = ws.core.ref_of(&e);
    ws.apply(&Operation::Edit(r, entry), None, 0).unwrap();
    assert!(ws.text("main.beancount").unwrap().contains(
        "2026-02-15 open Assets:Savings USD \"FIFO\"\n  budget: \"on\"\n  note: \"joint\"\n  since: 2020-01-01"
    ));
    assert_eq!(
        ws.book().account_info()["Assets:Savings"].open.date,
        d("2026-02-15")
    );
}

#[test]
fn closes_an_account_moves_the_close_date_and_reopens_it_by_deleting_the_close() {
    let text = format!("{}\n2026-03-01 open Assets:Savings USD\n", *BASE);
    let mut ws = open_files(&[("main.beancount", &text)]);
    let close = |date: &str| {
        EntryInput::Close(CloseInput {
            date: date.into(),
            account: "Assets:Savings".into(),
        })
    };
    add(&mut ws, close("2026-12-31"));
    let find_close = |ws: &Ws| {
        find(ws, |x| {
            x.close().is_some_and(|c| c.account == "Assets:Savings")
        })
    };
    let r = ws.core.ref_of(&find_close(&ws));
    ws.apply(&Operation::Edit(r, close("2026-11-30")), None, 0)
        .unwrap();
    assert_eq!(
        ws.book().account_info()["Assets:Savings"]
            .close
            .as_ref()
            .unwrap()
            .date,
        d("2026-11-30")
    );
    let r = ws.core.ref_of(&find_close(&ws));
    ws.apply(&Operation::Delete(r), None, 0).unwrap();
    assert!(ws.book().account_info()["Assets:Savings"].close.is_none());
}

#[test]
fn deleting_an_open_that_postings_still_use_is_refused() {
    let mut ws = base();
    let e = find(&ws, |x| {
        x.open().is_some_and(|o| o.account == "Assets:Checking")
    });
    let r = ws.core.ref_of(&e);
    assert!(matches!(
        ws.apply(&Operation::Delete(r), None, 0),
        Err(ApplyError::Operation(_))
    ));
}

#[test]
fn edits_and_deletes_assignments_and_moves() {
    let mut ws = base();
    let dining = "Expenses:Food:Dining";
    add(&mut ws, assign_input("2026-11-03", dining, "25.00"));
    let assign_on = |ws: &Ws, date: &str| {
        find(ws, |x| {
            is_custom(x, "envelope-assign") && x.date.to_string() == date
        })
    };
    let r = ws.core.ref_of(&assign_on(&ws, "2026-11-03"));
    ws.apply(
        &Operation::Edit(r, assign_input("2026-11-04", dining, "30.00")),
        None,
        0,
    )
    .unwrap();
    assert!(
        ws.text("main.beancount")
            .unwrap()
            .contains("2026-11-04 custom \"envelope-assign\" Expenses:Food:Dining 30.00 USD")
    );
    add(
        &mut ws,
        move_input("2026-11-05", dining, "Expenses:Food:Groceries", "5.00"),
    );
    let r = ws
        .core
        .ref_of(&find(&ws, |x| is_custom(x, "envelope-move")));
    ws.apply(&Operation::Delete(r), None, 0).unwrap();
    let r = ws.core.ref_of(&assign_on(&ws, "2026-11-04"));
    ws.apply(&Operation::Delete(r), None, 0).unwrap();
    let text = ws.text("main.beancount").unwrap();
    assert!(!text.contains("2026-11-04") && !text.contains("2026-11-05"));
}

fn find_txn(ws: &Ws, narration: &str) -> branchbooks_core::write::workspace::EntryRef {
    let e = find(ws, |x| x.txn().is_some_and(|t| t.narration == narration));
    ws.core.ref_of(&e)
}

#[test]
fn editing_without_a_date_change_replaces_the_lines_in_place() {
    let mut ws = base();
    let r = find_txn(&ws, "Grocery store");
    let entry = spend_named(
        "2026-10-16",
        "Expenses:Food:Groceries",
        "130.00",
        "Assets:Checking",
        "Grocery store",
    );
    let result = ws
        .apply(&Operation::Edit(r.clone(), entry.clone()), None, 0)
        .unwrap();
    assert_eq!(result.op_type, "edit-transaction");
    let before: Vec<&str> = BASE.split('\n').collect();
    let text = ws.text("main.beancount").unwrap().to_owned();
    let after: Vec<&str> = text.split('\n').collect();
    let block = render_entry(&entry, "USD").unwrap();
    let i = r.line as usize - 1;
    assert_eq!(
        after[i..i + block.len()],
        block.iter().map(String::as_str).collect::<Vec<_>>()[..]
    );
    let rest_after: Vec<&str> = after[..i]
        .iter()
        .chain(&after[i + block.len()..])
        .copied()
        .collect();
    let rest_before: Vec<&str> = before[..i]
        .iter()
        .chain(&before[i + 3..])
        .copied()
        .collect();
    assert_eq!(rest_after, rest_before);
    eq(
        cat(&ws.book().budget(m("2026-10"), None), "Groceries").available,
        -30,
    );
}

#[test]
fn editing_the_date_moves_the_entry_and_closes_the_old_gap() {
    let mut ws = base();
    let r = find_txn(&ws, "Card payment");
    let entry = EntryInput::Txn(TransactionInput {
        date: "2026-11-01".into(),
        narration: "Card payment".into(),
        postings: vec![
            posting("Liabilities:CreditCard", Some("45.00")),
            posting("Assets:Checking", Some("-45.00")),
        ],
        ..Default::default()
    });
    ws.apply(&Operation::Edit(r, entry), None, 0).unwrap();
    let text = ws.text("main.beancount").unwrap().to_owned();
    assert!(!text.contains("\n\n\n"));
    let at = text.find("\"Card payment\"").unwrap();
    assert!(at > text.find("\"Bulk grocery run\"").unwrap());
    assert!(at < text.find("; Example 5").unwrap());
    eq(card(&ws.book().budget(m("2026-10"), None)).owed, 45);
    eq(card(&ws.book().budget(m("2026-11"), None)).owed, 0);
    assert_eq!(ws.book().errors(), vec![]);
}

#[test]
fn deleting_removes_the_entry_and_one_blank_line_and_keeps_comments() {
    let mut ws = base();
    let r = find_txn(&ws, "Bulk grocery run");
    let result = ws.apply(&Operation::Delete(r), None, 0).unwrap();
    assert_eq!(result.op_type, "delete-transaction");
    let text = ws.text("main.beancount").unwrap();
    assert!(text.contains("; Example 4\n\n; Example 5"));
    assert!(!text.contains("Bulk grocery run"));
    eq(
        cat(&ws.book().budget(m("2026-10"), None), "Groceries").available,
        480,
    );
}

#[test]
fn an_edit_or_delete_based_on_an_old_read_is_refused_as_stale() {
    let mut ws = base();
    let r = find_txn(&ws, "Grocery store");
    let entry = spend_named(
        "2026-10-16",
        "Expenses:Food:Groceries",
        "130.00",
        "Assets:Checking",
        "Grocery store",
    );
    ws.apply(&Operation::Edit(r.clone(), entry), None, 0)
        .unwrap();
    assert_eq!(
        code(ws.apply(&Operation::Delete(r), None, 0)),
        OperationErrorCode::Stale
    );
}

#[test]
fn refuses_an_unbalanced_transaction_or_an_unknown_account_and_commits_nothing() {
    let mut ws = base();
    let unbalanced = EntryInput::Txn(TransactionInput {
        date: "2026-11-03".into(),
        narration: "x".into(),
        postings: vec![
            posting("Expenses:Food:Dining", Some("10.00")),
            posting("Assets:Checking", Some("-9.00")),
        ],
        ..Default::default()
    });
    assert_eq!(
        code(ws.apply(&Operation::Add(unbalanced), None, 0)),
        OperationErrorCode::Rejected
    );
    let unknown = spend("2026-11-03", "Expenses:Nowhere", "1.00", "Assets:Checking");
    assert_eq!(
        code(ws.apply(&Operation::Add(unknown), None, 0)),
        OperationErrorCode::Rejected
    );
    let bad_date = spend(
        "2026-02-30",
        "Expenses:Food:Dining",
        "1.00",
        "Assets:Checking",
    );
    assert_eq!(
        code(ws.apply(&Operation::Add(bad_date), None, 0)),
        OperationErrorCode::Invalid
    );
    assert!(ws.store.log.is_empty());
    assert_eq!(ws.store.snapshot().unwrap().files["main.beancount"], *BASE);
}

#[test]
fn refuses_an_assignment_to_something_that_is_not_a_budgeted_category() {
    let mut ws = base();
    let op = Operation::Add(assign_input("2026-11-03", "Income:Salary", "5.00"));
    assert_eq!(code(ws.apply(&op, None, 0)), OperationErrorCode::Invalid);
}

#[test]
fn an_assignment_to_a_card_sets_money_aside_carries_over_and_can_be_taken_back_to_zero() {
    let mut ws = base();
    let card_name = "Liabilities:CreditCard";
    let card_available = |ws: &mut Ws, m: &str, today: &str| {
        ws.book()
            .budget(m.parse().unwrap(), Some(today.parse().unwrap()))
            .card(card_name)
            .unwrap()
            .available
    };
    let before = ws.book().budget(m("2026-11"), Some(d("2026-11-10")));
    let set = card_available(&mut ws, "2026-11", "2026-11-10");
    add(&mut ws, assign_input("2026-11-10", card_name, "40.00"));
    let after = ws.book().budget(m("2026-11"), Some(d("2026-11-10")));
    assert_eq!(
        fixed(card_available(&mut ws, "2026-11", "2026-11-10"), 2),
        fixed(set + dec("40"), 2)
    );
    assert_eq!(
        fixed(after.unassigned, 2),
        fixed(before.unassigned - dec("40"), 2)
    );
    assert!(after.categories.iter().all(|c| c.category != card_name));
    // Next month it is still set aside.
    assert!(card_available(&mut ws, "2026-12", "2026-12-01") >= dec("40"));
    // Taking back more than is set aside is refused; taking back exactly that is fine.
    let all = card_available(&mut ws, "2026-11", "2026-11-10");
    let too_much = fixed(-(all + dec("1")), 2);
    assert!(matches!(
        ws.apply(
            &Operation::Add(assign_input("2026-11-11", card_name, &too_much)),
            None,
            0
        ),
        Err(ApplyError::Operation(_))
    ));
    add(
        &mut ws,
        assign_input("2026-11-11", card_name, &fixed(-all, 2)),
    );
    assert_eq!(
        fixed(card_available(&mut ws, "2026-11", "2026-11-11"), 2),
        "0.00"
    );
    // An off-budget or cash account is still not something to assign to.
    let op = Operation::Add(assign_input("2026-11-03", "Assets:Checking", "5.00"));
    assert_eq!(code(ws.apply(&op, None, 0)), OperationErrorCode::Invalid);
}

#[test]
fn refuses_a_move_that_would_overspend_the_source_and_allows_exactly_the_available() {
    let mut ws = base();
    let op = Operation::Add(move_input(
        "2026-11-10",
        "Expenses:Housing:Rent",
        "Expenses:Food:Dining",
        "2000.00",
    ));
    assert_eq!(code(ws.apply(&op, None, 0)), OperationErrorCode::Blocked);
    add(
        &mut ws,
        move_input(
            "2026-11-10",
            "Expenses:Housing:Rent",
            "Expenses:Food:Dining",
            "1500.00",
        ),
    );
    eq(
        cat(&ws.book().budget(m("2026-11"), None), "Rent").available,
        0,
    );
}

#[test]
fn a_transaction_that_makes_an_existing_move_blocked_is_committed_with_a_warning() {
    let mut ws = base();
    add(
        &mut ws,
        move_input(
            "2026-11-10",
            "Expenses:Food:Dining",
            "Expenses:Housing:Rent",
            "155.00",
        ),
    );
    let r = add(
        &mut ws,
        spend(
            "2026-11-05",
            "Expenses:Food:Dining",
            "50.00",
            "Assets:Checking",
        ),
    );
    assert_eq!(
        r.warnings
            .iter()
            .map(|w| w.code.as_str())
            .collect::<Vec<_>>(),
        ["blocked"]
    );
    assert_eq!(ws.book().blocked(None).len(), 1);
}

#[test]
fn one_commit_per_operation_with_op_id_and_op_type() {
    let mut ws = base();
    let a = ws
        .apply(
            &Operation::Add(assign_input("2026-11-03", "Expenses:Food:Dining", "5.00")),
            Some("fixed-id"),
            0,
        )
        .unwrap();
    let b = add(
        &mut ws,
        spend(
            "2026-11-04",
            "Expenses:Food:Dining",
            "5.00",
            "Assets:Checking",
        ),
    );
    let log: Vec<(&str, &str, Vec<String>)> = ws
        .store
        .log
        .iter()
        .map(|c| {
            (
                c.info.op_id.as_str(),
                c.info.op_type.as_str(),
                c.paths.clone(),
            )
        })
        .collect();
    assert_eq!(
        log,
        [
            ("fixed-id", "assign", vec!["main.beancount".to_owned()]),
            (
                b.op_id.as_str(),
                "add-transaction",
                vec!["main.beancount".to_owned()]
            ),
        ]
    );
    assert_eq!(a.op_id, "fixed-id");
    assert_eq!(b.op_id, "op-1");
    assert_eq!(
        ws.store.log[0].info.summary,
        "Assign 5.00 USD to Expenses:Food:Dining on 2026-11-03"
    );
}

#[test]
fn a_stale_base_version_is_a_conflict_refresh_and_retry_succeeds() {
    let mut ws = base();
    ws.store.external(
        "main.beancount",
        &format!(
            "{}\n2026-11-03 custom \"envelope-assign\" Expenses:Food:Dining 1.00 USD\n",
            *BASE
        ),
    );
    let op = Operation::Add(assign_input("2026-11-04", "Expenses:Food:Dining", "2.00"));
    assert!(matches!(
        ws.apply(&op, None, 0),
        Err(ApplyError::Store(StoreError::Conflict(_)))
    ));
    ws.refresh().unwrap();
    ws.apply(&op, None, 0).unwrap();
    eq(
        cat(&ws.book().budget(m("2026-11"), None), "Dining").assigned,
        3,
    );
}

#[test]
fn the_cached_reload_equals_a_fresh_load_after_a_run_of_operations() {
    let mut ws = open_multi();
    let fun = EntryInput::Open(OpenInput {
        date: "2026-01-01".into(),
        account: "Expenses:Fun".into(),
        currencies: vec!["USD".into()],
        ..Default::default()
    });
    add(&mut ws, fun);
    add(&mut ws, assign_input("2026-01-02", "Expenses:Fun", "40.00"));
    add(
        &mut ws,
        spend("2026-01-03", "Expenses:Fun", "15.00", "Liabilities:Visa"),
    );
    add(
        &mut ws,
        spend("2027-01-03", "Expenses:Fun", "5.00", "Assets:Checking"),
    );
    let snap = ws.store.snapshot().unwrap();
    let source = MemorySource(snap.files.into_iter().collect());
    let mut fresh = Book::from_load(load("main.beancount", &source, None));
    for m in ["2025-12", "2026-01", "2026-02", "2027-01"] {
        let month = m.parse().unwrap();
        assert_eq!(
            ws.book().budget(month, None),
            fresh.budget(month, None),
            "{m}"
        );
    }
    assert_eq!(ws.book().errors(), fresh.errors());
}

#[test]
fn editing_keeps_metadata_types() {
    let meta = [
        "  receipt: \"paper\"",
        "  due: 2026-11-30",
        "  via: Assets:Checking",
        "  count: 3",
        "  fee: 1.50 USD",
        "  ok: TRUE",
        "  tag: #trip",
    ]
    .join("\n");
    let text = format!(
        "{}\n2026-11-03 * \"Typed\"\n{meta}\n  Expenses:Food:Dining  5.00 USD\n  Assets:Checking\n",
        *BASE
    );
    let mut ws = open_files(&[("main.beancount", &text)]);
    let e = find(&ws, |x| x.txn().is_some_and(|t| t.narration == "Typed"));
    assert!(matches!(e.body, Body::Txn(_)));
    let entry = EntryInput::Txn(TransactionInput {
        date: e.date.to_string(),
        narration: "Typed, edited".into(),
        meta: meta_input(&e.meta),
        postings: vec![
            posting("Expenses:Food:Dining", Some("6.00")),
            posting("Assets:Checking", None),
        ],
        ..Default::default()
    });
    let r = ws.core.ref_of(&e);
    ws.apply(&Operation::Edit(r, entry), None, 0).unwrap();
    assert!(
        ws.text("main.beancount")
            .unwrap()
            .contains(&format!("2026-11-03 * \"Typed, edited\"\n{meta}\n"))
    );
    assert_eq!(ws.book().errors(), vec![]);
}
