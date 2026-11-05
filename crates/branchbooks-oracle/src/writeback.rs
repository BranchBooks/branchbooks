//! The write path, checked by the oracle: sequences of operations applied to copies of the
//! fixtures through the Workspace, and the resulting ledgers written to `out/writeback/<case>/` for
//! Beancount to load. Covers adds of every kind, in-place and relocating edits, deletes, escaped
//! strings, metadata, refusals and new year files.

use std::path::{Path, PathBuf};

use branchbooks_core::write::render::{
    AssignInput, BudgetFlag, CloseInput, EntryInput, MetaInputValue, MoveInput, OpenInput,
    PostingInput, TransactionInput,
};
use branchbooks_core::write::store::{MemoryStore, Store};
use branchbooks_core::write::workspace::{Operation, Workspace};

enum Step {
    Add(EntryInput),
    /// Edit the transaction with this narration.
    Edit(&'static str, EntryInput),
    Delete(&'static str),
}

fn posting(account: &str, amount: Option<&str>) -> PostingInput {
    PostingInput {
        account: account.into(),
        amount: amount.map(Into::into),
        ..Default::default()
    }
}

fn txn(date: &str, narration: &str, postings: Vec<PostingInput>) -> EntryInput {
    EntryInput::Txn(TransactionInput {
        date: date.into(),
        narration: narration.into(),
        postings,
        ..Default::default()
    })
}

fn open(date: &str, account: &str, budget: Option<BudgetFlag>) -> EntryInput {
    EntryInput::Open(OpenInput {
        date: date.into(),
        account: account.into(),
        currencies: vec!["USD".into()],
        budget,
        ..Default::default()
    })
}

fn text(v: &str) -> MetaInputValue {
    MetaInputValue::Text(v.into())
}

/// A case: its name, its starting files (path, text) and the steps applied to them.
type Case = (&'static str, Vec<(String, String)>, Vec<Step>);

fn cases(repo: &Path) -> Vec<Case> {
    let fixture =
        |p: &str| std::fs::read_to_string(repo.join("fixtures").join(p)).expect("a fixture");
    let single = vec![
        Step::Add(open("2026-11-01", "Expenses:Fun", None)),
        Step::Add(open(
            "2026-11-01",
            "Expenses:Group",
            Some(BudgetFlag::Placeholder),
        )),
        Step::Add(EntryInput::Assign(AssignInput {
            date: "2026-11-03".into(),
            category: "Expenses:Fun".into(),
            amount: "50.00".into(),
            currency: None,
        })),
        Step::Add(EntryInput::Move(MoveInput {
            date: "2026-11-04".into(),
            from: "Expenses:Fun".into(),
            to: "Expenses:Food:Groceries".into(),
            amount: "20.00".into(),
            currency: None,
        })),
        Step::Add(EntryInput::Txn(TransactionInput {
            date: "2026-10-20".into(),
            flag: Some("!".into()),
            payee: Some("Café \"Le Coin\"".into()),
            narration: "Back\\slash and quotes".into(),
            tags: vec!["trip".into()],
            links: vec!["r-1".into()],
            meta: vec![
                ("receipt".into(), text("paper")),
                ("notes".into(), text("two\nlines")),
            ],
            postings: vec![
                PostingInput {
                    meta: vec![("memo".into(), text("big lunch"))],
                    ..posting("Expenses:Food:Dining", Some("1,234.50"))
                },
                PostingInput {
                    currency: Some("USD".into()),
                    flag: Some("!".into()),
                    ..posting("Liabilities:CreditCard", Some("-1234.50"))
                },
            ],
        })),
        Step::Add(txn(
            "2026-11-05",
            "Elided",
            vec![
                posting("Expenses:Fun", Some("7.25")),
                posting("Assets:Checking", None),
            ],
        )),
        Step::Edit(
            "Grocery store",
            txn(
                "2026-10-16",
                "Grocery store",
                vec![
                    posting("Expenses:Food:Groceries", Some("130.00")),
                    posting("Assets:Checking", Some("-130.00")),
                ],
            ),
        ),
        Step::Edit(
            "Card payment",
            txn(
                "2026-11-01",
                "Card payment",
                vec![
                    posting("Liabilities:CreditCard", Some("45.00")),
                    posting("Assets:Checking", Some("-45.00")),
                ],
            ),
        ),
        Step::Delete("Bulk grocery run"),
    ];
    let years = vec![
        Step::Add(open("2026-01-01", "Assets:Savings", Some(BudgetFlag::Off))),
        Step::Add(EntryInput::Txn(TransactionInput {
            date: "2026-01-20".into(),
            narration: "To savings".into(),
            meta: vec![("category".into(), text("Expenses:Food:Groceries"))],
            postings: vec![
                posting("Assets:Savings", Some("100.00")),
                posting("Assets:Checking", Some("-100.00")),
            ],
            ..Default::default()
        })),
        Step::Add(EntryInput::Assign(AssignInput {
            date: "2025-12-15".into(),
            category: "Expenses:Food:Groceries".into(),
            amount: "10.00".into(),
            currency: None,
        })),
        Step::Add(txn(
            "2027-03-01",
            "Next year",
            vec![
                posting("Expenses:Housing:Rent", Some("1000.00")),
                posting("Assets:Checking", None),
            ],
        )),
        // Refused (the account is not open yet), then done properly: creates 2024.beancount
        // before the other years.
        Step::Add(txn(
            "2024-06-01",
            "Before the ledger",
            vec![
                posting("Equity:Opening-Balances", Some("-1.00")),
                posting("Equity:Opening-Balances", Some("1.00")),
            ],
        )),
        Step::Add(open("2024-01-01", "Assets:Cash", Some(BudgetFlag::Off))),
        Step::Add(open("2024-01-01", "Income:Gift", None)),
        Step::Add(txn(
            "2024-06-01",
            "Before the ledger",
            vec![
                posting("Assets:Cash", Some("20.00")),
                posting("Income:Gift", None),
            ],
        )),
        Step::Add(EntryInput::Close(CloseInput {
            date: "2027-12-31".into(),
            account: "Assets:Savings".into(),
        })),
    ];
    let year_files = ["main.beancount", "2025.beancount", "2026.beancount"]
        .iter()
        .map(|f| ((*f).to_owned(), fixture(&format!("multifile/{f}"))))
        .collect();
    vec![
        (
            "single-file",
            vec![("main.beancount".into(), fixture("examples-1-5.bean"))],
            single,
        ),
        ("year-files", year_files, years),
    ]
}

/// Applies each case and writes its ledger under `out`; returns each case's main.beancount.
pub fn write(out: &Path) -> Vec<PathBuf> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let _ = std::fs::remove_dir_all(out);
    let mut roots = Vec::new();
    for (name, files, steps) in cases(&repo) {
        let store = MemoryStore::new(files.iter().map(|(p, t)| (p.as_str(), t.as_str())));
        let mut n = 0;
        let ids = Box::new(move || {
            n += 1;
            format!("wb-{n}")
        });
        let mut ws = Workspace::open(store, None, ids).expect("the case opens");
        for step in steps {
            let op = match step {
                Step::Add(e) => Operation::Add(e),
                Step::Edit(narration, e) => Operation::Edit(find(&ws, narration), e),
                Step::Delete(narration) => Operation::Delete(find(&ws, narration)),
            };
            // Refusals are part of what is checked: say so, and keep going.
            if let Err(e) = ws.apply(&op, None, 0) {
                println!("  {name}: refused: {e:?}");
            }
        }
        for (path, body) in ws.store.snapshot().expect("a snapshot").files {
            let at = out.join(name).join(&path);
            std::fs::create_dir_all(at.parent().expect("a folder")).expect("out/writeback");
            std::fs::write(&at, body).expect("a written ledger");
        }
        roots.push(out.join(name).join("main.beancount"));
    }
    roots
}

fn find(
    ws: &Workspace<MemoryStore>,
    narration: &str,
) -> branchbooks_core::write::workspace::EntryRef {
    let (_, e) = ws
        .core
        .entries()
        .find(|(_, e)| e.txn().is_some_and(|t| t.narration == narration))
        .unwrap_or_else(|| panic!("no transaction {narration}"));
    ws.core.ref_of(e)
}
