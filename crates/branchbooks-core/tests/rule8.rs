//! Rule 8 counts only on-budget money that goes to an off-budget account (docs/BUDGET-RULES.md):
//! the off-budget amount that cash and cards paid for, less what the same transaction spent in
//! budgeted categories. Money that reaches an off-budget account from income is not a transfer.
//! On the Examples 1 to 5 ledger: Checking and the card on-budget, Brokerage off-budget.

mod common;

use branchbooks_core::types::Diagnostic;
use common::*;

fn codes(xs: &[Diagnostic]) -> Vec<&str> {
    xs.iter().map(|x| x.code.as_str()).collect()
}

/// November activity in a category, beyond the base ledger's own (Example 5 moved Investing's 500
/// to Brokerage).
fn added_activity(extra: &str, category: &str) -> branchbooks_core::money::Decimal {
    let base = book_of("").budget(m("2026-11"), None);
    let v = book_of(extra).budget(m("2026-11"), None);
    cat(&v, category).activity - cat(&base, category).activity
}

/// A paycheck on Nov 5 that pays `brokerage` into Brokerage and the rest into Checking.
fn paycheck(brokerage: &str, checking: &str, meta: &str) -> String {
    format!(
        "2026-11-05 * \"Employer\" \"Paycheck\"\n{meta}  Income:Salary  -1000.00 USD\n  Assets:Brokerage  {brokerage} USD\n  Assets:Checking  {checking} USD\n"
    )
}

#[test]
fn income_paid_into_an_off_budget_account_is_not_a_transfer() {
    let v = book_of(&paycheck("300.00", "700.00", "")).budget(m("2026-11"), None);
    assert_eq!(codes(&v.flags), Vec::<&str>::new());
    // Unassigned rises by the on-budget part alone (rule 3): 1,100 + 700.
    eq(v.unassigned, 1800);
    eq(
        added_activity(&paycheck("300.00", "700.00", ""), "Investing"),
        0,
    );
}

#[test]
fn a_category_line_on_such_a_paycheck_charges_nothing() {
    let meta = "  category: \"Expenses:Savings:Investing\"\n";
    let v = book_of(&paycheck("300.00", "700.00", meta)).budget(m("2026-11"), None);
    assert_eq!(codes(&v.flags), Vec::<&str>::new());
    eq(
        added_activity(&paycheck("300.00", "700.00", meta), "Investing"),
        0,
    );
}

#[test]
fn money_from_checking_to_an_off_budget_account_still_needs_a_category() {
    let extra =
        "2026-11-05 * \"Top up\"\n  Assets:Brokerage  300.00 USD\n  Assets:Checking  -300.00 USD\n";
    let v = book_of(extra).budget(m("2026-11"), None);
    assert_eq!(codes(&v.flags), ["uncategorized"]);
}

#[test]
fn only_the_part_on_budget_money_paid_for_is_charged_to_the_category() {
    // 400 into Brokerage: 300 from income, 100 from Checking. The category is charged 100.
    let extra = "2026-11-05 * \"Mixed\"\n  category: \"Expenses:Savings:Investing\"\n  Income:Other  -300.00 USD\n  Assets:Brokerage  400.00 USD\n  Assets:Checking  -100.00 USD\n";
    let v = book_of(extra).budget(m("2026-11"), None);
    assert_eq!(codes(&v.flags), Vec::<&str>::new());
    eq(added_activity(extra, "Investing"), -100);
    // The income went to Brokerage, not an on-budget account, so it does not raise Unassigned
    // (rule 3); the 100 from Checking is Investing's cash overspend, which Unassigned does not
    // absorb (rule 6).
    eq(v.unassigned, 1100);
}

#[test]
fn spending_in_the_same_transaction_is_not_counted_as_a_transfer() {
    // A mortgage-style payment: 250 interest to a category, 150 principal to Brokerage, all from
    // Checking. Only the 150 is a transfer.
    let extra = "2026-11-05 * \"Lender\"\n  category: \"Expenses:Housing:Rent\"\n  Expenses:Food:Dining  250.00 USD\n  Assets:Brokerage  150.00 USD\n  Assets:Checking  -400.00 USD\n";
    let v = book_of(extra).budget(m("2026-11"), None);
    assert_eq!(codes(&v.flags), Vec::<&str>::new());
    eq(added_activity(extra, "Rent"), -150);
    eq(added_activity(extra, "Dining"), -250);
}
