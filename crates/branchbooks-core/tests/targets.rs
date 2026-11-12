//! Target budgeting, rules T1 to T7 in docs/BUDGET-RULES.md: targets are rules, envelopes are results.

mod common;

use branchbooks_core::book::Book;
use branchbooks_core::budget::{Recurrence, Rollover};
use branchbooks_core::loader::MemorySource;
use common::*;

const OPENS: &str = r#"option "operating_currency" "USD"
2026-01-01 open Assets:Checking USD
  budget: "on"
2026-01-01 open Income:Salary USD
2026-01-01 open Expenses:Housing:Rent USD
2026-01-01 open Expenses:Savings:CarFund USD
2026-01-01 open Expenses:Food:Groceries USD
"#;

fn paycheck(date: &str, amount: &str) -> String {
    format!("{date} * \"Paycheck\"\n  Income:Salary  -{amount} USD\n  Assets:Checking\n")
}

fn rent(date: &str) -> String {
    format!("{date} * \"Rent\"\n  Expenses:Housing:Rent  1400.00 USD\n  Assets:Checking\n")
}

const RENT_TARGET: &str = "2026-01-01 custom \"target\" Expenses:Housing:Rent 1400.00 USD\n";
const CAR_TARGET: &str =
    "2026-01-01 custom \"target\" Expenses:Savings:CarFund 6000.00 USD\n  due: 2026-06-01\n";
const GROCERIES: &str =
    "2026-01-01 custom \"envelope-assign\" Expenses:Food:Groceries 400.00 USD\n";

fn book(parts: &[&str]) -> Book {
    let text = format!("{OPENS}{}", parts.concat());
    Book::load("/t.bean", &MemorySource::new([("/t.bean", text.as_str())]))
}

/// January with a 5,000 paycheck, rent paid, Groceries assigned, and two targets.
fn january() -> Vec<String> {
    vec![
        paycheck("2026-01-01", "5000.00"),
        GROCERIES.into(),
        RENT_TARGET.into(),
        CAR_TARGET.into(),
        rent("2026-01-03"),
    ]
}

fn refs(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

#[test]
fn targets_fund_from_unassigned_by_due_month_then_name() {
    let mut b = book(&refs(&january()));
    assert_eq!(b.errors(), vec![]);
    assert_eq!(b.budget_diagnostics(), vec![]);
    let v = b.budget(m("2026-01"), None);
    // Rent (monthly, due this month) refills to 1,400 and covers the rent paid; CarFund (due
    // June) gets 6,000 over six months.
    eq(cat(&v, "Rent").target_allocated, 1400);
    eq(cat(&v, "Rent").available, 0);
    eq(cat(&v, "Rent").cash_overspent, 0);
    eq(cat(&v, "CarFund").target_allocated, 1000);
    eq(cat(&v, "CarFund").available, 1000);
    eq(cat(&v, "Groceries").target_allocated, 0);
    eq(cat(&v, "Groceries").available, 400);
    // 5,000 income, 400 assigned, 2,400 to targets.
    eq(v.unassigned, 2200);
    assert_eq!(v.shortfalls, vec![]);
    let t = cat(&v, "CarFund").target.as_ref().unwrap();
    assert_eq!(
        (t.recurrence, t.rollover, t.due),
        (
            Recurrence::Once,
            Rollover::CarryShortfall,
            Some(d("2026-06-01"))
        )
    );
    let t = cat(&v, "Rent").target.as_ref().unwrap();
    assert_eq!(
        (t.recurrence, t.rollover),
        (Recurrence::Monthly, Rollover::Reset)
    );
}

#[test]
fn a_one_time_target_spreads_what_is_left_over_the_months_to_its_due_month() {
    let mut parts = january();
    parts.push(paycheck("2026-02-01", "5000.00"));
    parts.push(rent("2026-02-03"));
    let mut b = book(&refs(&parts));
    let v = b.budget(m("2026-02"), None);
    eq(cat(&v, "CarFund").carried_in, 1000);
    eq(cat(&v, "CarFund").target_allocated, 1000); // 5,000 left over five months
    eq(cat(&v, "Rent").target_allocated, 1400);
    eq(v.unassigned, 4800);
}

#[test]
fn the_last_month_takes_the_rounding_remainder_and_nothing_is_allocated_after_the_due_month() {
    let mut b = book(&[
        &paycheck("2026-01-01", "5000.00"),
        "2026-01-01 custom \"target\" Expenses:Savings:CarFund 1000.00 USD\n  due: 2026-03-15\n",
    ]);
    let allocated = |b: &mut Book, m: &str| {
        cat(&b.budget(m.parse().unwrap(), None), "CarFund").target_allocated
    };
    eq(allocated(&mut b, "2026-01"), "333.33");
    eq(allocated(&mut b, "2026-02"), "333.33");
    eq(allocated(&mut b, "2026-03"), "333.34");
    eq(allocated(&mut b, "2026-04"), 0);
    eq(
        cat(&b.budget(m("2026-04"), None), "CarFund").available,
        1000,
    );
}

#[test]
fn explicit_assignments_count_toward_the_target() {
    let mut parts = january();
    parts
        .push("2026-01-01 custom \"envelope-assign\" Expenses:Savings:CarFund 500.00 USD\n".into());
    let mut b = book(&refs(&parts));
    let v = b.budget(m("2026-01"), None);
    eq(cat(&v, "CarFund").assigned, 500);
    // The month's share is 6,000 over six months; the 500 assigned counts toward it.
    eq(cat(&v, "CarFund").target_allocated, 500);
    eq(cat(&v, "CarFund").available, 1000);
    eq(v.unassigned, "2200.00");
}

#[test]
fn what_unassigned_cannot_cover_is_the_targets_shortfall() {
    // February: rent is paid but there is no paycheck.
    let mut parts = january();
    parts.push(rent("2026-02-03"));
    let mut b = book(&refs(&parts));
    let v = b.budget(m("2026-02"), None);
    eq(cat(&v, "Rent").target_allocated, 1400);
    eq(cat(&v, "CarFund").target_allocated, 800);
    eq(cat(&v, "CarFund").shortfall, 200);
    eq(v.unassigned, 0);
    assert_eq!(v.shortfalls.len(), 1);
    assert_eq!(
        v.shortfalls[0].message,
        "Expenses:Savings:CarFund is 200.00 short of its share for the 2026-06 target"
    );
}

#[test]
fn a_shortfall_in_the_due_month_says_the_target_is_missed() {
    let mut b = book(&[
        &paycheck("2026-01-01", "100.00"),
        "2026-01-01 custom \"target\" Expenses:Savings:CarFund 500.00 USD\n  due: 2026-01-20\n",
    ]);
    let v = b.budget(m("2026-01"), None);
    eq(cat(&v, "CarFund").target_allocated, 100);
    assert_eq!(
        v.shortfalls[0].message,
        "Expenses:Savings:CarFund misses its 2026-01 target by 400.00"
    );
}

#[test]
fn monthly_reset_refills_while_carry_shortfall_also_catches_up() {
    let run = |rollover: &str| {
        let target = format!(
            "2026-01-01 custom \"target\" Expenses:Housing:Rent 1400.00 USD\n  rollover: \"{rollover}\"\n"
        );
        let mut b = book(&[
            &paycheck("2026-01-01", "1000.00"),
            &target,
            &paycheck("2026-02-01", "3000.00"),
        ]);
        let jan = b.budget(m("2026-01"), None);
        let feb = b.budget(m("2026-02"), None);
        (
            cat(&jan, "Rent").shortfall,
            cat(&feb, "Rent").target_allocated,
            cat(&feb, "Rent").available,
        )
    };
    // January funds 1,000 of 1,400 either way.
    let (short, feb, available) = run("reset");
    eq(short, 400);
    eq(feb, 400); // refill 1,000 up to 1,400
    eq(available, 1400);
    let (short, feb, available) = run("carry-shortfall");
    eq(short, 400);
    eq(feb, 1800); // 1,400 plus January's 400
    eq(available, 2800);
}

#[test]
fn a_later_target_entry_replaces_the_earlier_from_its_month_and_zero_ends_it() {
    let mut b = book(&[
        &paycheck("2026-01-01", "9000.00"),
        CAR_TARGET,
        // From March the car is due in December instead.
        "2026-03-10 custom \"target\" Expenses:Savings:CarFund 6000.00 USD\n  due: 2026-12-01\n",
        RENT_TARGET,
        "2026-04-01 custom \"target\" Expenses:Housing:Rent 0.00 USD\n",
    ]);
    eq(
        cat(&b.budget(m("2026-02"), None), "CarFund").target_allocated,
        1000,
    );
    // 4,000 left over March to December.
    eq(
        cat(&b.budget(m("2026-03"), None), "CarFund").target_allocated,
        400,
    );
    assert_eq!(
        cat(&b.budget(m("2026-03"), None), "CarFund")
            .target
            .as_ref()
            .unwrap()
            .since,
        d("2026-03-10")
    );
    let april = b.budget(m("2026-04"), None);
    assert!(cat(&april, "Rent").target.is_none());
    eq(cat(&april, "Rent").target_allocated, 0);
}

#[test]
fn unassigned_now_counts_target_allocations_up_to_todays_month_only() {
    let mut parts = january();
    parts.push(paycheck("2026-02-01", "5000.00"));
    let mut b = book(&refs(&parts));
    // Seen from January, February's paycheck and allocations are in the future.
    eq(
        b.budget(m("2026-02"), Some(d("2026-01-15"))).unassigned,
        2200,
    );
    // February: 7,200 before funding, less 1,400 for Rent and 1,000 for CarFund.
    eq(
        b.budget(m("2026-02"), Some(d("2026-02-15"))).unassigned,
        4800,
    );
}

#[test]
fn invalid_targets_are_flagged_and_change_nothing() {
    let cases = [
        (
            "2026-01-01 custom \"target\" Income:Salary 10.00 USD\n",
            "Income:Salary is not a budgeted category or liability",
        ),
        (
            "2026-01-01 custom \"target\" Expenses:Housing:Rent 10.00 EUR\n",
            "target amount must be in USD",
        ),
        (
            "2026-01-01 custom \"target\" Expenses:Housing:Rent -10.00 USD\n",
            "target amount can't be negative",
        ),
        (
            "2026-01-01 custom \"target\" Expenses:Housing:Rent\n",
            "target takes a category and an amount",
        ),
        (
            "2026-01-01 custom \"target\" Expenses:Housing:Rent 10.00 USD\n  due: \"June\"\n",
            "target due must be a date",
        ),
        (
            "2026-01-01 custom \"target\" Expenses:Housing:Rent 10.00 USD\n  recurrence: \"once\"\n",
            "a one-time target needs a due date",
        ),
        (
            "2026-01-01 custom \"target\" Expenses:Housing:Rent 10.00 USD\n  due: 2026-06-01\n  recurrence: \"monthly\"\n",
            "a monthly target takes no due date",
        ),
        (
            "2026-01-01 custom \"target\" Expenses:Housing:Rent 10.00 USD\n  due: 2026-06-01\n  rollover: \"reset\"\n",
            "rollover \"reset\" needs a monthly target",
        ),
        (
            "2026-01-01 custom \"target\" Expenses:Housing:Rent 10.00 USD\n  rollover: \"never\"\n",
            "target rollover must be \"carry-shortfall\" or \"reset\"",
        ),
    ];
    for (entry, message) in cases {
        let mut b = book(&[&paycheck("2026-01-01", "100.00"), entry]);
        let d = b.budget_diagnostics();
        assert_eq!(
            d.iter()
                .map(|d| (d.code.as_str(), d.message.as_str()))
                .collect::<Vec<_>>(),
            [("envelope", message)],
            "{entry}"
        );
        eq(b.budget(m("2026-01"), None).unassigned, 100);
    }
}

#[test]
fn ledgers_without_targets_are_unchanged() {
    // Every row keeps zero target fields, and Example 5's state is as before.
    let v = book_of("").budget(m("2026-11"), None);
    assert!(
        v.categories
            .iter()
            .all(|c| c.target.is_none() && c.target_allocated.is_zero() && c.shortfall.is_zero())
    );
    eq(v.unassigned, 1080);
}

#[test]
fn funding_counts_only_money_on_hand_so_a_future_assignment_can_overcommit() {
    // January: 1,000 on hand funds 1,000 of Rent's 1,400. The 300 assigned to February is not
    // counted when January is funded (a month depends only on itself and earlier months), so
    // Unassigned now shows the over-commitment.
    let mut b = book(&[
        &paycheck("2026-01-01", "1000.00"),
        RENT_TARGET,
        "2026-02-01 custom \"envelope-assign\" Expenses:Food:Groceries 300.00 USD\n",
    ]);
    let v = b.budget(m("2026-01"), Some(d("2026-01-15")));
    eq(cat(&v, "Rent").target_allocated, 1000);
    eq(cat(&v, "Rent").shortfall, 400);
    eq(v.assigned_to_future, 300);
    eq(v.unassigned, -300);
}

#[test]
fn the_target_fixture_and_its_scenario() {
    let mut trunk = fixture("targets/targets.bean");
    assert_eq!(trunk.errors(), vec![]);
    assert_eq!(trunk.budget_diagnostics(), vec![]);
    let month = |b: &mut Book, m: &str| b.budget(m.parse().unwrap(), None);
    let jan = month(&mut trunk, "2026-01");
    eq(cat(&jan, "Rent").target_allocated, 1400);
    eq(cat(&jan, "CarFund").target_allocated, 1000);
    eq(cat(&jan, "Groceries").available, "314.80");
    eq(jan.unassigned, 2200);
    let feb = month(&mut trunk, "2026-02");
    eq(cat(&feb, "CarFund").shortfall, 200);
    eq(feb.unassigned, 0);
    let mar = month(&mut trunk, "2026-03");
    eq(cat(&mar, "CarFund").target_allocated, 1050); // 4,200 left over March to June
    eq(mar.unassigned, 2550);
    let apr = month(&mut trunk, "2026-04");
    assert!(cat(&apr, "Rent").target.is_none());
    eq(cat(&apr, "Rent").available, 1400);

    // The scenario changes nothing before March, then spreads CarFund to December.
    let mut branch = fixture("targets/car-later.bean");
    assert_eq!(branch.errors(), vec![]);
    let d = branchbooks_core::diff::diff_books(&mut trunk, &mut branch, m("2026-01"), m("2026-04"));
    assert!(d.categories.iter().all(|c| c.month >= m("2026-03")));
    assert_eq!(d.rules.len(), 1);
    assert_eq!(
        (d.rules[0].from, d.rules[0].to),
        (m("2026-03"), m("2026-04"))
    );
    let mar = month(&mut branch, "2026-03");
    eq(cat(&mar, "CarFund").target_allocated, 420); // 4,200 over March to December
    eq(mar.unassigned, 3180);
}

/// A car loan on budget with 10,000 owed, a 300.00 monthly payment target, and its interest in a
/// category with its own 25.00 target (the loan example in docs/BUDGET-RULES.md).
fn loan(paycheck_amount: &str) -> Vec<String> {
    vec![
        "2026-01-01 open Liabilities:Loans:Car USD\n  budget: \"on\"\n".into(),
        "2026-01-01 open Liabilities:Loans:Old USD\n  budget: \"off\"\n".into(),
        "2026-01-01 open Expenses:Loan-Interest USD\n".into(),
        "2026-01-01 open Equity:Opening-Balances USD\n".into(),
        "2026-11-01 * \"Car loan\"\n  Liabilities:Loans:Car  -10000.00 USD\n  Equity:Opening-Balances\n".into(),
        paycheck("2026-11-01", paycheck_amount),
        "2026-11-01 custom \"target\" Liabilities:Loans:Car 300.00 USD\n".into(),
        "2026-11-01 custom \"target\" Expenses:Loan-Interest 25.00 USD\n".into(),
    ]
}

const LOAN_PAYMENT: &str = "2026-11-25 * \"Car loan payment\"\n  Liabilities:Loans:Car  300.00 USD\n  Expenses:Loan-Interest  25.00 USD\n  Assets:Checking\n";

#[test]
fn a_liability_target_sets_its_monthly_payment_aside() {
    let mut parts = loan("1000.00");
    parts.push(LOAN_PAYMENT.into());
    let mut b = book(&refs(&parts));
    assert_eq!(b.errors(), vec![]);
    let v = b.budget(m("2026-11"), Some(d("2026-11-30")));
    let car = liability_named(&v, "Liabilities:Loans:Car");
    assert_eq!(car.target.as_ref().map(|t| t.amount), Some(dec("300")));
    eq(car.target_allocated, 300);
    eq(car.available, 0);
    eq(car.owed, 9700);
    eq(car.underfunded, 0); // the target's shortfall, not the 9,700 still owed
    eq(cat(&v, "Loan-Interest").available, 0);
    eq(v.unassigned, 675);
    assert!(v.shortfalls.is_empty());
    // December sets the next payment aside: last month's money is not this month's payment.
    let dec_view = b.budget(m("2026-12"), Some(d("2026-12-01")));
    eq(
        liability_named(&dec_view, "Liabilities:Loans:Car").target_allocated,
        300,
    );
    eq(
        liability_named(&dec_view, "Liabilities:Loans:Car").available,
        300,
    );
    // Viewed after a later month, from its snapshot, November is the same.
    assert_eq!(b.budget(m("2026-11"), Some(d("2026-11-30"))), v);
}

#[test]
fn a_loan_switched_on_budget_keeps_its_earlier_months() {
    let mut b = book(&[
        "2026-01-01 open Liabilities:Mortgage USD\n  budget: \"on\"\n  budget-from: 2026-11-01\n",
        "2026-01-01 open Equity:Opening-Balances USD\n",
        "2026-01-01 * \"Mortgage\"\n  Liabilities:Mortgage  -10000.00 USD\n  Equity:Opening-Balances\n",
        &paycheck("2026-10-01", "1000.00"),
        // Before the switch: an off-budget transfer, charged to its category: line (rule 8).
        "2026-10-15 * \"Mortgage\"\n  category: \"Expenses:Housing:Rent\"\n  Liabilities:Mortgage  300.00 USD\n  Assets:Checking\n",
        "2026-11-01 custom \"target\" Liabilities:Mortgage 300.00 USD\n",
        "2026-11-15 * \"Mortgage\"\n  Liabilities:Mortgage  300.00 USD\n  Assets:Checking\n",
    ]);
    assert_eq!(b.errors(), vec![]);
    let oct = b.budget(m("2026-10"), Some(d("2026-10-31")));
    eq(cat(&oct, "Rent").activity, -300);
    assert!(oct.liabilities.is_empty());
    let nov = b.budget(m("2026-11"), Some(d("2026-11-30")));
    let loan = liability_named(&nov, "Liabilities:Mortgage");
    eq(loan.owed, 9400); // what it owed at the switch counts, as debt from before the budget
    eq(loan.target_allocated, 300);
    eq(loan.available, 0);
    eq(loan.underfunded, 0);
    eq(nov.unassigned, 400);
}

#[test]
fn a_liability_target_unassigned_cannot_fund_is_its_underfunded() {
    let mut b = book(&refs(&loan("100.00")));
    let v = b.budget(m("2026-11"), Some(d("2026-11-30")));
    // By due month, then name: Expenses:Loan-Interest takes 25, the loan the other 75.
    let car = liability_named(&v, "Liabilities:Loans:Car");
    eq(car.target_allocated, 75);
    eq(car.underfunded, 225);
    eq(v.unassigned, 0);
    assert_eq!(
        v.shortfalls
            .iter()
            .map(|s| s.category.as_str())
            .collect::<Vec<_>>(),
        ["Liabilities:Loans:Car"]
    );
}

#[test]
fn ending_a_liability_target_pays_it_in_full_again() {
    let mut parts = loan("1000.00");
    parts.push(LOAN_PAYMENT.into());
    parts.push("2026-12-01 custom \"target\" Liabilities:Loans:Car 0.00 USD\n".into());
    let mut b = book(&refs(&parts));
    let v = b.budget(m("2026-12"), Some(d("2026-12-01")));
    let car = liability_named(&v, "Liabilities:Loans:Car");
    assert!(car.target.is_none());
    eq(car.available, 0);
    eq(car.underfunded, 9700);
}

#[test]
fn a_target_on_an_off_budget_liability_or_with_a_due_date_is_flagged() {
    let cases = [
        (
            "2026-11-01 custom \"target\" Liabilities:Loans:Old 300.00 USD\n",
            "Liabilities:Loans:Old is not a budgeted category or liability",
        ),
        (
            "2026-11-01 custom \"target\" Liabilities:Loans:Car 300.00 USD\n  due: 2027-06-01\n",
            "a liability's target is monthly: it takes no due date",
        ),
    ];
    for (entry, message) in cases {
        let mut parts = loan("1000.00");
        parts.retain(|p| !p.contains("custom \"target\" Liabilities"));
        parts.push(entry.into());
        let mut b = book(&refs(&parts));
        let d = b.budget_diagnostics();
        assert!(
            d.iter()
                .any(|d| d.code == "envelope" && d.message == message),
            "{entry}: {d:?}"
        );
        eq(b.budget(m("2026-11"), None).unassigned, 975);
    }
}
