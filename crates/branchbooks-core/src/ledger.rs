//! Ledger model: booking (fill one elided amount), per-transaction balance check, account
//! open/close validation, and balances per account. Errors mirror Beancount's so the oracle can
//! compare them by file, line and code.

use std::cmp::Ordering;
use std::collections::HashSet;

use indexmap::IndexMap;

use crate::date::Date;
use crate::loader::{LoadResult, Options};
use crate::money::{Decimal, ZERO, normalized_scale, plain};
use crate::types::{Amount, Diagnostic, Entry, EntryType, Posting};

/// An account's `open` entry and, once closed, its `close` entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountInfo {
    pub open: Entry,
    pub close: Option<Entry>,
}

impl AccountInfo {
    pub fn currencies(&self) -> &[String] {
        self.open.open().map_or(&[], |o| &o.currencies)
    }
}

pub type Accounts = IndexMap<String, AccountInfo>;

/// Balance per account, then per currency.
pub type Balances = IndexMap<String, IndexMap<String, Decimal>>;

#[derive(Clone, Debug)]
pub struct Ledger {
    /// Valid entries sorted by (date, type, file order); transactions are booked.
    pub entries: Vec<Entry>,
    pub accounts: Accounts,
    pub options: Options,
    pub errors: Vec<Diagnostic>,
}

impl Ledger {
    pub fn transactions(&self) -> impl Iterator<Item = &Entry> {
        self.entries
            .iter()
            .filter(|e| e.entry_type() == EntryType::Txn)
    }

    pub fn customs(&self) -> impl Iterator<Item = &Entry> {
        self.entries
            .iter()
            .filter(|e| e.entry_type() == EntryType::Custom)
    }
}

/// Sort rank of an entry type on one date: opens first, then balance assertions (so they see the
/// day before), closes last.
pub fn type_rank(t: EntryType) -> i8 {
    match t {
        EntryType::Open => -2,
        EntryType::Balance => -1,
        EntryType::Close => 2,
        _ => 0,
    }
}

pub fn compare_entries(a: &Entry, b: &Entry) -> Ordering {
    a.date
        .cmp(&b.date)
        .then_with(|| type_rank(a.entry_type()).cmp(&type_rank(b.entry_type())))
        .then_with(|| a.seq.cmp(&b.seq))
}

fn err(e: &Entry, code: &str, message: String) -> Diagnostic {
    Diagnostic::new(&e.file, e.line, code, message)
}

/// The outcome of booking one transaction.
pub struct Booked {
    /// `None` when booking fails (Beancount drops those entries).
    pub txn: Option<Entry>,
    pub errors: Vec<Diagnostic>,
}

/// Fills a single elided posting amount and checks the transaction balances within the
/// tolerance Beancount infers: half a unit of the coarsest fractional precision per currency.
pub fn book_transaction(entry: &Entry) -> Booked {
    let txn = entry.txn().expect("book_transaction takes a transaction");
    let mut errors = Vec::new();
    let missing: Vec<usize> = (0..txn.postings.len())
        .filter(|&i| txn.postings[i].units.is_none())
        .collect();
    let mut residual: IndexMap<&str, Decimal> = IndexMap::new();
    let mut tolerance: IndexMap<&str, Decimal> = IndexMap::new();
    for p in &txn.postings {
        let Some(Amount {
            number,
            currency,
            scale,
        }) = &p.units
        else {
            continue;
        };
        *residual.entry(currency).or_insert(ZERO) += *number;
        if *scale > 0 {
            // 5 × 10^-(scale+1). Past 28 digits no residual can be that small, so zero is exact.
            let tol = Decimal::try_from_i128_with_scale(5, scale + 1).unwrap_or(ZERO);
            let prev = tolerance.entry(currency).or_insert(tol);
            if tol > *prev {
                *prev = tol;
            }
        }
    }

    if missing.len() > 1 {
        // Beancount reports this on the second auto-posting's line and drops the transaction.
        let line = txn.postings[missing[1]].line;
        errors.push(Diagnostic::new(
            &entry.file,
            line,
            "booking",
            "You may not have more than one auto-posting per currency",
        ));
        return Booked { txn: None, errors };
    }

    let mut filled: Option<Vec<Posting>> = None;
    if let [hole] = missing[..] {
        let fills: Vec<(&str, Decimal)> = residual
            .iter()
            .filter(|(_, v)| !v.is_zero())
            .map(|(c, v)| (*c, *v))
            .collect();
        let mut postings = Vec::with_capacity(txn.postings.len() + fills.len());
        for (i, p) in txn.postings.iter().enumerate() {
            if i != hole {
                postings.push(p.clone());
                continue;
            }
            for (currency, v) in &fills {
                let number = -*v;
                let units = Amount {
                    number,
                    currency: (*currency).to_owned(),
                    scale: normalized_scale(number),
                };
                postings.push(Posting {
                    units: Some(units),
                    ..p.clone()
                });
            }
        }
        filled = Some(postings);
        residual.clear();
    }

    for (currency, v) in &residual {
        let tol = tolerance.get(currency).copied().unwrap_or(ZERO);
        if v.abs() > tol {
            errors.push(err(
                entry,
                "unbalanced",
                format!("Transaction does not balance: ({} {currency})", plain(*v)),
            ));
        }
    }
    let mut booked = entry.clone();
    if let Some(postings) = filled {
        booked.txn_mut().expect("a transaction").postings = postings;
    }
    Booked {
        txn: Some(booked),
        errors,
    }
}

/// Accounts an entry refers to, for open/close validation. Beancount 3.2.3 does not validate
/// account values inside custom directives, so neither do we; the budget flags them instead.
pub fn entry_accounts(e: &Entry) -> impl Iterator<Item = &str> {
    let postings = e.txn().into_iter().flat_map(|t| t.postings.iter());
    postings
        .map(|p| p.account.as_str())
        .chain(e.balance().map(|b| b.account.as_str()))
}

pub fn check_account_refs(e: &Entry, accounts: &Accounts) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for name in entry_accounts(e) {
        if !seen.insert(name) {
            continue;
        }
        match accounts.get(name) {
            None => out.push(err(
                e,
                "unknown-account",
                format!("Invalid reference to unknown account '{name}'"),
            )),
            Some(info) => {
                if e.date < info.open.date || info.close.as_ref().is_some_and(|c| e.date > c.date) {
                    out.push(err(
                        e,
                        "inactive-account",
                        format!("Invalid reference to inactive account '{name}'"),
                    ));
                }
            }
        }
    }
    if let Some(t) = e.txn() {
        for p in &t.postings {
            let (Some(info), Some(units)) = (accounts.get(&p.account), &p.units) else {
                continue;
            };
            let allowed = info.currencies();
            if !allowed.is_empty() && !allowed.contains(&units.currency) {
                out.push(err(
                    e,
                    "currency-constraint",
                    format!(
                        "Invalid currency {} for account '{}'",
                        units.currency, p.account
                    ),
                ));
            }
        }
    }
    out
}

/// Accounts from the `open` and `close` entries, in date order (`sorted` is sorted).
pub fn collect_accounts<'a>(
    sorted: impl IntoIterator<Item = &'a Entry>,
    errors: &mut Vec<Diagnostic>,
) -> Accounts {
    let mut accounts = Accounts::new();
    let mut closes = Vec::new();
    for e in sorted {
        if let Some(o) = e.open() {
            if accounts.contains_key(&o.account) {
                errors.push(err(
                    e,
                    "duplicate-open",
                    format!("Duplicate open directive for {}", o.account),
                ));
            } else {
                accounts.insert(
                    o.account.clone(),
                    AccountInfo {
                        open: e.clone(),
                        close: None,
                    },
                );
            }
        } else if e.close().is_some() {
            closes.push(e);
        }
    }
    for c in closes {
        let account = &c.close().expect("a close").account;
        match accounts.get_mut(account) {
            None => errors.push(err(
                c,
                "close-unopened",
                format!("Unopened account {account} is being closed"),
            )),
            Some(info) if info.close.is_some() => {
                errors.push(err(
                    c,
                    "duplicate-close",
                    format!("Duplicate close directive for {account}"),
                ));
            }
            Some(info) => info.close = Some(c.clone()),
        }
    }
    accounts
}

/// The full load: sorts, collects accounts, books every transaction and checks references.
pub fn build_ledger(loaded: &LoadResult) -> Ledger {
    let mut errors = loaded.errors.clone();
    let mut sorted: Vec<&Entry> = loaded.entries.iter().collect();
    sorted.sort_by(|a, b| compare_entries(a, b));
    let accounts = collect_accounts(sorted.iter().copied(), &mut errors);
    let mut entries = Vec::with_capacity(sorted.len());
    for e in sorted {
        match e.entry_type() {
            EntryType::Txn => {
                let booked = book_transaction(e);
                errors.extend(booked.errors);
                let Some(txn) = booked.txn else { continue };
                errors.extend(check_account_refs(&txn, &accounts));
                entries.push(txn);
            }
            t => {
                if matches!(t, EntryType::Custom | EntryType::Balance) {
                    errors.extend(check_account_refs(e, &accounts));
                }
                entries.push(e.clone());
            }
        }
    }
    let txns: Vec<&Entry> = entries.iter().filter(|e| e.txn().is_some()).collect();
    let assertions: Vec<&Entry> = entries.iter().filter(|e| e.balance().is_some()).collect();
    errors.extend(check_balances(&txns, &assertions, &accounts));
    Ledger {
        entries,
        accounts,
        options: loaded.options.clone(),
        errors: sort_diagnostics(errors),
    }
}

/// Beancount's balance check. Each assertion compares its account and the account's sub-accounts,
/// in its currency, with the transactions dated before it, within its tolerance or else one unit
/// of the amount's last decimal place (none for a whole number). `txns` are booked, in date order.
pub fn check_balances<'a>(
    txns: &[&'a Entry],
    assertions: &[&'a Entry],
    accounts: &Accounts,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut sorted = assertions.to_vec();
    sorted.sort_by(|a, b| compare_entries(a, b));
    let asserted: HashSet<&str> = sorted
        .iter()
        .filter_map(|e| Some(e.balance()?.account.as_str()))
        .collect();
    // Only postings to an asserted account or one below it are summed.
    let tracked = |account: &str| {
        asserted.contains(account)
            || account
                .match_indices(':')
                .any(|(i, _)| asserted.contains(&account[..i]))
    };
    let mut running: IndexMap<(&str, &str), Decimal> = IndexMap::new();
    let mut next = txns.iter().peekable();
    for e in sorted {
        let Some(b) = e.balance() else { continue };
        while let Some(t) = next.next_if(|t| t.date < e.date) {
            for p in t.txn().map_or(&[][..], |t| &t.postings) {
                if let Some(u) = &p.units
                    && tracked(&p.account)
                {
                    let key = (p.account.as_str(), u.currency.as_str());
                    *running.entry(key).or_insert(ZERO) += u.number;
                }
            }
        }
        let Some(info) = accounts.get(&b.account) else {
            let message = format!("Invalid reference to unknown account '{}'", b.account);
            out.push(err(e, "unknown-account", message));
            continue;
        };
        let expected = &b.amount;
        let allowed = info.currencies();
        if !allowed.is_empty() && !allowed.contains(&expected.currency) {
            let message = format!(
                "Invalid currency '{}' for Balance directive:",
                expected.currency
            );
            out.push(err(e, "currency-constraint", message));
        }
        let child = format!("{}:", b.account);
        let actual: Decimal = running
            .iter()
            .filter(|((a, c), _)| {
                *c == expected.currency && (*a == b.account || a.starts_with(&child))
            })
            .map(|(_, v)| *v)
            .sum();
        let tolerance = match (b.tolerance, expected.scale) {
            (Some(t), _) => t,
            (None, 0) => ZERO,
            (None, scale) => Decimal::new(1, scale),
        };
        let diff = actual - expected.number;
        if diff.abs() > tolerance {
            let how = if diff > ZERO {
                "too much"
            } else {
                "too little"
            };
            let cur = &expected.currency;
            let message = format!(
                "Balance failed for '{}': expected {} {cur} != accumulated {actual} {cur} ({} {how})",
                b.account,
                expected.number,
                diff.abs()
            );
            out.push(err(e, "balance", message));
        }
    }
    out
}

pub fn compare_diagnostics(a: &Diagnostic, b: &Diagnostic) -> Ordering {
    a.file
        .cmp(&b.file)
        .then(a.line.cmp(&b.line))
        .then_with(|| a.code.cmp(&b.code))
        .then_with(|| a.message.cmp(&b.message))
}

pub fn sort_diagnostics(mut d: Vec<Diagnostic>) -> Vec<Diagnostic> {
    d.sort_by(compare_diagnostics);
    d
}

/// Sums postings per account and currency, optionally only up to and including a date.
pub fn compute_balances<'a>(
    transactions: impl IntoIterator<Item = &'a Entry>,
    until: Option<Date>,
) -> Balances {
    let mut out = Balances::new();
    for t in transactions {
        if until.is_some_and(|u| t.date > u) {
            continue;
        }
        if let Some(txn) = t.txn() {
            add_postings(&mut out, &txn.postings, false);
        }
    }
    out
}

/// Adds postings to `out`, or subtracts them when `negate` is set.
pub fn add_postings(out: &mut Balances, postings: &[Posting], negate: bool) {
    for p in postings {
        let Some(units) = &p.units else { continue };
        let by_cur = out.entry(p.account.clone()).or_default();
        let slot = by_cur.entry(units.currency.clone()).or_insert(ZERO);
        if negate {
            *slot -= units.number;
        } else {
            *slot += units.number;
        }
    }
}

pub fn balance_of(b: &Balances, account: &str, currency: &str) -> Decimal {
    b.get(account)
        .and_then(|m| m.get(currency))
        .copied()
        .unwrap_or(ZERO)
}
