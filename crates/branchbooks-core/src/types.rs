//! The ledger's data types: entries as parsed, their metadata, and diagnostics.

use std::sync::Arc;

use crate::date::Date;
use crate::money::Decimal;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Amount {
    pub number: Decimal,
    pub currency: String,
    /// Digits after the decimal point as written; used for Beancount-style tolerance inference.
    pub scale: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MetaValue {
    String(String),
    Account(String),
    Currency(String),
    Date(Date),
    Number(Decimal),
    Amount(Amount),
    Bool(bool),
    Tag(String),
    Null,
}

/// Metadata in the order written. Setting a key again replaces its value in place.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Meta(Vec<(String, MetaValue)>);

impl Meta {
    pub fn get(&self, key: &str) -> Option<&MetaValue> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn set(&mut self, key: String, value: MetaValue) {
        match self.0.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => self.0.push((key, value)),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &MetaValue)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The value of a string-valued key.
    pub fn str(&self, key: &str) -> Option<&str> {
        match self.get(key) {
            Some(MetaValue::String(s)) => Some(s),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub date: Date,
    pub file: Arc<str>,
    pub line: u32,
    /// Last line of the entry (header, postings and metadata; not trailing comments).
    pub end_line: u32,
    /// Same-day tie-break order: file load order, then line (see `seq_of` in the loader).
    pub seq: u64,
    pub meta: Meta,
    pub body: Body,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    Open(Open),
    Close(Close),
    Txn(Transaction),
    Custom(Custom),
    Balance(Balance),
    /// Directives we parse and keep but that never affect balances or the budget (commodity,
    /// price, event, note, ...).
    Other(Other),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Open {
    pub account: String,
    pub currencies: Vec<String>,
    pub booking: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Close {
    pub account: String,
}

/// `balance`: the account and its sub-accounts hold `amount` at the start of `date`, within
/// `tolerance` (by default one unit of the amount's last decimal place).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Balance {
    pub account: String,
    pub amount: Amount,
    pub tolerance: Option<Decimal>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Posting {
    pub account: String,
    /// `None` when elided; filled in by booking.
    pub units: Option<Amount>,
    pub flag: Option<String>,
    pub meta: Meta,
    pub line: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transaction {
    pub flag: String,
    pub payee: Option<String>,
    pub narration: String,
    pub tags: Vec<String>,
    pub links: Vec<String>,
    pub postings: Vec<Posting>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CustomValue {
    String(String),
    Account(String),
    Date(Date),
    Number(Decimal),
    Amount(Amount),
    Bool(bool),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Custom {
    pub name: String,
    pub values: Vec<CustomValue>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Other {
    pub keyword: String,
}

/// Which kind of entry, for sorting and matching.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EntryType {
    Open,
    Close,
    Txn,
    Custom,
    Balance,
    Other,
}

impl EntryType {
    /// The name for the type (`open`, `close`, `txn`, `custom`, `balance`, `other`).
    pub fn name(self) -> &'static str {
        match self {
            EntryType::Open => "open",
            EntryType::Close => "close",
            EntryType::Txn => "txn",
            EntryType::Custom => "custom",
            EntryType::Balance => "balance",
            EntryType::Other => "other",
        }
    }
}

impl Entry {
    pub fn entry_type(&self) -> EntryType {
        match self.body {
            Body::Open(_) => EntryType::Open,
            Body::Close(_) => EntryType::Close,
            Body::Txn(_) => EntryType::Txn,
            Body::Custom(_) => EntryType::Custom,
            Body::Balance(_) => EntryType::Balance,
            Body::Other(_) => EntryType::Other,
        }
    }

    pub fn txn(&self) -> Option<&Transaction> {
        match &self.body {
            Body::Txn(t) => Some(t),
            _ => None,
        }
    }

    pub fn txn_mut(&mut self) -> Option<&mut Transaction> {
        match &mut self.body {
            Body::Txn(t) => Some(t),
            _ => None,
        }
    }

    pub fn custom(&self) -> Option<&Custom> {
        match &self.body {
            Body::Custom(c) => Some(c),
            _ => None,
        }
    }

    pub fn balance(&self) -> Option<&Balance> {
        match &self.body {
            Body::Balance(b) => Some(b),
            _ => None,
        }
    }

    pub fn open(&self) -> Option<&Open> {
        match &self.body {
            Body::Open(o) => Some(o),
            _ => None,
        }
    }

    pub fn close(&self) -> Option<&Close> {
        match &self.body {
            Body::Close(c) => Some(c),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    pub file: Arc<str>,
    pub line: u32,
    pub code: String,
    pub message: String,
}

impl Diagnostic {
    pub fn new(file: &Arc<str>, line: u32, code: &str, message: impl Into<String>) -> Self {
        Diagnostic {
            file: file.clone(),
            line,
            code: code.to_owned(),
            message: message.into(),
        }
    }
}
