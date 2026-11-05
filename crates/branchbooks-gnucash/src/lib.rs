//! Reads a `GnuCash` book, saved as XML (compressed, as `GnuCash` saves it, or plain), into Beancount
//! files for a new `BranchBooks` book: `main.beancount` with the options and every account's open,
//! and one file per year with its transactions, included from it. Entries are written by the
//! core's renderer, so they are what the write path would write.
//!
//! - Accounts: the `GnuCash` tree under its type's root (Assets, Liabilities, Income, Expenses,
//!   Equity); a top-level "Assets" (and so on) folder is not repeated. Names become Beancount
//!   names ("Checking Account" is Checking-Account) and keep the `GnuCash` path as `gnucash:`
//!   metadata. Bank, cash and card accounts are on budget; other assets and liabilities are off
//!   budget; expenses are budgeted categories. All open on the first transaction's date.
//! - Transactions: every one with its date, description, number and notes, and each split's
//!   memo. Amounts are the splits' values in the transaction's currency, so every transaction
//!   balances. A split in another commodity (shares, another currency) keeps its quantity as
//!   `quantity:` metadata: costs and prices are not supported yet.
//! - Left out: scheduled transactions, prices, `GnuCash` budgets; counted in `notes`.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

use std::collections::HashMap;
use std::io::Read;
use std::str::FromStr;

use branchbooks_core::date::Date;
use branchbooks_core::write::render::{
    BudgetFlag, EntryInput, InputError, MetaInputValue, OpenInput, PostingInput, TransactionInput,
    render_entry,
};
use indexmap::IndexMap;
use roxmltree::Node;
use rust_decimal::Decimal;
use unicode_normalization::UnicodeNormalization;

/// A converted book.
#[derive(Debug)]
pub struct Converted {
    /// Beancount files by path: main.beancount and one per year.
    pub files: IndexMap<String, String>,
    pub accounts: usize,
    pub transactions: usize,
    /// What did not come over as it was, for the person importing.
    pub notes: Vec<String>,
}

struct Account {
    name: String,
    kind: String,
    parent: Option<String>,
    /// (space, id): ("CURRENCY", "USD"), or ("NASDAQ", "AAPL") for shares.
    commodity: Option<(String, String)>,
    /// `GnuCash`'s own placeholder setting: it only groups its sub-accounts.
    placeholder: bool,
}

fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.tag_name().name() == name)
}

fn text(n: Node, name: &str) -> String {
    child(n, name)
        .and_then(|c| c.text())
        .unwrap_or("")
        .trim()
        .to_owned()
}

fn commodity(n: Node) -> Option<(String, String)> {
    Some((text(n, "space"), text(n, "id"))).filter(|(_, id)| !id.is_empty())
}

fn is_currency(space: &str) -> bool {
    space == "ISO4217" || space == "CURRENCY"
}

/// A `GnuCash` slot's string value (`notes`, `placeholder`).
fn slot(n: Node, key: &str) -> Option<String> {
    let slots = child(n, "slots")?;
    slots
        .children()
        .filter(|s| s.tag_name().name() == "slot")
        .find(|s| text(*s, "key") == key)
        .map(|s| text(s, "value"))
        .filter(|v| !v.is_empty())
}

/// "12345/100" as 123.45.
fn fraction(s: &str) -> Option<Decimal> {
    let (n, d) = s.split_once('/').unwrap_or((s, "1"));
    let (n, d) = (Decimal::from_str(n).ok()?, Decimal::from_str(d).ok()?);
    if d.is_zero() {
        return None;
    }
    let digits = d.to_string().len() as u32 - 1;
    Some((n / d).round_dp(digits.max(2)))
}

/// The Beancount root for a `GnuCash` account type.
fn root(kind: &str) -> &'static str {
    match kind {
        "LIABILITY" | "CREDIT" | "PAYABLE" => "Liabilities",
        "INCOME" => "Income",
        "EXPENSE" => "Expenses",
        "EQUITY" | "TRADING" => "Equity",
        _ => "Assets",
    }
}

/// One part of a Beancount account name: ASCII letters, digits and dashes, starting with a capital
/// or a digit. "checking account" is Checking-Account; "Café" is Cafe.
fn component(name: &str) -> String {
    let ascii: String = name.nfkd().filter(char::is_ascii).collect();
    let words: Vec<String> = ascii
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w[..1].to_ascii_uppercase() + &w[1..])
        .collect();
    if words.is_empty() {
        "Account".to_owned()
    } else {
        words.join("-")
    }
}

/// The `GnuCash` path of an account: its parents' names and its own, without the root.
fn path(id: &str, accounts: &HashMap<String, Account>) -> Vec<String> {
    let mut parts = Vec::new();
    let mut at = accounts.get(id);
    while let Some(a) = at.filter(|a| a.kind != "ROOT") {
        // A corrupt file can make accounts each other's parents.
        if parts.len() == accounts.len() {
            break;
        }
        parts.push(a.name.clone());
        at = a.parent.as_ref().and_then(|p| accounts.get(p));
    }
    parts.reverse();
    parts
}

/// A transaction, by its date for sorting.
struct Txn {
    date: Date,
    input: TransactionInput,
}

/// Why a file could not be imported.
#[derive(Debug)]
pub enum ConvertError {
    /// Compressed, but not something that decompresses to text.
    Compressed(std::io::Error),
    NotText,
    Xml(roxmltree::Error),
    NoBook,
    TransactionDate {
        description: String,
        date: String,
    },
    Amount {
        description: String,
        value: String,
    },
    Account {
        name: String,
        reason: InputError,
    },
    Transaction {
        date: Date,
        narration: String,
        reason: InputError,
    },
}

impl std::fmt::Display for ConvertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConvertError::Compressed(e) => write!(f, "not a GnuCash file: {e}"),
            ConvertError::NotText => f.write_str("not a GnuCash file"),
            ConvertError::Xml(e) => write!(f, "not a GnuCash XML file: {e}"),
            ConvertError::NoBook => {
                f.write_str("no book in this GnuCash file (is it saved as SQLite? Save As XML)")
            }
            ConvertError::TransactionDate { description, date } => {
                write!(f, "transaction {description:?}: invalid date {date:?}")
            }
            ConvertError::Amount { description, value } => {
                write!(f, "transaction {description:?}: invalid amount {value:?}")
            }
            ConvertError::Account { name, reason } => write!(f, "{name}: {reason}"),
            ConvertError::Transaction {
                date,
                narration,
                reason,
            } => {
                write!(f, "transaction {date} {narration:?}: {reason}")
            }
        }
    }
}

impl std::error::Error for ConvertError {}

pub fn convert(bytes: &[u8]) -> Result<Converted, ConvertError> {
    let xml = if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut s = String::new();
        flate2::read::GzDecoder::new(bytes)
            .read_to_string(&mut s)
            .map_err(ConvertError::Compressed)?;
        s
    } else {
        String::from_utf8(bytes.to_vec()).map_err(|_| ConvertError::NotText)?
    };
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    let doc = roxmltree::Document::parse_with_options(&xml, options).map_err(ConvertError::Xml)?;
    let book = doc
        .descendants()
        .find(|n| n.tag_name().name() == "book")
        .ok_or(ConvertError::NoBook)?;

    let mut accounts: HashMap<String, Account> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut notes = Vec::new();
    let mut skipped = HashMap::<&str, usize>::new();
    for n in book.children().filter(roxmltree::Node::is_element) {
        match n.tag_name().name() {
            "account" => {
                let id = text(n, "id");
                order.push(id.clone());
                accounts.insert(
                    id,
                    Account {
                        name: text(n, "name"),
                        kind: text(n, "type"),
                        parent: Some(text(n, "parent")).filter(|p| !p.is_empty()),
                        commodity: child(n, "commodity").and_then(commodity),
                        placeholder: slot(n, "placeholder").as_deref() == Some("true"),
                    },
                );
            }
            "schedxaction" => *skipped.entry("scheduled transaction(s)").or_default() += 1,
            "pricedb" => {
                let prices = n.children().filter(roxmltree::Node::is_element).count();
                *skipped.entry("price(s)").or_default() += prices;
            }
            "budget" => *skipped.entry("GnuCash budget(s)").or_default() += 1,
            _ => {}
        }
    }

    // Beancount names, unique, in GnuCash's order.
    let mut names: HashMap<String, String> = HashMap::new();
    let mut taken: HashMap<String, usize> = HashMap::new();
    for id in &order {
        let a = &accounts[id];
        if a.kind == "ROOT" {
            continue;
        }
        let top = root(&a.kind);
        let mut parts = path(id, &accounts);
        // "Assets:Checking" in GnuCash is Assets:Checking, not Assets:Assets:Checking.
        let first = parts.first().map(|p| component(p).to_ascii_lowercase());
        let word = top.to_ascii_lowercase();
        if parts.len() > 1
            && first.is_some_and(|f| {
                f == word || format!("{f}s") == word || f == word.trim_end_matches('s')
            })
        {
            parts.remove(0);
        }
        let base = std::iter::once(top.to_owned())
            .chain(parts.iter().map(|p| component(p)))
            .collect::<Vec<_>>()
            .join(":");
        let n = taken.entry(base.clone()).or_default();
        *n += 1;
        names.insert(
            id.clone(),
            if *n == 1 { base } else { format!("{base}-{n}") },
        );
    }

    // Transactions, outside the scheduled transactions' templates.
    let mut txns: Vec<Txn> = Vec::new();
    let mut currencies: IndexMap<String, usize> = IndexMap::new();
    let mut other_commodity = 0;
    // Accounts some split uses: a GnuCash folder (sub-accounts, no splits) is not opened.
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Accounts with a split in another currency: they open with no currency limit, or the ledger
    // would refuse those splits.
    let mut mixed: std::collections::HashSet<String> = std::collections::HashSet::new();
    for t in book
        .children()
        .filter(|n| n.tag_name().name() == "transaction")
    {
        let currency = child(t, "currency")
            .and_then(commodity)
            .map(|(_, id)| id)
            .unwrap_or_default();
        *currencies.entry(currency.clone()).or_default() += 1;
        let date: String = child(t, "date-posted")
            .map(|d| text(d, "date"))
            .unwrap_or_default()
            .chars()
            .take(10)
            .collect();
        let mut meta = Vec::new();
        let num = text(t, "num");
        if !num.is_empty() {
            meta.push(("num".to_owned(), MetaInputValue::Text(num)));
        }
        if let Some(n) = slot(t, "notes") {
            meta.push(("notes".to_owned(), MetaInputValue::Text(n)));
        }
        let mut postings = Vec::new();
        for s in child(t, "splits")
            .into_iter()
            .flat_map(|ss| ss.children().filter(|s| s.tag_name().name() == "split"))
        {
            let account = text(s, "account");
            used.insert(account.clone());
            let Some(name) = names.get(&account) else {
                continue;
            };
            // Never a silent zero: zeros on every split still balance, so no error would show it.
            let amount = |field: &str| {
                let value = text(s, field);
                fraction(&value).ok_or_else(|| ConvertError::Amount {
                    description: text(t, "description"),
                    value,
                })
            };
            let value = amount("value")?;
            let mut pmeta = Vec::new();
            let memo = text(s, "memo");
            if !memo.is_empty() {
                pmeta.push(("memo".to_owned(), MetaInputValue::Text(memo)));
            }
            if let Some((_, id)) = accounts[&account]
                .commodity
                .as_ref()
                .filter(|(_, id)| *id != currency)
            {
                let qty = amount("quantity")?;
                pmeta.push((
                    "quantity".to_owned(),
                    MetaInputValue::Text(format!("{qty} {id}")),
                ));
                other_commodity += 1;
                mixed.insert(account.clone());
            }
            postings.push(PostingInput {
                account: name.clone(),
                amount: Some(value.to_string()),
                currency: Some(currency.clone()),
                meta: pmeta,
            });
        }
        if postings.len() < 2 {
            *skipped
                .entry("transaction(s) with fewer than two splits")
                .or_default() += 1;
            continue;
        }
        let posted = date.parse().map_err(|_| ConvertError::TransactionDate {
            description: text(t, "description"),
            date: date.clone(),
        })?;
        txns.push(Txn {
            date: posted,
            input: TransactionInput {
                date,
                narration: text(t, "description"),
                meta,
                postings,
                ..Default::default()
            },
        });
    }
    txns.sort_by_key(|t| t.date);
    let book_currency = currencies
        .iter()
        .max_by_key(|(_, n)| **n)
        .map(|(c, _)| c.clone())
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "USD".to_owned());
    if other_commodity > 0 {
        notes.push(format!(
            "{other_commodity} split(s) in another commodity (shares or another currency) came over at their \
             value in the transaction's currency, with the quantity kept as metadata"
        ));
    }
    for (what, n) in &skipped {
        notes.push(format!("left out {n} {what}"));
    }
    notes.sort();

    // main.beancount: options, the opens, then the year files.
    let first_date = txns
        .first()
        .map_or_else(|| "2000-01-01".to_owned(), |t| t.date.to_string());
    let mut main = format!(
        "; Imported from GnuCash.\noption \"title\" \"GnuCash import\"\noption \"operating_currency\" \"{book_currency}\"\n\n"
    );
    let mut opened = 0;
    for id in &order {
        let Some(name) = names.get(id) else { continue };
        let folder = accounts
            .values()
            .any(|b| b.parent.as_deref() == Some(id.as_str()));
        let a = &accounts[id];
        // Kept as a placeholder when GnuCash says it is one. One with splits holds money, so it comes
        // over as an ordinary account, and the notes say so.
        let holding = a.placeholder && !used.contains(id);
        if folder && !used.contains(id) && !holding {
            continue;
        }
        let budget = match a.kind.as_str() {
            _ if holding => Some(BudgetFlag::Placeholder),
            "BANK" | "CASH" | "CREDIT" => Some(BudgetFlag::On),
            "ASSET" | "STOCK" | "MUTUAL" | "RECEIVABLE" | "LIABILITY" | "PAYABLE" => {
                Some(BudgetFlag::Off)
            }
            _ => None,
        };
        if a.placeholder && !holding {
            let name = path(id, &accounts).join(":");
            notes.push(format!(
                "{name} is a placeholder in GnuCash but has transactions, so it came over as an ordinary account"
            ));
        }
        let currencies = match &a.commodity {
            Some((space, cur))
                if is_currency(space) && *cur == book_currency && !mixed.contains(id) =>
            {
                vec![cur.clone()]
            }
            _ => Vec::new(),
        };
        let open = EntryInput::Open(OpenInput {
            date: first_date.clone(),
            account: name.clone(),
            currencies,
            budget,
            meta: vec![(
                "gnucash".to_owned(),
                MetaInputValue::Text(path(id, &accounts).join(":")),
            )],
            ..Default::default()
        });
        let lines =
            render_entry(&open, &book_currency).map_err(|reason| ConvertError::Account {
                name: name.clone(),
                reason,
            })?;
        main.push_str(&lines.join("\n"));
        main.push('\n');
        opened += 1;
    }
    let mut years: IndexMap<String, String> = IndexMap::new();
    for t in &txns {
        let lines =
            render_entry(&EntryInput::Txn(t.input.clone()), &book_currency).map_err(|reason| {
                ConvertError::Transaction {
                    date: t.date,
                    narration: t.input.narration.clone(),
                    reason,
                }
            })?;
        let file = years
            .entry(format!("{:04}.beancount", t.date.year()))
            .or_default();
        if !file.is_empty() {
            file.push('\n');
        }
        file.push_str(&lines.join("\n"));
        file.push('\n');
    }
    years.sort_keys();
    main.push('\n');
    for y in years.keys() {
        main.push_str(&format!("include \"{y}\"\n"));
    }
    let mut files = IndexMap::new();
    files.insert("main.beancount".to_owned(), main);
    files.extend(years);
    Ok(Converted {
        files,
        accounts: opened,
        transactions: txns.len(),
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use branchbooks_core::book::Book;
    use branchbooks_core::loader::MemorySource;
    use std::io::Write;

    /// A small book as `GnuCash` saves it: a root, nested accounts, a card, shares, two
    /// transactions over two years, and a scheduled transaction's template that must be skipped.
    const BOOK: &str = r#"<?xml version="1.0" encoding="utf-8" ?>
<gnc-v2 xmlns:gnc="http://www.gnucash.org/XML/gnc" xmlns:act="http://www.gnucash.org/XML/act"
  xmlns:trn="http://www.gnucash.org/XML/trn" xmlns:split="http://www.gnucash.org/XML/split"
  xmlns:cmdty="http://www.gnucash.org/XML/cmdty" xmlns:ts="http://www.gnucash.org/XML/ts"
  xmlns:slot="http://www.gnucash.org/XML/slot" xmlns:book="http://www.gnucash.org/XML/book">
<gnc:book version="2.0.0">
<gnc:account version="2.0.0"><act:name>Root Account</act:name><act:id type="guid">r</act:id><act:type>ROOT</act:type></gnc:account>
<gnc:account version="2.0.0"><act:name>Assets</act:name><act:id type="guid">a</act:id><act:type>ASSET</act:type>
  <act:commodity><cmdty:space>CURRENCY</cmdty:space><cmdty:id>USD</cmdty:id></act:commodity><act:parent type="guid">r</act:parent></gnc:account>
<gnc:account version="2.0.0"><act:name>Checking Account</act:name><act:id type="guid">c</act:id><act:type>BANK</act:type>
  <act:commodity><cmdty:space>CURRENCY</cmdty:space><cmdty:id>USD</cmdty:id></act:commodity><act:parent type="guid">a</act:parent></gnc:account>
<gnc:account version="2.0.0"><act:name>Brokerage</act:name><act:id type="guid">s</act:id><act:type>STOCK</act:type>
  <act:commodity><cmdty:space>NASDAQ</cmdty:space><cmdty:id>ACME</cmdty:id></act:commodity><act:parent type="guid">a</act:parent></gnc:account>
<gnc:account version="2.0.0"><act:name>Credit Card</act:name><act:id type="guid">v</act:id><act:type>CREDIT</act:type>
  <act:commodity><cmdty:space>CURRENCY</cmdty:space><cmdty:id>USD</cmdty:id></act:commodity><act:parent type="guid">r</act:parent></gnc:account>
<gnc:account version="2.0.0"><act:name>Expenses</act:name><act:id type="guid">e</act:id><act:type>EXPENSE</act:type>
  <act:commodity><cmdty:space>CURRENCY</cmdty:space><cmdty:id>USD</cmdty:id></act:commodity><act:parent type="guid">r</act:parent></gnc:account>
<gnc:account version="2.0.0"><act:name>Café &amp; dining</act:name><act:id type="guid">d</act:id><act:type>EXPENSE</act:type>
  <act:commodity><cmdty:space>CURRENCY</cmdty:space><cmdty:id>USD</cmdty:id></act:commodity><act:parent type="guid">e</act:parent></gnc:account>
<gnc:account version="2.0.0"><act:name>Opening Balances</act:name><act:id type="guid">o</act:id><act:type>EQUITY</act:type>
  <act:commodity><cmdty:space>CURRENCY</cmdty:space><cmdty:id>USD</cmdty:id></act:commodity><act:parent type="guid">r</act:parent></gnc:account>
<gnc:transaction version="2.0.0"><trn:id type="guid">t1</trn:id>
  <trn:currency><cmdty:space>CURRENCY</cmdty:space><cmdty:id>USD</cmdty:id></trn:currency>
  <trn:date-posted><ts:date>2025-12-31 10:59:00 +0000</ts:date></trn:date-posted>
  <trn:description>Opening "balance"</trn:description>
  <trn:splits>
    <trn:split><split:id type="guid">x1</split:id><split:reconciled-state>y</split:reconciled-state><split:value>100000/100</split:value><split:quantity>100000/100</split:quantity><split:account type="guid">c</split:account></trn:split>
    <trn:split><split:id type="guid">x2</split:id><split:reconciled-state>y</split:reconciled-state><split:value>-100000/100</split:value><split:quantity>-100000/100</split:quantity><split:account type="guid">o</split:account></trn:split>
  </trn:splits></gnc:transaction>
<gnc:transaction version="2.0.0"><trn:id type="guid">t2</trn:id>
  <trn:currency><cmdty:space>CURRENCY</cmdty:space><cmdty:id>USD</cmdty:id></trn:currency>
  <trn:num>42</trn:num>
  <trn:date-posted><ts:date>2026-01-05 10:59:00 +0000</ts:date></trn:date-posted>
  <trn:description>Lunch and shares</trn:description>
  <trn:slots><slot><slot:key>notes</slot:key><slot:value type="string">with Ann</slot:value></slot></trn:slots>
  <trn:splits>
    <trn:split><split:id type="guid">y1</split:id><split:memo>sandwich</split:memo><split:reconciled-state>n</split:reconciled-state><split:value>1250/100</split:value><split:quantity>1250/100</split:quantity><split:account type="guid">d</split:account></trn:split>
    <trn:split><split:id type="guid">y2</split:id><split:reconciled-state>n</split:reconciled-state><split:value>-1250/100</split:value><split:quantity>-1250/100</split:quantity><split:account type="guid">v</split:account></trn:split>
    <trn:split><split:id type="guid">y3</split:id><split:reconciled-state>n</split:reconciled-state><split:value>20000/100</split:value><split:quantity>2/1</split:quantity><split:account type="guid">s</split:account></trn:split>
    <trn:split><split:id type="guid">y4</split:id><split:reconciled-state>n</split:reconciled-state><split:value>-20000/100</split:value><split:quantity>-20000/100</split:quantity><split:account type="guid">c</split:account></trn:split>
  </trn:splits></gnc:transaction>
<gnc:template-transactions>
<gnc:transaction version="2.0.0"><trn:id type="guid">tt</trn:id>
  <trn:currency><cmdty:space>CURRENCY</cmdty:space><cmdty:id>USD</cmdty:id></trn:currency>
  <trn:date-posted><ts:date>2026-02-01 10:59:00 +0000</ts:date></trn:date-posted>
  <trn:description>Template</trn:description><trn:splits></trn:splits></gnc:transaction>
</gnc:template-transactions>
<gnc:schedxaction version="2.0.0"/>
</gnc:book>
</gnc-v2>"#;

    fn gzip(text: &str) -> Vec<u8> {
        let mut z = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(text.as_bytes()).unwrap();
        z.finish().unwrap()
    }

    fn loaded(c: &Converted) -> Book {
        let files: Vec<(&str, &str)> = c
            .files
            .iter()
            .map(|(p, t)| (p.as_str(), t.as_str()))
            .collect();
        Book::load("main.beancount", &MemorySource::new(files))
    }

    /// `BOOK` with the account of guid `id` marked as a placeholder, as `GnuCash` saves it.
    fn with_placeholder(id: &str) -> String {
        let at = format!(r#"<act:id type="guid">{id}</act:id>"#);
        assert!(BOOK.contains(&at));
        BOOK.replace(
            &at,
            &format!(
                r#"{at}<act:slots><slot><slot:key>placeholder</slot:key><slot:value type="string">true</slot:value></slot></act:slots>"#
            ),
        )
    }

    #[test]
    fn a_gnucash_placeholder_stays_one_unless_it_holds_money() {
        // Expenses is a folder with no splits: skipped unless it is marked.
        let c = convert(with_placeholder("e").as_bytes()).unwrap();
        let main = &c.files["main.beancount"];
        assert!(main.contains("budget: \"placeholder\""), "{main}");
        assert_eq!(c.accounts, 6);
        let mut book = loaded(&c);
        assert_eq!(book.errors(), vec![]);
        assert_eq!(book.budget_diagnostics(), vec![]);

        // Checking has splits: an ordinary account, and the person is told.
        let c = convert(with_placeholder("c").as_bytes()).unwrap();
        assert!(!c.files["main.beancount"].contains("budget: \"placeholder\""));
        assert!(
            c.notes
                .iter()
                .any(|n| n.contains("came over as an ordinary account"))
        );
    }

    #[test]
    fn notes_with_a_line_break_keep_their_transaction() {
        let c = convert(BOOK.replace("with Ann", "with Ann\nand Bob").as_bytes()).unwrap();
        let mut book = loaded(&c);
        assert!(book.errors().is_empty(), "{:?}", book.errors());
        assert_eq!(book.transactions().len(), 2);
        assert!(c.files["2026.beancount"].contains(r#"notes: "with Ann\nand Bob""#));
    }

    #[test]
    fn an_account_with_a_foreign_currency_split_opens_without_a_limit() {
        const USD: &str =
            "<trn:currency><cmdty:space>CURRENCY</cmdty:space><cmdty:id>USD</cmdty:id>";
        let eur = USD.replace("USD", "EUR");
        let c = convert(BOOK.replacen(USD, &eur, 1).as_bytes()).unwrap();
        let mut book = loaded(&c);
        assert!(book.errors().is_empty(), "{:?}", book.errors());
        assert!(c.files["main.beancount"].contains("open Assets:Checking-Account\n"));
        assert!(c.notes.iter().any(|n| n.contains("another commodity")));
    }

    #[test]
    fn an_unreadable_amount_is_refused_not_read_as_zero() {
        let e = convert(BOOK.replace("1250/100", "oops").as_bytes()).unwrap_err();
        assert_eq!(
            e.to_string(),
            r#"transaction "Lunch and shares": invalid amount "oops""#
        );
    }

    #[test]
    fn a_gnucash_book_becomes_a_valid_ledger_with_the_same_balances() {
        let c = convert(&gzip(BOOK)).unwrap();
        // Assets and Expenses are GnuCash folders (sub-accounts, no splits): not opened.
        assert_eq!((c.accounts, c.transactions), (5, 2));
        assert!(
            !c.files["main.beancount"].contains("open Assets:Assets")
                && !c.files["main.beancount"].contains("open Expenses ")
        );
        assert_eq!(
            c.files.keys().map(String::as_str).collect::<Vec<_>>(),
            ["main.beancount", "2025.beancount", "2026.beancount"]
        );
        let main = &c.files["main.beancount"];
        assert!(
            main.contains("open Assets:Checking-Account USD\n  budget: \"on\""),
            "{main}"
        );
        assert!(main.contains("open Liabilities:Credit-Card USD\n  budget: \"on\""));
        assert!(main.contains("open Assets:Brokerage\n  budget: \"off\""));
        assert!(main.contains("open Expenses:Cafe-Dining USD"));
        assert!(main.contains("gnucash: \"Expenses:Café & dining\""));
        assert!(
            c.notes
                .iter()
                .any(|n| n.starts_with("1 split(s) in another commodity"))
        );
        assert!(
            c.notes
                .iter()
                .any(|n| n == "left out 1 scheduled transaction(s)")
        );

        let files: Vec<(&str, &str)> = c
            .files
            .iter()
            .map(|(p, t)| (p.as_str(), t.as_str()))
            .collect();
        let mut book = Book::load("main.beancount", &MemorySource::new(files));
        assert!(book.errors().is_empty(), "{:?}", book.errors());
        let balance = |b: &mut Book, a: &str| b.balances()[a]["USD"];
        assert_eq!(
            balance(&mut book, "Assets:Checking-Account"),
            Decimal::from(800)
        );
        assert_eq!(balance(&mut book, "Assets:Brokerage"), Decimal::from(200));
        assert_eq!(
            balance(&mut book, "Liabilities:Credit-Card"),
            Decimal::from_str("-12.50").unwrap()
        );
        let year = &c.files["2026.beancount"];
        assert!(
            year.contains("num: \"42\"") && year.contains("notes: \"with Ann\""),
            "{year}"
        );
        assert!(year.contains("memo: \"sandwich\"") && year.contains("quantity: \"2 ACME\""));
        assert!(c.files["2025.beancount"].contains("Opening \\\"balance\\\""));
        // A plain (uncompressed) XML file reads the same.
        assert_eq!(convert(BOOK.as_bytes()).unwrap().files, c.files);
    }

    #[test]
    fn names_become_beancount_names() {
        assert_eq!(component("checking account"), "Checking-Account");
        assert_eq!(component("401(k)"), "401-K");
        assert_eq!(component("Café"), "Cafe");
        assert_eq!(component("***"), "Account");
        assert_eq!(fraction("12345/100"), Decimal::from_str("123.45").ok());
        assert_eq!(fraction("2/1"), Decimal::from_str("2.00").ok());
    }

    #[test]
    fn a_transaction_with_no_such_date_is_refused() {
        let bad = BOOK.replace("2026-01-05 10:59:00", "2026-02-31 10:59:00");
        let e = convert(bad.as_bytes()).unwrap_err().to_string();
        assert!(e.contains("invalid date \"2026-02-31\""), "{e}");
    }

    #[test]
    fn accounts_that_are_each_others_parents_do_not_hang_the_import() {
        let cycle = "<gnc-v2><gnc:book xmlns:gnc=\"x\" xmlns:act=\"y\"><gnc:account><act:name>A</act:name><act:id>a</act:id><act:type>BANK</act:type><act:parent>b</act:parent></gnc:account><gnc:account><act:name>B</act:name><act:id>b</act:id><act:type>BANK</act:type><act:parent>a</act:parent></gnc:account></gnc:book></gnc-v2>";
        assert!(convert(cycle.as_bytes()).is_ok());
    }

    #[test]
    fn a_file_that_is_not_gnucash_is_refused() {
        assert!(convert(b"not xml").is_err());
        assert!(
            convert(&gzip("<gnc-v2></gnc-v2>"))
                .unwrap_err()
                .to_string()
                .contains("no book")
        );
    }
}
