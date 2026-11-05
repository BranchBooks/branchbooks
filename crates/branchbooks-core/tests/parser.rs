//! The parser, from text to entries and errors.

use branchbooks_core::ledger::build_ledger;
use branchbooks_core::loader::{MemorySource, load};
use branchbooks_core::money::{ZERO, plain};
use branchbooks_core::parser::parse;
use branchbooks_core::types::{Body, EntryType, MetaValue};

#[test]
fn parses_options_opens_with_metadata_transactions_custom() {
    let r = parse(
        r#"option "operating_currency" "USD"
2026-10-01 open Assets:Checking USD,CAD "STRICT"
  budget: "on"
2026-10-15 * "Payee" "Narration" #tag ^link
  note: "txn meta"
  Income:Salary   -1,234.50 USD
    memo: "posting meta"
  ! Assets:Checking  ; inline comment
2026-10-15 custom "envelope-move" Expenses:A Expenses:B 20.00 USD
"#,
        "f.bean",
    );
    assert_eq!(r.errors, vec![]);
    assert_eq!(
        (r.options[0].key.as_str(), r.options[0].value.as_str()),
        ("operating_currency", "USD")
    );
    let [open, txn, custom] = &r.entries[..] else {
        panic!("three entries: {:?}", r.entries)
    };

    let Body::Open(o) = &open.body else { panic!() };
    assert_eq!(o.account, "Assets:Checking");
    assert_eq!(o.currencies, ["USD", "CAD"]);
    assert_eq!(o.booking.as_deref(), Some("STRICT"));
    assert_eq!(
        open.meta.get("budget"),
        Some(&MetaValue::String("on".into()))
    );

    let t = txn.txn().unwrap();
    assert_eq!(t.payee.as_deref(), Some("Payee"));
    assert_eq!(t.narration, "Narration");
    assert_eq!(t.tags, ["tag"]);
    assert_eq!(t.links, ["link"]);
    assert_eq!(txn.meta.str("note"), Some("txn meta"));
    let units = t.postings[0].units.as_ref().unwrap();
    assert_eq!(plain(units.number), "-1234.5");
    assert_eq!(units.scale, 2);
    assert_eq!(t.postings[0].meta.str("memo"), Some("posting meta"));
    assert_eq!(t.postings[1].account, "Assets:Checking");
    assert_eq!(t.postings[1].units, None);
    assert_eq!(t.postings[1].flag.as_deref(), Some("!"));

    assert_eq!(custom.custom().unwrap().name, "envelope-move");
}

#[test]
fn reports_parse_errors_with_file_and_line_and_keeps_going() {
    let r = parse(
        r#"2026-01-01 open Assets:A USD
2026-01-02 * "bad"
  Assets:A  1.00 USD USD
  Assets:A
2026-01-03 nonsense
2026-01-04 * "fine"
  Assets:A  1.00 USD
  Assets:A
2026-01-05 open Bogus:Root
2026-01-06 * "cost"
  Assets:A  1 HOOL {10.00 USD}
  Assets:A
"#,
        "dir/x.bean",
    );
    let at: Vec<(&str, u32)> = r.errors.iter().map(|e| (&*e.file, e.line)).collect();
    assert_eq!(
        at,
        [
            ("dir/x.bean", 3),
            ("dir/x.bean", 5),
            ("dir/x.bean", 9),
            ("dir/x.bean", 11)
        ]
    );
    let types: Vec<EntryType> = r
        .entries
        .iter()
        .map(branchbooks_core::types::Entry::entry_type)
        .collect();
    assert_eq!(types, [EntryType::Open, EntryType::Txn]);
}

#[test]
fn expands_includes_depth_first_in_file_order_and_reports_errors_in_included_files() {
    let src = MemorySource::new([
        (
            "/l/main.beancount",
            "option \"operating_currency\" \"USD\"\n2026-01-01 open Assets:A USD\ninclude \"2026.beancount\"\n2026-01-01 open Assets:B USD\n",
        ),
        (
            "/l/2026.beancount",
            "2026-02-01 * \"x\"\n  Assets:A  1.00 USD\n  Assets:B\n2026-02-02 oops\n",
        ),
    ]);
    let l = load("/l/main.beancount", &src, None);
    let got: Vec<(EntryType, &str, u64)> = l
        .entries
        .iter()
        .map(|e| (e.entry_type(), &*e.file, e.seq))
        .collect();
    assert_eq!(
        got,
        // seq: file load order, then line (same-day tie-break, rule 14)
        [
            (EntryType::Open, "/l/main.beancount", 2),
            (EntryType::Txn, "/l/2026.beancount", 10_000_001),
            (EntryType::Open, "/l/main.beancount", 4),
        ]
    );
    assert_eq!(l.errors.len(), 1);
    assert_eq!(
        (
            &*l.errors[0].file,
            l.errors[0].line,
            l.errors[0].code.as_str()
        ),
        ("/l/2026.beancount", 4, "syntax")
    );
}

#[test]
fn reports_a_missing_include() {
    let src = MemorySource::new([("/m.bean", "include \"nope.bean\"\n")]);
    let ledger = build_ledger(&load("/m.bean", &src, None));
    assert_eq!(ledger.errors.len(), 1);
    let e = &ledger.errors[0];
    assert_eq!(
        (&*e.file, e.line, e.code.as_str()),
        ("/m.bean", 1, "include")
    );
}

#[test]
fn never_uses_floats_amounts_keep_every_digit() {
    let r = parse(
        "2026-01-01 * \"x\"\n  Assets:A  0.1 USD\n  Assets:A  0.2 USD\n  Assets:B  -0.3 USD\n",
        "f",
    );
    let t = r.entries[0].txn().unwrap();
    let total = t
        .postings
        .iter()
        .fold(ZERO, |s, p| s + p.units.as_ref().unwrap().number);
    assert_eq!(plain(total), "0");
}
