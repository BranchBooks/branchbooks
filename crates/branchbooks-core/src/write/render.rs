//! Renders new entries as Beancount text in one canonical style: two-space indent, amounts
//! right-aligned after the longest account, a blank line between multi-line entries.
//!
//! Checks run in a fixed order, so the first problem with an input is the one reported.

use std::fmt;

use crate::budget::{Recurrence, Rollover};
use crate::date::{Date, DateError};
use crate::money::{fixed, parse_decimal, plain};
use crate::types::MetaValue;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BudgetFlag {
    On,
    Off,
    Excluded,
    /// Only groups its sub-accounts: it holds no transactions and is no envelope.
    Placeholder,
}

impl BudgetFlag {
    pub fn as_str(self) -> &'static str {
        match self {
            BudgetFlag::On => "on",
            BudgetFlag::Off => "off",
            BudgetFlag::Excluded => "excluded",
            BudgetFlag::Placeholder => "placeholder",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "on" => Some(BudgetFlag::On),
            "off" => Some(BudgetFlag::Off),
            "excluded" => Some(BudgetFlag::Excluded),
            "placeholder" => Some(BudgetFlag::Placeholder),
            _ => None,
        }
    }
}

/// A metadata value to write. `Text` is written quoted. A parsed `Value` keeps its type, so an
/// edited entry writes its dates, accounts, numbers and amounts back the way they were.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MetaInputValue {
    Text(String),
    Value(MetaValue),
}

/// Metadata to write, in order.
pub type MetaInput = Vec<(String, MetaInputValue)>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PostingInput {
    pub account: String,
    /// Decimal text such as "-45.00". `None` lets Beancount fill in the balancing amount.
    pub amount: Option<String>,
    pub currency: Option<String>,
    pub meta: MetaInput,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TransactionInput {
    pub date: String,
    /// `*` or `!`; `*` when `None`.
    pub flag: Option<String>,
    pub payee: Option<String>,
    pub narration: String,
    pub tags: Vec<String>,
    pub links: Vec<String>,
    pub meta: MetaInput,
    pub postings: Vec<PostingInput>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenInput {
    pub date: String,
    pub account: String,
    pub currencies: Vec<String>,
    pub budget: Option<BudgetFlag>,
    /// Booking method ("FIFO" and so on), kept when an open is edited.
    pub booking: Option<String>,
    /// Other metadata, kept when an open is edited; `budget` is set by its own field, not here.
    pub meta: MetaInput,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CloseInput {
    pub date: String,
    pub account: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AssignInput {
    pub date: String,
    pub category: String,
    pub amount: String,
    pub currency: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MoveInput {
    pub date: String,
    pub from: String,
    pub to: String,
    pub amount: String,
    pub currency: Option<String>,
}

/// A target rule for a category (T1 to T7). An amount of zero ends the category's target.
/// `recurrence` and `rollover` are written only when given; the engine fills in the defaults.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TargetInput {
    pub date: String,
    pub category: String,
    pub amount: String,
    pub currency: Option<String>,
    /// The due date of a one-time target.
    pub due: Option<String>,
    pub recurrence: Option<Recurrence>,
    pub rollover: Option<Rollover>,
}

/// A `balance` assertion (Reconcile, R3).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BalanceInput {
    pub date: String,
    pub account: String,
    pub amount: String,
    pub currency: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryInput {
    Txn(TransactionInput),
    Open(OpenInput),
    Close(CloseInput),
    Assign(AssignInput),
    Move(MoveInput),
    Target(TargetInput),
    Balance(BalanceInput),
}

impl EntryInput {
    pub fn date(&self) -> &str {
        match self {
            EntryInput::Txn(e) => &e.date,
            EntryInput::Open(e) => &e.date,
            EntryInput::Close(e) => &e.date,
            EntryInput::Assign(e) => &e.date,
            EntryInput::Move(e) => &e.date,
            EntryInput::Target(e) => &e.date,
            EntryInput::Balance(e) => &e.date,
        }
    }
}

/// An input the write path refuses, with the reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputError(pub String);

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InputError {}

type Result<T> = std::result::Result<T, InputError>;

fn check(ok: bool, message: impl FnOnce() -> String) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(InputError(message()))
    }
}

const ROOTS: [&str; 5] = ["Assets", "Liabilities", "Equity", "Income", "Expenses"];

/// `^(Assets|Liabilities|Equity|Income|Expenses)(:[A-Z0-9][A-Za-z0-9-]*)+$`
pub fn is_account(a: &str) -> bool {
    let mut parts = a.split(':');
    let root = parts.next().unwrap_or("");
    let mut rest = parts.peekable();
    ROOTS.contains(&root)
        && rest.peek().is_some()
        && rest.all(|p| {
            let b = p.as_bytes();
            !b.is_empty()
                && (b[0].is_ascii_uppercase() || b[0].is_ascii_digit())
                && b[1..]
                    .iter()
                    .all(|c| c.is_ascii_alphanumeric() || *c == b'-')
        })
}

/// `^[A-Z][A-Z0-9'._-]{0,22}[A-Z0-9]$|^[A-Z]$`
pub fn is_currency(c: &str) -> bool {
    let b = c.as_bytes();
    let middle = |x: &u8| x.is_ascii_uppercase() || x.is_ascii_digit() || b"'._-".contains(x);
    match b.len() {
        1 => b[0].is_ascii_uppercase(),
        2..=24 => {
            b[0].is_ascii_uppercase()
                && b[1..b.len() - 1].iter().all(middle)
                && (b[b.len() - 1].is_ascii_uppercase() || b[b.len() - 1].is_ascii_digit())
        }
        _ => false,
    }
}

/// `^[a-z][a-zA-Z0-9_-]*$`
fn is_key(k: &str) -> bool {
    let b = k.as_bytes();
    !b.is_empty()
        && b[0].is_ascii_lowercase()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(c))
}

/// `^[A-Za-z0-9_/.-]+$`
fn is_tag(t: &str) -> bool {
    !t.is_empty()
        && t.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_/.-".contains(&c))
}

/// A Beancount string on one line: a line break in the text is written `\n`, which the parser reads
/// back, because every entry's lines must stay lines.
pub fn quote(s: &str) -> String {
    let text = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace("\r\n", "\\n")
        .replace(['\n', '\r'], "\\n");
    format!("\"{text}\"")
}

/// Validates decimal text and returns it normalised ("1,000.5" -> "1000.5", keeps trailing
/// zeros). Like the parser, it refuses amounts the core cannot hold (docs/DESIGN.md).
pub fn amount_text(text: &str) -> Result<String> {
    let clean = text.trim().replace(',', "");
    let b = clean.as_bytes();
    let body = clean.strip_prefix(['-', '+']).unwrap_or(&clean);
    let (int, frac) = body
        .split_once('.')
        .map_or((body, None), |(i, f)| (i, Some(f)));
    let digits = |s: &str| s.bytes().all(|c| c.is_ascii_digit());
    let shaped = !b.is_empty()
        && digits(int)
        && frac.is_none_or(digits)
        && (!int.is_empty() || frac.is_some_and(|f| !f.is_empty()));
    check(shaped && parse_decimal(&clean).is_some(), || {
        format!("invalid amount {text:?}")
    })?;
    Ok(clean.strip_prefix('+').unwrap_or(&clean).to_owned())
}

/// Input text as a date.
pub fn parse_date(d: &str) -> Result<Date> {
    d.parse().map_err(|e| match e {
        DateError::Shape => InputError(format!("invalid date {d:?}")),
        DateError::Range => InputError(format!("invalid date {d}")),
    })
}

fn check_account(a: &str) -> Result<()> {
    check(is_account(a), || format!("invalid account name {a:?}"))
}

fn check_currency(c: &str) -> Result<()> {
    check(is_currency(c), || format!("invalid currency {c:?}"))
}

fn meta_value_text(v: &MetaInputValue) -> Result<String> {
    let v = match v {
        MetaInputValue::Text(s) => return Ok(quote(s)),
        MetaInputValue::Value(v) => v,
    };
    Ok(match v {
        MetaValue::String(s) => quote(s),
        MetaValue::Account(a) => {
            check_account(a)?;
            a.clone()
        }
        MetaValue::Currency(c) => {
            check_currency(c)?;
            c.clone()
        }
        MetaValue::Date(d) => d.to_string(),
        MetaValue::Tag(t) => {
            check(is_tag(t), || format!("invalid tag {t}"))?;
            format!("#{t}")
        }
        MetaValue::Number(n) => plain(*n),
        MetaValue::Amount(a) => {
            check_currency(&a.currency)?;
            format!("{} {}", fixed(a.number, a.scale), a.currency)
        }
        MetaValue::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_owned(),
        MetaValue::Null => "NULL".to_owned(),
    })
}

fn meta_lines<'a>(
    meta: impl IntoIterator<Item = &'a (String, MetaInputValue)>,
    indent: &str,
) -> Result<Vec<String>> {
    meta.into_iter()
        .map(|(k, v)| {
            check(is_key(k), || format!("invalid metadata key {k:?}"))?;
            Ok(format!("{indent}{k}: {}", meta_value_text(v)?))
        })
        .collect()
}

/// Renders an entry. `currency` is the ledger's budget currency, used when an amount has none.
pub fn render_entry(e: &EntryInput, currency: &str) -> Result<Vec<String>> {
    parse_date(e.date())?;
    match e {
        EntryInput::Txn(e) => {
            check(e.postings.len() >= 2, || {
                "a transaction needs at least two postings".to_owned()
            })?;
            let mut head = vec![format!("{} {}", e.date, e.flag.as_deref().unwrap_or("*"))];
            if let Some(p) = &e.payee {
                head.push(quote(p));
            }
            head.push(quote(&e.narration));
            for t in &e.tags {
                check(is_tag(t), || format!("invalid tag {t}"))?;
                head.push(format!("#{t}"));
            }
            for t in &e.links {
                check(is_tag(t), || format!("invalid link {t}"))?;
                head.push(format!("^{t}"));
            }
            let width = e
                .postings
                .iter()
                .map(|p| p.account.chars().count())
                .max()
                .unwrap_or(0);
            let amounts = e
                .postings
                .iter()
                .map(|p| p.amount.as_deref().map(amount_text).transpose())
                .collect::<Result<Vec<_>>>()?;
            let num_width = amounts
                .iter()
                .map(|a| a.as_ref().map_or(0, std::string::String::len))
                .max()
                .unwrap_or(0);
            let mut out = vec![head.join(" ")];
            out.extend(meta_lines(&e.meta, "  ")?);
            for (p, amt) in e.postings.iter().zip(&amounts) {
                check_account(&p.account)?;
                match amt {
                    None => out.push(format!("  {}", p.account)),
                    Some(amt) => {
                        let cur = p.currency.as_deref().unwrap_or(currency);
                        check_currency(cur)?;
                        out.push(format!("  {:<width$}  {amt:>num_width$} {cur}", p.account));
                    }
                }
                out.extend(meta_lines(&p.meta, "    ")?);
            }
            Ok(out)
        }
        EntryInput::Open(e) => {
            check_account(&e.account)?;
            for c in &e.currencies {
                check_currency(c)?;
            }
            let cur = if e.currencies.is_empty() {
                String::new()
            } else {
                format!(" {}", e.currencies.join(","))
            };
            let booking = e
                .booking
                .as_deref()
                .filter(|b| !b.is_empty())
                .map_or(String::new(), |b| format!(" {}", quote(b)));
            let mut out = vec![format!("{} open {}{cur}{booking}", e.date, e.account)];
            if let Some(flag) = e.budget {
                out.push(format!("  budget: {}", quote(flag.as_str())));
            }
            out.extend(meta_lines(
                e.meta.iter().filter(|(k, _)| k != "budget"),
                "  ",
            )?);
            Ok(out)
        }
        EntryInput::Close(e) => {
            check_account(&e.account)?;
            Ok(vec![format!("{} close {}", e.date, e.account)])
        }
        EntryInput::Assign(e) => {
            check_account(&e.category)?;
            let cur = e.currency.as_deref().unwrap_or(currency);
            check_currency(cur)?;
            Ok(vec![format!(
                "{} custom \"envelope-assign\" {} {} {cur}",
                e.date,
                e.category,
                amount_text(&e.amount)?
            )])
        }
        EntryInput::Move(e) => {
            check_account(&e.from)?;
            check_account(&e.to)?;
            check(e.from != e.to, || {
                "envelope-move needs two different categories".to_owned()
            })?;
            let cur = e.currency.as_deref().unwrap_or(currency);
            check_currency(cur)?;
            Ok(vec![format!(
                "{} custom \"envelope-move\" {} {} {} {cur}",
                e.date,
                e.from,
                e.to,
                amount_text(&e.amount)?
            )])
        }
        EntryInput::Target(e) => {
            check_account(&e.category)?;
            let cur = e.currency.as_deref().unwrap_or(currency);
            check_currency(cur)?;
            let mut out = vec![format!(
                "{} custom \"target\" {} {} {cur}",
                e.date,
                e.category,
                amount_text(&e.amount)?
            )];
            if let Some(due) = &e.due {
                parse_date(due)?;
                out.push(format!("  due: {due}"));
            }
            if let Some(r) = e.recurrence {
                out.push(format!("  recurrence: {}", quote(r.as_str())));
            }
            if let Some(r) = e.rollover {
                out.push(format!("  rollover: {}", quote(r.as_str())));
            }
            Ok(out)
        }
        EntryInput::Balance(e) => {
            check_account(&e.account)?;
            let cur = e.currency.as_deref().unwrap_or(currency);
            check_currency(cur)?;
            Ok(vec![format!(
                "{} balance {}  {} {cur}",
                e.date,
                e.account,
                amount_text(&e.amount)?
            )])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns() {
        assert!(is_account("Assets:Checking-1:2"));
        assert!(!is_account("Assets"));
        assert!(!is_account("Assets:checking"));
        assert!(!is_account("Assets::X"));
        assert!(!is_account("Bogus:X"));
        assert!(is_currency("USD") && is_currency("A") && is_currency("A.B1"));
        assert!(
            !is_currency("US-") && !is_currency("") && !is_currency("ABCDEFGHIJKLMNOPQRSTUVWXY")
        );
        assert_eq!(quote("a\nb\r\nc\\d\"e"), r#""a\nb\nc\\d\"e""#);
        assert_eq!(amount_text(" 1,000.50 ").unwrap(), "1000.50");
        assert_eq!(amount_text("+5").unwrap(), "5");
        assert_eq!(amount_text(".5").unwrap(), ".5");
        assert_eq!(amount_text("5.").unwrap(), "5.");
        assert!(
            amount_text("1e5").is_err() && amount_text("-").is_err() && amount_text(".").is_err()
        );
        assert!(parse_date("0050-02-29").is_err() && parse_date("2024-02-29").is_ok());
        assert_eq!(
            parse_date("2026-1-01"),
            Err(InputError("invalid date \"2026-1-01\"".into()))
        );
    }
}
