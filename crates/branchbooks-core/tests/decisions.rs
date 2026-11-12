//! The budget decisions in docs/DESIGN.md.

mod common;

use branchbooks_core::types::Diagnostic;
use common::*;

fn mv(date: &str, from: &str, to: &str, amount: &str) -> String {
    format!("{date} custom \"envelope-move\" {from} {to} {amount} USD\n")
}

fn codes(xs: &[Diagnostic]) -> Vec<&str> {
    xs.iter().map(|x| x.code.as_str()).collect()
}

// 8. Card payments and transfers count whatever else the transaction holds.

const LUNCH: &str =
    "2026-11-05 * \"Lunch\"\n  Expenses:Food:Dining  60.00 USD\n  Liabilities:CreditCard\n";

#[test]
fn d8_a_card_payment_and_a_fee_in_one_transaction_match_two_transactions() {
    let one = format!(
        "{LUNCH}2026-11-06 * \"Pay with fee\"\n  Assets:Checking  -65.00 USD\n  Liabilities:CreditCard  60.00 USD\n  Expenses:Food:Dining  5.00 USD\n"
    );
    let two = format!(
        "{LUNCH}2026-11-06 * \"Pay\"\n  Assets:Checking  -60.00 USD\n  Liabilities:CreditCard\n2026-11-06 * \"Fee\"\n  Assets:Checking  -5.00 USD\n  Expenses:Food:Dining\n"
    );
    for extra in [one, two] {
        let v = book_of(&extra).budget(m("2026-11"), None);
        eq(v.unassigned, 1080);
        eq(v.liabilities[0].available, 0);
        eq(v.liabilities[0].owed, 0);
    }
}

#[test]
fn d8_a_balance_transfer_moves_the_set_aside_money_to_the_other_card() {
    let extra = format!(
        "2026-11-01 open Liabilities:Other USD\n  budget: \"on\"\n{LUNCH}2026-11-06 * \"Transfer\"\n  Liabilities:Other  -60.00 USD\n  Liabilities:CreditCard\n"
    );
    let v = book_of(&extra).budget(m("2026-11"), None);
    eq(v.unassigned, 1080);
    let card = |name: &str| v.liabilities.iter().find(|c| c.account == name).unwrap();
    eq(card("Liabilities:CreditCard").available, 0);
    eq(card("Liabilities:CreditCard").owed, 0);
    eq(card("Liabilities:Other").available, 60);
    eq(card("Liabilities:Other").owed, 60);
    eq(card("Liabilities:Other").underfunded, 0);
}

#[test]
fn d8_a_cash_advance_is_not_a_transfer() {
    let extra =
        "2026-11-06 * \"Advance\"\n  Assets:Checking  100.00 USD\n  Liabilities:CreditCard\n";
    let v = book_of(extra).budget(m("2026-11"), None);
    eq(v.unassigned, 1180);
    eq(v.liabilities[0].available, 0);
    eq(v.liabilities[0].underfunded, 100);
}

// 1. Rule 15 warns only when the correction is dated after the last spending.

#[test]
fn d1_example_5_same_day_assignment_and_transfer_no_warning() {
    assert_eq!(book_of("").budget(m("2026-11"), None).warnings, vec![]);
}

#[test]
fn d1_example_12_same_day_assignment_and_purchase_no_warning_same_numbers() {
    let v = fixture("scenarios/ex12-same-day-assign-first.bean").budget(m("2026-11"), None);
    assert_eq!(v.warnings, vec![]);
    eq(v.unassigned, 1040);
}

#[test]
fn d1_an_assignment_the_day_after_the_spending_still_warns() {
    let extra = format!(
        "2026-11-05 * \"Dinner\"\n  Expenses:Food:Dining  200.00 USD\n  Liabilities:CreditCard\n{}",
        assign("2026-11-06", "Expenses:Food:Dining", "45.00")
    );
    let v = book_of(&extra).budget(m("2026-11"), None);
    assert_eq!(codes(&v.warnings), ["late-correction"]);
    assert!(v.warnings[0].message.contains("spending on 2026-11-05"));
}

// 2. Moves and negative assignments may not overspend a category.

#[test]
fn d2_a_move_larger_than_the_source_available_is_skipped_and_flagged() {
    let v = book_of(&mv(
        "2026-11-10",
        "Expenses:Housing:Rent",
        "Expenses:Food:Dining",
        "2000.00",
    ))
    .budget(m("2026-11"), None);
    eq(cat(&v, "Rent").available, 1500);
    eq(cat(&v, "Dining").available, 155);
    eq(v.unassigned, 1080);
    assert_eq!(codes(&v.flags), ["blocked"]);
}

#[test]
fn d2_a_move_of_exactly_the_source_available_is_allowed() {
    let v = book_of(&mv(
        "2026-11-10",
        "Expenses:Housing:Rent",
        "Expenses:Food:Dining",
        "1500.00",
    ))
    .budget(m("2026-11"), None);
    eq(cat(&v, "Rent").available, 0);
    eq(cat(&v, "Dining").available, 1655);
    assert_eq!(v.flags, vec![]);
}

#[test]
fn d2_a_move_out_of_an_overspent_category_is_blocked() {
    let groceries = "2026-11-08 * \"Groceries\"
  Expenses:Food:Groceries  20.00 USD
  Assets:Checking
";
    let v = book_of(&format!(
        "{groceries}{}",
        mv(
            "2026-11-10",
            "Expenses:Food:Groceries",
            "Expenses:Food:Dining",
            "5.00"
        )
    ))
    .budget(m("2026-11"), None);
    eq(cat(&v, "Groceries").available, -20);
    assert_eq!(codes(&v.flags), ["blocked"]);
}

#[test]
fn d2_a_negative_assignment_that_would_overspend_is_blocked_one_that_fits_is_applied() {
    let blocked = book_of(&assign("2026-11-10", "Expenses:Food:Dining", "-200.00"))
        .budget(m("2026-11"), None);
    eq(cat(&blocked, "Dining").available, 155);
    assert_eq!(codes(&blocked.flags), ["blocked"]);
    let ok = book_of(&assign("2026-11-10", "Expenses:Food:Dining", "-155.00"))
        .budget(m("2026-11"), None);
    eq(cat(&ok, "Dining").available, 0);
    eq(ok.unassigned, 1235);
    assert_eq!(ok.flags, vec![]);
}

#[test]
fn d2_a_negative_move_runs_the_other_way_and_is_checked_on_that_side() {
    let v = book_of(&mv(
        "2026-11-10",
        "Expenses:Housing:Rent",
        "Expenses:Food:Dining",
        "-100.00",
    ))
    .budget(m("2026-11"), None);
    eq(cat(&v, "Dining").available, 55);
    eq(cat(&v, "Rent").available, 1600);
}

// 4. Mixed funding: cash first, then cards.

fn split(amount: &str, cash: &str, card_amt: &str) -> String {
    format!(
        "2026-11-05 * \"Split payment\"\n  Expenses:Food:Dining  {amount} USD\n  Assets:Checking  -{cash} USD\n  Liabilities:CreditCard  -{card_amt} USD\n"
    )
}

#[test]
fn d4_funded_purchase_card_share_moves_to_the_payment_envelope() {
    let v = book_of(&split("100.00", "60.00", "40.00")).budget(m("2026-11"), None);
    eq(cat(&v, "Dining").available, 55);
    eq(liability(&v).available, 40);
    eq(v.unassigned, 1080);
    assert_eq!(v.flags, vec![]);
}

#[test]
fn d4_overspend_cash_share_is_charged_first_so_the_overspend_is_cash() {
    let v = book_of(&split("200.00", "50.00", "150.00")).budget(m("2026-11"), None);
    eq(cat(&v, "Dining").available, -45);
    eq(cat(&v, "Dining").cash_overspent, 45);
    eq(cat(&v, "Dining").credit_overspent, 0);
    eq(liability(&v).available, 150);
    eq(liability(&v).underfunded, 0);
}

// 6. Unassigned is computed for today, the same on every month view.

#[test]
fn d6_today_in_november_december_assignment_is_future_in_every_view() {
    let mut book = fixture("scenarios/ex08-future-month.bean");
    for m in ["2026-10", "2026-11", "2026-12"] {
        let v = book.budget(m.parse().unwrap(), Some(d("2026-11-03")));
        eq(v.unassigned, 780);
        eq(v.assigned_to_future, 300);
    }
    eq(
        cat(&book.budget(m("2026-12"), Some(d("2026-11-03"))), "Rent").available,
        1800,
    );
    eq(
        cat(
            &book.budget(m("2026-10"), Some(d("2026-11-03"))),
            "Groceries",
        )
        .available,
        -20,
    );
}

#[test]
fn d6_today_in_october_the_november_and_december_assignments_are_both_future() {
    let v = fixture("scenarios/ex08-future-month.bean").budget(m("2026-10"), Some(d("2026-10-20")));
    eq(v.assigned_to_future, 800);
    eq(v.unassigned, 800);
}

#[test]
fn d6_today_in_december_nothing_is_future() {
    let v = fixture("scenarios/ex08-future-month.bean").budget(m("2026-11"), Some(d("2026-12-02")));
    eq(v.assigned_to_future, 0);
    eq(v.unassigned, 780);
    eq(cat(&v, "Rent").available, 1500);
}
