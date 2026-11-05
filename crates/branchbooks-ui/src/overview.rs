//! The Overview: where things stand at a glance (net worth, Unassigned, this month's money in and out), what needs attention, and the latest transactions. Each
//! card opens its view.

use branchbooks_core::date::Date;
use branchbooks_core::reports::{balance_sheet, income_statement};
use branchbooks_core::types::Entry;
use dioxus::prelude::*;
use rust_decimal::Decimal;

use crate::journal::Shared;
use crate::text::{fmt, month_label, short};

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
        .cards
        .iter()
        .filter(|c| c.underfunded > Decimal::ZERO)
        .map(|c| (c.card.clone(), c.underfunded))
        .collect();
    let attention = errors > 0
        || !overspent.is_empty()
        || !underfunded.is_empty()
        || !view.shortfalls.is_empty();
    let sign = if flow.net > Decimal::ZERO { "+" } else { "" };
    rsx! {
        div { class: "view-head", p { class: "muted", "As of {today}" } }
        div { class: "cards",
            button { r#type: "button", class: "stat", onclick: move |_| onopen.call("reports"),
                span { class: "label", "Net worth" }
                strong { "data-testid": "net-worth", {fmt(sheet.net_worth)} }
                span { class: "sub", "{fmt(sheet.assets)} assets · {fmt(sheet.liabilities)} owed" }
            }
            button { r#type: "button", class: if view.unassigned < Decimal::ZERO { "stat bad" } else { "stat good" },
                onclick: move |_| onopen.call("budget"),
                span { class: "label", "Unassigned" }
                strong { "data-testid": "unassigned", {fmt(view.unassigned)} }
                span { class: "sub", "{fmt(view.cash)} in on-budget accounts" }
            }
            button { r#type: "button", class: "stat", onclick: move |_| onopen.call("reports"),
                span { class: "label", {month_label(month)} }
                strong { class: if flow.net < Decimal::ZERO { "neg" } else { "" }, "{sign}{fmt(flow.net)}" }
                span { class: "sub", "{fmt(flow.income)} in · {fmt(flow.expenses)} out" }
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
                    for (card, amount) in underfunded {
                        li { key: "{card}",
                            button { r#type: "button", class: "link", onclick: move |_| onopen.call("budget"), {short(&card)} }
                            " payment is underfunded by {fmt(amount)}"
                        }
                    }
                    for s in view.shortfalls.iter() {
                        li { key: "{s.category}", "{short(&s.category)}'s target is {fmt(s.amount)} short" }
                    }
                }
            }
        }
        section { "aria-label": "Recent activity",
            div { class: "section-head",
                h2 { "Recent activity" }
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
    }
}
