//! Reports: net worth over the past year, income against expenses by
//! month, where the money went in the period, then the income statement and the balance sheet. The
//! month (the screen's) and the period (month or year to date) scope everything.
// shortcut: no "As of" a close or an earlier version yet; add it with month close.

use branchbooks_core::date::Month;
use branchbooks_core::reports::{ReportLine, balance_sheet, income_statement};
use dioxus::prelude::*;
use rust_decimal::Decimal;

use crate::charts::{BarList, ChartCard, ColumnChart, LineChart, Series};
use crate::journal::Shared;
use crate::text::{fmt, month_label, short, words};

/// Up to 12 months ending at `month`, not before `first` (the first transaction's month).
pub fn months_to(month: Month, first: Option<Month>) -> Vec<Month> {
    let mut out = vec![month];
    while out.len() < 12 && first.is_some_and(|f| out[0] > f) {
        out.insert(0, out[0].prev());
    }
    out
}

/// Spending by top-level category (Expenses:Insurance:Auto is Insurance), largest first; past
/// nine, the rest are Other.
pub fn spending(lines: &[ReportLine]) -> Vec<(String, Decimal)> {
    let mut groups: Vec<(String, Decimal)> = Vec::new();
    for l in lines {
        let g = words(l.account.split(':').nth(1).unwrap_or(&l.account));
        match groups.iter_mut().find(|(n, _)| *n == g) {
            Some(slot) => slot.1 += l.amount,
            None => groups.push((g, l.amount)),
        }
    }
    groups.retain(|(_, v)| *v > Decimal::ZERO);
    groups.sort_by_key(|g| std::cmp::Reverse(g.1));
    if groups.len() > 10 {
        let other: Decimal = groups[9..].iter().map(|(_, v)| *v).sum();
        groups.truncate(9);
        groups.push(("Other".to_owned(), other));
    }
    groups
}

#[component]
fn Lines(title: &'static str, lines: Vec<ReportLine>, total: Decimal) -> Element {
    rsx! {
        tbody {
            tr { class: "group-head", th { scope: "rowgroup", "{title}" } td { class: "num", {fmt(total)} } }
            for l in lines.into_iter().filter(|l| !l.amount.is_zero()) {
                tr { key: "{l.account}",
                    td { class: "indent", title: "{l.account}", {short(&l.account)} }
                    td { class: "num", {fmt(l.amount)} }
                }
            }
        }
    }
}

#[component]
pub fn Reports(core: Shared, version: Signal<Option<String>>, month: Month) -> Element {
    let mut year = use_signal(|| false);
    let _ = version();
    let mut c = core.0.borrow_mut();
    let book = c.book();
    let currency = book.currency().to_owned();
    let txns = book.transactions();
    let from = if year() {
        month.first_of_year()
    } else {
        month.first_day()
    };
    let to = month.last_day();
    let flow = income_statement(txns.iter().copied(), &currency, Some(from), Some(to));
    let sheet = balance_sheet(txns.iter().copied(), &currency, Some(to));
    let first = txns.first().map(|t| t.date.month());
    let months = months_to(month, first);
    let trend: Vec<(Month, Decimal, Decimal, Decimal)> = months
        .iter()
        .map(|&m| {
            let end = m.last_day();
            let s = income_statement(
                txns.iter().copied(),
                &currency,
                Some(m.first_day()),
                Some(end),
            );
            let worth = balance_sheet(txns.iter().copied(), &currency, Some(end)).net_worth;
            (m, worth, s.income, s.expenses)
        })
        .collect();
    drop(c);
    let period = if year() {
        format!("{:04} through {}", month.year(), month_label(month))
    } else {
        month_label(month)
    };
    let labels: Vec<String> = trend
        .iter()
        .map(|t| month_label(t.0)[..3].to_owned())
        .collect();
    let income = Series {
        name: "Income",
        color: "var(--series-1)",
        values: trend.iter().map(|t| t.2).collect(),
    };
    let expenses = Series {
        name: "Expenses",
        color: "var(--series-2)",
        values: trend.iter().map(|t| t.3).collect(),
    };
    let worth = Series {
        name: "Net worth",
        color: "var(--series-1)",
        values: trend.iter().map(|t| t.1).collect(),
    };
    let spent = spending(&flow.expense_lines);
    rsx! {
        div { class: "view-head",
            div { class: "segmented", role: "group", "aria-label": "Period",
                button { r#type: "button", "aria-pressed": !year(), onclick: move |_| year.set(false), "Month" }
                button { r#type: "button", "aria-pressed": year(), onclick: move |_| year.set(true), "Year to date" }
            }
        }
        div { class: "chart-grid",
            ChartCard { title: "Net worth", subtitle: format!("at the end of each month, to {}", month_label(month)),
                head: vec!["Month", "Net worth"],
                rows: trend.iter().map(|t| (month_label(t.0), vec![t.1])).collect::<Vec<_>>(),
                LineChart { labels: labels.clone(), series: worth }
            }
            ChartCard { title: "Income and expenses", subtitle: "by month", legend: vec![income.clone(), expenses.clone()],
                head: vec!["Month", "Income", "Expenses", "Net"],
                rows: trend.iter().map(|t| (month_label(t.0), vec![t.2, t.3, t.2 - t.3])).collect::<Vec<_>>(),
                ColumnChart { labels: labels.clone(), series: vec![income, expenses] }
            }
        }
        ChartCard { title: "Spending by category", subtitle: period.clone(),
            head: vec!["Category", "Spent"],
            rows: spent.iter().map(|(l, v)| (l.clone(), vec![*v])).collect::<Vec<_>>(),
            if spent.is_empty() {
                p { class: "muted", "No spending in this period." }
            } else {
                BarList { rows: spent.clone(), color: "var(--series-1)" }
            }
        }
        div { class: "report-grid",
            section { "aria-label": "Income statement",
                h2 { "Income statement " span { class: "muted small-inline", "{period}" } }
                table { class: "report",
                    Lines { title: "Income", lines: flow.income_lines.clone(), total: flow.income }
                    Lines { title: "Expenses", lines: flow.expense_lines.clone(), total: flow.expenses }
                    tfoot { tr { th { "Net" } td { class: if flow.net < Decimal::ZERO { "num neg" } else { "num" }, "data-testid": "net-income", {fmt(flow.net)} } } }
                }
            }
            section { "aria-label": "Balance sheet",
                h2 { "Balance sheet " span { class: "muted small-inline", "at {to}" } }
                table { class: "report",
                    Lines { title: "Assets", lines: sheet.asset_lines.clone(), total: sheet.assets }
                    Lines { title: "Liabilities", lines: sheet.liability_lines.clone(), total: sheet.liabilities }
                    tfoot { tr { th { "Net worth" } td { class: "num", "data-testid": "net-worth", {fmt(sheet.net_worth)} } } }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn months_and_spending_groups() {
        let m = |s: &str| s.parse::<Month>().unwrap();
        assert_eq!(
            months_to(m("2026-11"), Some(m("2026-10"))),
            [m("2026-10"), m("2026-11")]
        );
        assert_eq!(months_to(m("2026-11"), None), [m("2026-11")]);
        assert_eq!(months_to(m("2027-12"), Some(m("2020-01"))).len(), 12);
        let line = |a: &str, v: i64| ReportLine {
            account: a.into(),
            amount: Decimal::from(v),
        };
        let s = spending(&[
            line("Expenses:Food:Dining", 45),
            line("Expenses:Food:Groceries", 620),
            line("Expenses:Tax", 800),
        ]);
        assert_eq!(
            s,
            [
                ("Tax".to_owned(), Decimal::from(800)),
                ("Food".to_owned(), Decimal::from(665))
            ]
        );
    }
}
