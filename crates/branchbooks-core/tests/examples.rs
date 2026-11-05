//! The brief's worked examples, run end to
//! end. Beancount text -> parser -> ledger -> budget.

mod common;

use branchbooks_core::money::plain;
use branchbooks_core::reports::{balance_sheet, income_statement};
use common::*;

fn codes(ds: &[branchbooks_core::types::Diagnostic]) -> Vec<&str> {
    ds.iter().map(|d| d.code.as_str()).collect()
}

#[test]
fn example_1_paycheck() {
    let mut book = book_of_with("", &up_to(1));
    assert_eq!(book.errors(), vec![]);
    eq(balance(&mut book, "Assets:Checking"), 3900);
    eq(book.budget(m("2026-10"), None).unassigned, 3900);
    let is = income_statement(book.transactions(), "USD", None, None);
    eq(is.income, 5000);
    eq(is.expenses, 1100);
    eq(is.net, 3900);
}

#[test]
fn example_2_assignments() {
    eq(
        book_of_with("", &up_to(2))
            .budget(m("2026-10"), None)
            .unassigned,
        1600,
    );
}

#[test]
fn example_3_cash_and_card_spending_card_payment() {
    let step1 = groceries("2026-10-16", "120.00");
    let step2 = step1.clone() + &dining("2026-10-17", "45.00", "Liabilities:CreditCard");

    let v = book_of_with(&step1, &up_to(2)).budget(m("2026-10"), None);
    eq(cat(&v, "Groceries").available, 480);
    eq(v.unassigned, 1600);

    let v = book_of_with(&step2, &up_to(2)).budget(m("2026-10"), None);
    eq(cat(&v, "Dining").available, 155);
    eq(card(&v).owed, 45);
    eq(card(&v).available, 45);
    eq(v.unassigned, 1600);

    let mut book = book_of_with("", &up_to(3));
    let v = book.budget(m("2026-10"), None);
    eq(balance(&mut book, "Assets:Checking"), 3735);
    eq(card(&v).available, 0);
    eq(v.unassigned, 1600);
}

#[test]
fn example_4_cash_overspending_resets_at_month_end() {
    let mut book = book_of_with("", &up_to(4));
    let v = book.budget(m("2026-10"), None);
    eq(cat(&v, "Groceries").available, -20);
    eq(balance(&mut book, "Assets:Checking"), 3235);
    eq(v.unassigned, 1600);
    eq(cat(&v, "Groceries").cash_overspent, 20);
    eq(cat(&v, "Groceries").credit_overspent, 0);

    let v = book_of_with("", &up_to(4)).budget(m("2026-11"), None);
    eq(cat(&v, "Groceries").carried_in, 0);
    eq(cat(&v, "Groceries").available, 0);
    eq(v.unassigned, 1580);

    // 20.00 assigned before Nov 1, with a rule 15 warning
    let mut book = book_of_with(
        &assign("2026-10-31", "Expenses:Food:Groceries", "20.00"),
        &up_to(4),
    );
    let oct = book.budget(m("2026-10"), None);
    eq(cat(&oct, "Groceries").available, 0);
    eq(oct.unassigned, 1580);
    assert_eq!(codes(&oct.warnings), ["late-correction"]);
    let nov = book.budget(m("2026-11"), None);
    eq(cat(&nov, "Groceries").available, 0);
    eq(nov.unassigned, 1580);
}

#[test]
fn example_5_off_budget_transfer() {
    let assign_only = book_of_with(
        &assign("2026-11-02", "Expenses:Savings:Investing", "500.00"),
        &up_to(4),
    )
    .budget(m("2026-11"), None);
    eq(assign_only.unassigned, 1080);
    let mut book = book_of_with("", &up_to(5));
    let v = book.budget(m("2026-11"), None);
    eq(v.unassigned, 1080);
    eq(cat(&v, "Investing").available, 0);
    eq(balance(&mut book, "Assets:Checking"), 2735);
    eq(balance(&mut book, "Assets:Brokerage"), 500);
    assert_eq!(v.flags, vec![]);
}

#[test]
fn example_5_without_the_category_line_is_flagged_uncategorized() {
    let v = fixture("scenarios/ex05-uncategorized.bean").budget(m("2026-11"), None);
    assert_eq!(codes(&v.flags), ["uncategorized"]);
    let base = BASE.replace("  category: \"Expenses:Savings:Investing\"\n", "");
    let v = book_of_with("", &base).budget(m("2026-11"), None);
    assert_eq!(codes(&v.flags), ["uncategorized"]);
}

#[test]
fn state_after_example_5_matches_the_brief() {
    let mut book = book_of("");
    let v = book.budget(m("2026-11"), None);
    eq(balance(&mut book, "Assets:Checking"), 2735);
    eq(cat(&v, "Rent").available, 1500);
    eq(cat(&v, "Groceries").available, 0);
    eq(cat(&v, "Dining").available, 155);
    eq(v.unassigned, 1080);
}

#[test]
fn example_6_credit_overspending() {
    let ex6 = dining("2026-11-05", "200.00", "Liabilities:CreditCard");
    let v = fixture("scenarios/ex06-credit-overspend.bean").budget(m("2026-11"), None);
    eq(cat(&v, "Dining").available, -45);
    eq(cat(&v, "Dining").cash_overspent, 0);
    eq(cat(&v, "Dining").credit_overspent, 45);
    eq(card(&v).available, 155);
    eq(card(&v).owed, 200);
    eq(card(&v).underfunded, 0);
    assert_eq!(
        waiting(&v),
        [("Expenses:Food:Dining".to_owned(), "45".to_owned())]
    );
    eq(v.unassigned, 1080);

    let v = book_of(&(ex6.clone() + &assign("2026-11-05", "Expenses:Food:Dining", "45.00")))
        .budget(m("2026-11"), None);
    eq(cat(&v, "Dining").available, 0);
    eq(card(&v).available, 200);
    assert!(waiting(&v).is_empty());
    eq(v.unassigned, 1035);

    let v = book_of(&ex6).budget(m("2026-12"), None);
    eq(cat(&v, "Dining").available, 0);
    eq(card(&v).available, 200);
    eq(card(&v).underfunded, 0);
    eq(v.unassigned, 1035);
}

#[test]
fn example_7_card_refund() {
    let v = book_of(&dining("2026-11-08", "60.00", "Liabilities:CreditCard"))
        .budget(m("2026-11"), None);
    eq(cat(&v, "Dining").available, 95);
    eq(card(&v).available, 60);
    eq(v.unassigned, 1080);

    let v = fixture("scenarios/ex07-card-refund.bean").budget(m("2026-11"), None);
    eq(cat(&v, "Dining").available, 120);
    eq(card(&v).available, 35);
    eq(card(&v).owed, 35);
    eq(v.unassigned, 1080);
}

#[test]
fn example_8_future_month() {
    let mut book = fixture("scenarios/ex08-future-month.bean");
    let v = book.budget(m("2026-11"), None);
    eq(v.unassigned, 780);
    eq(cat(&v, "Rent").available, 1500);
    eq(v.assigned_to_future, 300);
    let v = book.budget(m("2026-12"), None);
    eq(cat(&v, "Rent").available, 1800);
    eq(v.unassigned, 780);
    eq(v.assigned_to_future, 0);
}

#[test]
fn example_9_mixed_overspending() {
    let ex9 = dining("2026-11-05", "200.00", "Liabilities:CreditCard")
        + &dining("2026-11-06", "30.00", "Assets:Checking");
    let with_assign = |amount: &str, date: &str| {
        book_of(&(ex9.clone() + &assign(date, "Expenses:Food:Dining", amount)))
    };

    let mut book = fixture("scenarios/ex09-mixed-overspend.bean");
    let v = book.budget(m("2026-11"), None);
    eq(cat(&v, "Dining").available, -75);
    eq(cat(&v, "Dining").cash_overspent, 30);
    eq(cat(&v, "Dining").credit_overspent, 45);
    eq(balance(&mut book, "Assets:Checking"), 2705);
    eq(card(&v).available, 155);
    eq(card(&v).underfunded, 0);
    eq(v.unassigned, 1080);

    let v = with_assign("20.00", "2026-11-07").budget(m("2026-11"), None);
    eq(cat(&v, "Dining").cash_overspent, 10);
    eq(cat(&v, "Dining").credit_overspent, 45);
    eq(v.unassigned, 1060);

    let mut book = with_assign("40.00", "2026-11-07");
    let v = book.budget(m("2026-11"), None);
    eq(cat(&v, "Dining").cash_overspent, 0);
    eq(cat(&v, "Dining").credit_overspent, 35);
    eq(card(&v).available, 165);
    eq(v.unassigned, 1040);
    let dec = book.budget(m("2026-12"), None);
    eq(cat(&dec, "Dining").available, 0);
    eq(card(&dec).available, 200);
    eq(card(&dec).underfunded, 0);
    eq(dec.unassigned, 1005);

    let v = with_assign("75.00", "2026-11-07").budget(m("2026-11"), None);
    eq(cat(&v, "Dining").available, 0);
    eq(card(&v).available, 200);
    eq(v.unassigned, 1005);

    let v = book_of(&ex9).budget(m("2026-12"), None);
    eq(cat(&v, "Dining").available, 0);
    eq(card(&v).available, 200);
    eq(card(&v).underfunded, 0);
    eq(v.unassigned, 1005);

    // The same 40 assigned on Nov 1, before the purchases
    let v = with_assign("40.00", "2026-11-01").budget(m("2026-11"), None);
    eq(cat(&v, "Dining").cash_overspent, 30);
    eq(cat(&v, "Dining").credit_overspent, 5);
    eq(card(&v).available, 195);
    eq(v.unassigned, 1040);
    assert!(v.warnings.iter().all(|w| !w.message.contains("Dining")));
}

#[test]
fn example_10_partial_reimbursement() {
    let mut book = fixture("scenarios/ex10-partial-reimbursement.bean");
    let v = book.budget(m("2026-11"), None);
    eq(balance(&mut book, "Assets:Checking"), 2935);
    eq(cat(&v, "Groceries").available, 150);
    eq(v.unassigned, 1130);
}

#[test]
fn example_11_envelope_move() {
    let v = fixture("scenarios/ex11-move.bean").budget(m("2026-11"), None);
    eq(cat(&v, "Rent").available, 1480);
    eq(cat(&v, "Groceries").available, 20);
    eq(v.unassigned, 1080);
    eq(cat(&v, "Groceries").cash_overspent, 0);
}

#[test]
fn example_12_same_day_order() {
    for f in [
        "scenarios/ex12-same-day-assign-first.bean",
        "scenarios/ex12-same-day-purchase-first.bean",
    ] {
        let v = fixture(f).budget(m("2026-11"), None);
        eq(cat(&v, "Dining").available, -35);
        eq(cat(&v, "Dining").cash_overspent, 0);
        eq(cat(&v, "Dining").credit_overspent, 35);
        eq(card(&v).available, 165);
        eq(v.unassigned, 1040);
    }
    // sim5: assign applies before move on the same day
    let v = fixture("scenarios/sim5-assign-then-move.bean").budget(m("2026-11"), None);
    eq(cat(&v, "Fun").available, 20);
    eq(cat(&v, "Groceries").available, 30);
    eq(v.unassigned, 1030);
}

/// `custom "overspending"` for a category, dated.
fn overspending(date: &str, category: &str, how: &str) -> String {
    format!("{date} custom \"overspending\" {category} \"{how}\"\n")
}

/// The card's credit overspending still in categories, as plain numbers.
fn waiting(v: &branchbooks_core::budget::MonthView) -> Vec<(String, String)> {
    card(v)
        .in_categories
        .iter()
        .map(|(c, x)| (c.clone(), plain(*x)))
        .collect()
}

#[test]
fn example_15_rollover_overspending_keeps_the_cash_overspend() {
    let flag = overspending("2026-10-01", "Expenses:Food:Groceries", "rollover");
    let v = book_of_with(&flag, &up_to(4)).budget(m("2026-11"), None);
    eq(cat(&v, "Groceries").carried_in, -20);
    eq(cat(&v, "Groceries").available, -20);
    eq(cat(&v, "Groceries").cash_overspent, 20);
    eq(v.unassigned, 1600);

    let fix = flag + &assign("2026-11-03", "Expenses:Food:Groceries", "20.00");
    let v = book_of_with(&fix, &up_to(4)).budget(m("2026-11"), None);
    eq(cat(&v, "Groceries").available, 0);
    eq(v.unassigned, 1580);
    assert_eq!(v.warnings, vec![]);

    let v = fixture("scenarios/ex15-rollover-overspending.bean").budget(m("2026-11"), None);
    eq(cat(&v, "Groceries").available, -20);
    eq(v.unassigned, 1100);
}

#[test]
fn example_16_rollover_overspending_carries_both_portions() {
    let mut book = fixture("scenarios/ex16-rollover-mixed.bean");
    let v = book.budget(m("2026-12"), None);
    eq(cat(&v, "Dining").available, -75);
    eq(cat(&v, "Dining").cash_overspent, 30);
    eq(cat(&v, "Dining").credit_overspent, 45);
    eq(cat(&v, "Dining").activity, 0);
    eq(card(&v).available, 155);
    eq(card(&v).owed, 200);
    eq(card(&v).underfunded, 0);
    assert_eq!(
        waiting(&v),
        [("Expenses:Food:Dining".to_owned(), "45".to_owned())]
    );
    eq(v.unassigned, 1080);

    let ex16 = dining("2026-11-05", "200.00", "Liabilities:CreditCard")
        + &dining("2026-11-06", "30.00", "Assets:Checking")
        + &overspending("2026-11-01", "Expenses:Food:Dining", "rollover")
        + &assign("2026-12-03", "Expenses:Food:Dining", "40.00");
    let v = book_of(&ex16).budget(m("2026-12"), None);
    eq(cat(&v, "Dining").available, -35);
    eq(cat(&v, "Dining").cash_overspent, 0);
    eq(cat(&v, "Dining").credit_overspent, 35);
    eq(card(&v).available, 165);
    eq(v.unassigned, 1040);
    assert_eq!(v.warnings, vec![]);
}

#[test]
fn example_17_reset_turns_rollover_overspending_off() {
    let mut book = fixture("scenarios/ex17-rollover-reset.bean");
    let v = book.budget(m("2026-11"), None);
    eq(cat(&v, "Groceries").available, -20);
    eq(v.unassigned, 1100);
    let v = book.budget(m("2026-12"), None);
    eq(cat(&v, "Groceries").available, 0);
    eq(v.unassigned, 1080);
}

#[test]
fn example_18_set_aside_twice_is_overfunded() {
    let ex6 = dining("2026-11-05", "200.00", "Liabilities:CreditCard");
    let on_card = ex6.clone() + &assign("2026-11-06", "Liabilities:CreditCard", "45.00");
    let v = book_of(&on_card).budget(m("2026-11"), None);
    eq(card(&v).available, 200);
    eq(card(&v).underfunded, 0);
    eq(card(&v).overfunded, 45);
    eq(v.unassigned, 1035);

    let both = on_card.clone() + &assign("2026-11-07", "Expenses:Food:Dining", "45.00");
    let v = book_of(&both).budget(m("2026-11"), None);
    eq(card(&v).available, 245);
    eq(card(&v).overfunded, 45);
    eq(v.unassigned, 990);

    let back = both + &assign("2026-11-08", "Liabilities:CreditCard", "-45.00");
    let v = book_of(&back).budget(m("2026-11"), None);
    eq(card(&v).available, 200);
    eq(card(&v).overfunded, 0);
    eq(v.unassigned, 1035);

    // Funded once, in Dining: nothing is flagged.
    let v = book_of(&(ex6 + &assign("2026-11-07", "Expenses:Food:Dining", "45.00")))
        .budget(m("2026-11"), None);
    eq(card(&v).overfunded, 0);
}

#[test]
fn an_overspending_entry_must_name_a_category_and_rollover_or_reset() {
    for bad in [
        overspending("2026-11-01", "Assets:Checking", "rollover"),
        overspending("2026-11-01", "Expenses:Food:Dining", "carry"),
        "2026-11-01 custom \"overspending\" Expenses:Food:Dining\n".to_owned(),
    ] {
        let mut book = book_of(&bad);
        assert_eq!(codes(&book.budget_diagnostics()), ["envelope"], "{bad}");
        eq(book.budget(m("2026-11"), None).unassigned, 1080);
    }
}

#[test]
fn example_14_reports_as_of_nov_2() {
    let mut book = book_of("");
    let is = income_statement(book.transactions(), "USD", None, Some(d("2026-11-02")));
    eq(is.income, 5000);
    eq(is.expenses, 1765);
    eq(is.net, 3235);
    let lines: Vec<(&str, String)> = is
        .expense_lines
        .iter()
        .map(|l| (l.account.as_str(), plain(l.amount)))
        .collect();
    let want = [
        ("Expenses:Benefits:401k", "300"),
        ("Expenses:Food:Dining", "45"),
        ("Expenses:Food:Groceries", "620"),
        ("Expenses:Tax:Federal", "800"),
    ];
    assert_eq!(lines, want.map(|(a, n)| (a, n.to_owned())));

    let bs = balance_sheet(book.transactions(), "USD", Some(d("2026-11-02")));
    eq(bs.assets, 3235);
    eq(bs.liabilities, 0);
    eq(bs.net_worth, 3235);
    let assets: Vec<(&str, String)> = bs
        .asset_lines
        .iter()
        .map(|l| (l.account.as_str(), plain(l.amount)))
        .collect();
    assert_eq!(
        assets,
        [
            ("Assets:Brokerage", "500".to_owned()),
            ("Assets:Checking", "2735".to_owned())
        ]
    );

    let v = book.budget(m("2026-10"), None);
    let row = |n: &str| {
        let r = cat(&v, n);
        [r.assigned, r.activity, r.available].map(plain)
    };
    assert_eq!(row("Rent"), ["1500", "0", "1500"]);
    assert_eq!(row("Groceries"), ["600", "-620", "-20"]);
    assert_eq!(row("Dining"), ["200", "-45", "155"]);
}

#[test]
fn purchase_order_does_not_matter_at_ledger_level() {
    let minimal = r#"
2026-12-01 open Assets:Checking USD
  budget: "on"
2026-12-01 open Liabilities:CreditCard USD
  budget: "on"
2026-12-01 open Income:Salary USD
2026-12-01 open Expenses:Food:Dining USD
2026-12-01 * "Paycheck"
  Income:Salary  -5000.00 USD
  Assets:Checking
2026-12-01 custom "envelope-assign" Expenses:Food:Dining 100.00 USD
"#;
    let cash = |date: &str| dining(date, "60.00", "Assets:Checking");
    let credit = |date: &str| dining(date, "70.00", "Liabilities:CreditCard");
    for text in [
        cash("2026-12-02") + &credit("2026-12-03"),
        credit("2026-12-02") + &cash("2026-12-03"),
    ] {
        let v = book_of_with(&text, minimal).budget(m("2026-12"), None);
        eq(cat(&v, "Dining").cash_overspent, 30);
        eq(cat(&v, "Dining").credit_overspent, 0);
        eq(card(&v).available, 70);
        eq(v.unassigned, 4900);
    }
}
