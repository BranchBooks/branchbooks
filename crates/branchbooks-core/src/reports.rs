//! Minimal reports needed by Examples 1 and 14: income statement and balance sheet in the budget
//! currency.

use indexmap::IndexMap;

use crate::date::Date;
use crate::money::{Decimal, ZERO};
use crate::types::Entry;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportLine {
    pub account: String,
    pub amount: Decimal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncomeStatement {
    pub income: Decimal,
    pub expenses: Decimal,
    pub net: Decimal,
    pub income_lines: Vec<ReportLine>,
    pub expense_lines: Vec<ReportLine>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BalanceSheet {
    pub assets: Decimal,
    /// Amount owed, shown positive.
    pub liabilities: Decimal,
    pub equity: Decimal,
    pub net_worth: Decimal,
    pub asset_lines: Vec<ReportLine>,
    pub liability_lines: Vec<ReportLine>,
}

fn totals_by_account<'a>(
    txns: impl IntoIterator<Item = &'a Entry>,
    currency: &str,
    from: Option<Date>,
    to: Option<Date>,
    roots: &[&str],
) -> IndexMap<String, Decimal> {
    let mut out: IndexMap<String, Decimal> = IndexMap::new();
    for t in txns {
        if from.is_some_and(|f| t.date < f) || to.is_some_and(|x| t.date > x) {
            continue;
        }
        for p in t.txn().map_or(&[][..], |t| &t.postings) {
            let Some(units) = p.units.as_ref().filter(|u| u.currency == currency) else {
                continue;
            };
            if !roots.iter().any(|r| {
                p.account
                    .strip_prefix(r)
                    .is_some_and(|rest| rest.starts_with(':'))
            }) {
                continue;
            }
            *out.entry(p.account.clone()).or_insert(ZERO) += units.number;
        }
    }
    out
}

fn lines(m: &IndexMap<String, Decimal>, root: &str, negate: bool) -> Vec<ReportLine> {
    let prefix = format!("{root}:");
    let mut out: Vec<ReportLine> = m
        .iter()
        .filter(|(a, _)| a.starts_with(&prefix))
        .map(|(account, v)| ReportLine {
            account: account.clone(),
            amount: if negate { -*v } else { *v },
        })
        .collect();
    out.sort_by(|a, b| a.account.cmp(&b.account));
    out
}

fn total(ls: &[ReportLine]) -> Decimal {
    ls.iter().map(|l| l.amount).sum()
}

/// Income and expenses between two dates inclusive; income is shown positive.
pub fn income_statement<'a>(
    txns: impl IntoIterator<Item = &'a Entry>,
    currency: &str,
    from: Option<Date>,
    to: Option<Date>,
) -> IncomeStatement {
    let m = totals_by_account(txns, currency, from, to, &["Income", "Expenses"]);
    let income_lines = lines(&m, "Income", true);
    let expense_lines = lines(&m, "Expenses", false);
    let income = total(&income_lines);
    let expenses = total(&expense_lines);
    IncomeStatement {
        income,
        expenses,
        net: income - expenses,
        income_lines,
        expense_lines,
    }
}

/// Balance sheet as of a date inclusive.
pub fn balance_sheet<'a>(
    txns: impl IntoIterator<Item = &'a Entry>,
    currency: &str,
    as_of: Option<Date>,
) -> BalanceSheet {
    let m = totals_by_account(
        txns,
        currency,
        None,
        as_of,
        &["Assets", "Liabilities", "Equity"],
    );
    let asset_lines = lines(&m, "Assets", false);
    let liability_lines = lines(&m, "Liabilities", true);
    let assets = total(&asset_lines);
    let liabilities = total(&liability_lines);
    BalanceSheet {
        assets,
        liabilities,
        equity: total(&lines(&m, "Equity", true)),
        net_worth: assets - liabilities,
        asset_lines,
        liability_lines,
    }
}
