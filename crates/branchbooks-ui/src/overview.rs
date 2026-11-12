//! The Overview, top to bottom: financial position (net worth, Unassigned and the accounts), what
//! needs attention, money in and out this month, recent activity, income against expenses, and how
//! assets and liabilities are distributed. Each part opens its view.

use branchbooks_core::date::{Date, Month};
use branchbooks_core::reports::{ReportLine, balance_sheet, income_statement};
use branchbooks_core::types::Entry;
use dioxus::prelude::*;
use rust_decimal::Decimal;

use crate::charts::{BarList, ChartCard, ColumnChart, Series};
use crate::journal::Shared;
use crate::reports::{months_to, spending};
use crate::text::{fmt, month_label, short};

/// How many accounts or sources a list on the Overview shows.
const TOP: usize = 5;

/// A source of money or a place it went this month, and its average over the months before.
#[derive(Clone, Debug, PartialEq)]
pub struct Mover {
    pub account: String,
    pub amount: Decimal,
    /// `None` when there are no earlier months to compare with.
    pub average: Option<Decimal>,
}

/// This month's largest lines, each with its average over `prior`, the months before this one.
pub fn movers(now: &[ReportLine], prior: &[Vec<ReportLine>]) -> Vec<Mover> {
    let months = Decimal::from(prior.len());
    let average = |account: &str| {
        (!prior.is_empty()).then(|| {
            let total: Decimal = prior
                .iter()
                .flatten()
                .filter(|l| l.account == account)
                .map(|l| l.amount)
                .sum();
            (total / months).round_dp(2)
        })
    };
    let mut out: Vec<Mover> = now
        .iter()
        .filter(|l| l.amount > Decimal::ZERO)
        .map(|l| Mover {
            account: l.account.clone(),
            amount: l.amount,
            average: average(&l.account),
        })
        .collect();
    out.sort_by_key(|m| std::cmp::Reverse(m.amount));
    out.truncate(TOP);
    out
}

/// Up to the three months before `month`, not before `first` (the first transaction's month).
pub fn prior_months(month: Month, first: Option<Month>) -> Vec<Month> {
    let mut out = Vec::new();
    let mut m = month.prev();
    while out.len() < 3 && first.is_some_and(|f| m >= f) {
        out.push(m);
        m = m.prev();
    }
    out
}

/// A balance sheet's lines by top-level group, largest first, as (group, amount, percent of the
/// total, to one decimal).
pub fn distribution(lines: &[ReportLine]) -> Vec<(String, Decimal, Decimal)> {
    let groups = spending(lines);
    let total: Decimal = groups.iter().map(|g| g.1).sum();
    groups
        .into_iter()
        .map(|(name, amount)| {
            let share = (amount * Decimal::ONE_HUNDRED / total).round_dp(1);
            (name, amount, share)
        })
        .collect()
}

/// The amount a transaction moved, as people read it: what left or reached the on-budget
/// accounts, else its largest posting.
pub fn headline(t: &Entry, on_budget: &dyn Fn(&str) -> bool) -> Decimal {
    let Some(txn) = t.txn() else {
        return Decimal::ZERO;
    };
    let amounts = || {
        txn.postings
            .iter()
            .filter_map(|p| p.units.as_ref().map(|u| (p, u.number)))
    };
    let own: Vec<Decimal> = amounts()
        .filter(|(p, _)| on_budget(&p.account))
        .map(|(_, n)| n)
        .collect();
    let net: Decimal = own.iter().sum();
    // Between on-budget accounts (paying the card from checking) the net is zero: show the size.
    if net.is_zero() {
        amounts().map(|(_, n)| n.abs()).max().unwrap_or_default()
    } else {
        net
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use branchbooks_core::book::Book;
    use branchbooks_core::loader::MemorySource;

    #[test]
    fn a_transaction_reads_as_what_moved_on_budget() {
        let ledger = "2026-01-01 open Assets:Checking USD\n  budget: \"on\"\n\
            2026-01-01 open Liabilities:Card USD\n  budget: \"on\"\n\
            2026-01-01 open Expenses:Food USD\n2026-01-01 open Assets:Brokerage USD\n  budget: \"off\"\n\
            2026-01-02 * \"Lunch\"\n  Expenses:Food  12.00 USD\n  Assets:Checking\n\
            2026-01-03 * \"Pay card\"\n  Liabilities:Card  45.00 USD\n  Assets:Checking\n";
        let mut book = Book::load(
            "main.beancount",
            &MemorySource::new([("main.beancount", ledger)]),
        );
        let info = book.account_info().clone();
        let on = |a: &str| {
            info.get(a)
                .is_some_and(|i| i.open.meta.str("budget") == Some("on"))
        };
        let t = book.transactions();
        assert_eq!(headline(t[0], &on), Decimal::from(-12));
        assert_eq!(headline(t[1], &on), Decimal::from(45));
    }

    fn line(account: &str, amount: i64) -> ReportLine {
        ReportLine {
            account: account.into(),
            amount: Decimal::from(amount),
        }
    }

    #[test]
    fn movers_are_the_largest_with_their_average_over_earlier_months() {
        let now = [
            line("Expenses:Rent", 900),
            line("Expenses:Food", 100),
            line("Expenses:Refund", -5),
        ];
        let prior = [
            vec![line("Expenses:Food", 50)],
            vec![line("Expenses:Food", 100)],
            vec![],
        ];
        let m = movers(&now, &prior);
        assert_eq!(m.len(), 2, "a net refund is not money spent");
        assert_eq!(m[0].account, "Expenses:Rent");
        assert_eq!(m[0].average, Some(Decimal::ZERO));
        assert_eq!(m[1].average, Some(Decimal::from(50)));
        assert_eq!(movers(&now, &[])[0].average, None);
        let many: Vec<ReportLine> = (1..=9)
            .map(|n| line(&format!("Expenses:A{n}"), n))
            .collect();
        assert_eq!(movers(&many, &[]).len(), TOP);
    }

    #[test]
    fn prior_months_stop_at_the_first_transaction() {
        let m = |s: &str| s.parse::<Month>().unwrap();
        assert_eq!(prior_months(m("2026-11"), None), []);
        assert_eq!(prior_months(m("2026-11"), Some(m("2026-11"))), []);
        assert_eq!(
            prior_months(m("2026-11"), Some(m("2026-10"))),
            [m("2026-10")]
        );
        assert_eq!(prior_months(m("2026-11"), Some(m("2020-01"))).len(), 3);
    }

    #[test]
    fn distribution_groups_lines_and_gives_their_share() {
        let d = distribution(&[
            line("Assets:Current-Assets:Checking", 100),
            line("Assets:Current-Assets:Savings", 200),
            line("Assets:Fixed-Assets:House", 100),
            line("Assets:Overdrawn", -50),
        ]);
        assert_eq!(
            d,
            [
                (
                    "Current Assets".to_owned(),
                    Decimal::from(300),
                    Decimal::from(75)
                ),
                (
                    "Fixed Assets".to_owned(),
                    Decimal::from(100),
                    Decimal::from(25)
                ),
            ]
        );
        assert_eq!(distribution(&[]), []);
    }
}

#[component]
pub fn Overview(
    core: Shared,
    version: Signal<Option<String>>,
    today: Date,
    /// Opens a view by its id ("budget", "journal", "reports", "files").
    onopen: EventHandler<&'static str>,
) -> Element {
    let _ = version();
    let month = today.month();
    let mut c = core.0.borrow_mut();
    let book = c.book();
    let currency = book.currency().to_owned();
    let errors = book.errors().len();
    let view = book.budget(month, Some(today));
    let info = book.account_info().clone();
    let on_budget = |a: &str| {
        info.get(a)
            .is_some_and(|i| i.open.meta.str("budget") == Some("on"))
    };
    let txns = book.transactions();
    let sheet = balance_sheet(txns.iter().copied(), &currency, Some(today));
    let flow = income_statement(
        txns.iter().copied(),
        &currency,
        Some(month.first_day()),
        Some(month.last_day()),
    );
    let of_month = |m: Month| {
        income_statement(
            txns.iter().copied(),
            &currency,
            Some(m.first_day()),
            Some(m.last_day()),
        )
    };
    let first = txns.first().map(|t| t.date.month());
    let earlier: Vec<_> = prior_months(month, first)
        .into_iter()
        .map(&of_month)
        .collect();
    let earlier_lines = |lines: fn(&_) -> &Vec<ReportLine>| -> Vec<Vec<ReportLine>> {
        earlier.iter().map(|s| lines(s).clone()).collect()
    };
    let money_in = movers(&flow.income_lines, &earlier_lines(|s| &s.income_lines));
    let money_out = movers(&flow.expense_lines, &earlier_lines(|s| &s.expense_lines));
    let trend: Vec<(Month, Decimal, Decimal)> = months_to(month, first)
        .into_iter()
        .map(|m| {
            let s = of_month(m);
            (m, s.income, s.expenses)
        })
        .collect();
    let last_month = balance_sheet(
        txns.iter().copied(),
        &currency,
        Some(month.prev().last_day()),
    )
    .net_worth;
    let recent: Vec<(Date, String, Decimal)> = txns
        .iter()
        .rev()
        .filter(|t| t.date <= today)
        .take(8)
        .map(|t| {
            let what = t
                .txn()
                .map(|x| x.payee.clone().unwrap_or_else(|| x.narration.clone()))
                .unwrap_or_default();
            (t.date, what, headline(t, &on_budget))
        })
        .collect();
    drop(c);
    let overspent: Vec<(String, Decimal)> = view
        .categories
        .iter()
        .filter(|r| r.cash_overspent > Decimal::ZERO || r.credit_overspent > Decimal::ZERO)
        .map(|r| (r.category.clone(), r.cash_overspent + r.credit_overspent))
        .collect();
    let underfunded: Vec<(String, Decimal)> = view
        .liabilities
        .iter()
        // One with a monthly payment shows its shortfall below, with the targets.
        .filter(|c| c.underfunded > Decimal::ZERO && c.target.is_none())
        .map(|c| (c.account.clone(), c.underfunded))
        .collect();
    let overfunded: Vec<(String, Decimal)> = view
        .liabilities
        .iter()
        .filter(|c| c.overfunded > Decimal::ZERO)
        .map(|c| (c.account.clone(), c.overfunded))
        .collect();
    let attention = errors > 0
        || !overspent.is_empty()
        || !underfunded.is_empty()
        || !overfunded.is_empty()
        || !view.shortfalls.is_empty();
    let change = sheet.net_worth - last_month;
    let sign = if change > Decimal::ZERO { "+" } else { "" };
    let mut accounts: Vec<(String, Decimal)> = sheet
        .asset_lines
        .iter()
        .map(|l| (l.account.clone(), l.amount))
        .chain(
            sheet
                .liability_lines
                .iter()
                .map(|l| (l.account.clone(), -l.amount)),
        )
        .filter(|(_, amount)| !amount.is_zero())
        .collect();
    accounts.sort_by_key(|(_, amount)| std::cmp::Reverse(amount.abs()));
    accounts.truncate(TOP);
    let labels: Vec<String> = trend
        .iter()
        .map(|t| month_label(t.0)[..3].to_owned())
        .collect();
    let income = Series {
        name: "Income",
        color: "var(--series-1)",
        values: trend.iter().map(|t| t.1).collect(),
    };
    let expenses = Series {
        name: "Expenses",
        color: "var(--series-2)",
        values: trend.iter().map(|t| t.2).collect(),
    };
    let assets = distribution(&sheet.asset_lines);
    let liabilities = distribution(&sheet.liability_lines);
    rsx! {
        div { class: "view-head", p { class: "muted", "As of {today}" } }
        h2 { class: "band", "Financial position" }
        div { class: "cards",
            button { r#type: "button", class: "stat", onclick: move |_| onopen.call("reports"),
                span { class: "label", "Net worth" }
                strong { "data-testid": "net-worth", {fmt(sheet.net_worth)} }
                span { class: "sub", "{sign}{fmt(change)} since last month · {fmt(sheet.assets)} assets · {fmt(sheet.liabilities)} owed" }
            }
            button { r#type: "button", class: if view.unassigned < Decimal::ZERO { "stat bad" } else { "stat good" },
                onclick: move |_| onopen.call("budget"),
                span { class: "label", "Unassigned" }
                strong { "data-testid": "unassigned", {fmt(view.unassigned)} }
                span { class: "sub", "{fmt(view.cash)} in on-budget accounts" }
            }
        }
        section { "aria-label": "Accounts",
            div { class: "section-head",
                h2 { "Accounts" }
                button { r#type: "button", class: "link", onclick: move |_| onopen.call("accounts"), "View all" }
            }
            if accounts.is_empty() {
                p { class: "muted", "No balances yet." }
            } else {
                table { class: "recent",
                    tbody {
                        for (account, amount) in accounts {
                            tr { key: "{account}",
                                td { title: "{account}", {short(&account)} }
                                td { class: if amount < Decimal::ZERO { "num neg" } else { "num" }, {fmt(amount)} }
                            }
                        }
                    }
                }
            }
        }
        if attention {
            section { class: "attention", "aria-label": "Needs attention",
                h2 { "Needs attention" }
                ul {
                    if errors > 0 {
                        li { button { r#type: "button", class: "link", onclick: move |_| onopen.call("files"), "{errors} ledger error(s)" } }
                    }
                    for (category, amount) in overspent {
                        li { key: "{category}",
                            button { r#type: "button", class: "link", onclick: move |_| onopen.call("budget"), {short(&category)} }
                            " is overspent by {fmt(amount)}"
                        }
                    }
                    for (account, amount) in underfunded {
                        li { key: "{account}",
                            button { r#type: "button", class: "link", onclick: move |_| onopen.call("budget"), {short(&account)} }
                            " still needs {fmt(amount)} set aside to pay it off"
                        }
                    }
                    for (account, amount) in overfunded {
                        li { key: "{account}-over",
                            button { r#type: "button", class: "link", onclick: move |_| onopen.call("budget"), {short(&account)} }
                            " has {fmt(amount)} more set aside than it owes"
                        }
                    }
                    for s in view.shortfalls.iter() {
                        li { key: "{s.category}", "{short(&s.category)}'s target is {fmt(s.amount)} short" }
                    }
                }
            }
        }
        h2 { class: "band", "Money movement " span { class: "muted small-inline", {month_label(month)} } }
        div { class: "report-grid",
            MoneyCard { title: "Money in", total: flow.income, movers: money_in, onall: move |()| onopen.call("reports") }
            MoneyCard { title: "Money out", total: flow.expenses, movers: money_out, onall: move |()| onopen.call("reports") }
        }
        h2 { class: "band", "Recent activity" }
        section { "aria-label": "Transactions",
            div { class: "section-head",
                h2 { "Transactions" }
                button { r#type: "button", class: "link", onclick: move |_| onopen.call("journal"), "View all" }
            }
            if recent.is_empty() {
                p { class: "muted", "No transactions yet." }
            } else {
                table { class: "recent",
                    tbody {
                        for (date, what, amount) in recent {
                            tr {
                                td { class: "date", "{date}" }
                                td { "{what}" }
                                td { class: if amount < Decimal::ZERO { "num neg" } else { "num pos" }, {fmt(amount)} }
                            }
                        }
                    }
                }
            }
        }
        h2 { class: "band", "Income vs Expenses" }
        ChartCard { title: "Income and expenses", subtitle: "by month", legend: vec![income.clone(), expenses.clone()],
            head: vec!["Period", "Income", "Expenses", "Net"],
            rows: trend.iter().map(|t| (month_label(t.0), vec![t.1, t.2, t.1 - t.2])).collect::<Vec<_>>(),
            ColumnChart { labels, series: vec![income, expenses] }
        }
        h2 { class: "band", "Balance sheet" }
        div { class: "chart-grid",
            Distribution { title: "Assets distribution", groups: assets, color: "var(--series-1)" }
            Distribution { title: "Liabilities distribution", groups: liabilities, color: "var(--series-2)" }
        }
    }
}

/// One side of the month's money: its total and largest lines, each against its recent average.
#[component]
fn MoneyCard(
    title: &'static str,
    total: Decimal,
    movers: Vec<Mover>,
    onall: EventHandler<()>,
) -> Element {
    rsx! {
        section { "aria-label": "{title}",
            div { class: "section-head",
                h2 { "{title} " span { class: "muted small-inline", {fmt(total)} } }
                button { r#type: "button", class: "link", onclick: move |_| onall.call(()), "View all" }
            }
            if movers.is_empty() {
                p { class: "muted", "Nothing this month." }
            } else {
                table { class: "recent",
                    thead { tr { th { "Account" } th { class: "num", "This month" } th { class: "num", "3-month average" } } }
                    tbody {
                        for m in movers {
                            tr { key: "{m.account}",
                                td { title: "{m.account}", {short(&m.account)} }
                                td { class: "num", {fmt(m.amount)} }
                                td { class: "num muted", {m.average.map_or_else(|| "–".to_owned(), fmt)} }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// How a side of the balance sheet divides between its groups, as bars or a table.
#[component]
fn Distribution(
    title: &'static str,
    groups: Vec<(String, Decimal, Decimal)>,
    color: &'static str,
) -> Element {
    let bars: Vec<(String, Decimal)> = groups.iter().map(|g| (g.0.clone(), g.1)).collect();
    rsx! {
        ChartCard { title, subtitle: "",
            head: vec!["Group", "Amount", "Share %"],
            rows: groups.iter().map(|g| (g.0.clone(), vec![g.1, g.2])).collect::<Vec<_>>(),
            if bars.is_empty() {
                p { class: "muted", "Nothing to show." }
            } else {
                BarList { rows: bars, color }
            }
        }
    }
}
