//! Importing entries from other Beancount files into a ledger: which entries are new, which are
//! already there, and which accounts the new entries need opened. Placement and the commit are in
//! the workspace.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use crate::date::Date;
use crate::ledger::book_transaction;
use crate::loader::{MemorySource, load};
use crate::money::plain;
use crate::parser::line_tokens;
use crate::types::{Body, Diagnostic, Entry};
use crate::write::text::split_lines;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImportKind {
    Txn,
    Open,
    Close,
    Assign,
    Move,
    Target,
    Custom,
    Balance,
    Other,
}

impl ImportKind {
    /// The kind's name (`txn`, `open`, `assign`, ...).
    pub fn name(self) -> &'static str {
        match self {
            ImportKind::Txn => "txn",
            ImportKind::Open => "open",
            ImportKind::Close => "close",
            ImportKind::Assign => "assign",
            ImportKind::Move => "move",
            ImportKind::Target => "target",
            ImportKind::Custom => "custom",
            ImportKind::Balance => "balance",
            ImportKind::Other => "other",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportItem {
    pub kind: ImportKind,
    pub date: Date,
    /// The entry's lines as written in the imported file (or generated, for an automatic open).
    pub lines: Vec<String>,
    /// For opens and closes.
    pub account: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportPreview {
    /// The file loaded as the imported ledger's root.
    pub root: String,
    /// New entries, in date order.
    pub add: Vec<ImportItem>,
    /// Entries skipped because the ledger already has them.
    pub duplicates: usize,
    /// Accounts the new entries use that neither ledger opens: opened on their first use.
    pub opens: Vec<String>,
    /// Options and plugins in the imported files, which the ledger keeps its own of.
    pub ignored: Vec<String>,
    /// Syntax and include errors in the imported files: nothing is imported while there are any.
    pub errors: Vec<Diagnostic>,
}

/// Whether a line of `text` starts with `include` and a space (`/^include\s/m`).
fn has_include(text: &str) -> bool {
    let mut at_line_start = true;
    for (i, c) in text.char_indices() {
        if at_line_start
            && text[i..].starts_with("include")
            && text[i + "include".len()..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
        {
            return true;
        }
        at_line_start = matches!(c, '\n' | '\r');
    }
    false
}

/// The root of a set of uploaded files: main.beancount, the one file that includes others, or the
/// only file.
pub fn import_root(files: &IndexMap<String, String>) -> Option<String> {
    if files.contains_key("main.beancount") {
        return Some("main.beancount".to_owned());
    }
    let including: Vec<&String> = files
        .iter()
        .filter(|(_, t)| has_include(t))
        .map(|(n, _)| n)
        .collect();
    if let [one] = including[..] {
        return Some(one.clone());
    }
    if files.len() == 1 {
        files.keys().next().cloned()
    } else {
        None
    }
}

pub fn kind_of(e: &Entry) -> ImportKind {
    match &e.body {
        Body::Custom(c) if c.name == "envelope-assign" => ImportKind::Assign,
        Body::Custom(c) if c.name == "envelope-move" => ImportKind::Move,
        Body::Custom(c) if c.name == "target" => ImportKind::Target,
        Body::Custom(_) => ImportKind::Custom,
        Body::Balance(_) => ImportKind::Balance,
        Body::Txn(_) => ImportKind::Txn,
        Body::Open(_) => ImportKind::Open,
        Body::Close(_) => ImportKind::Close,
        Body::Other(_) => ImportKind::Other,
    }
}

fn squash(lines: &[String]) -> String {
    lines
        .iter()
        .map(|l| match line_tokens(l) {
            Some(tokens) => tokens.join(" "),
            None => l.split_whitespace().collect::<Vec<_>>().join(" "),
        })
        .filter(|l| !l.is_empty() && !l.starts_with(';'))
        .collect::<Vec<_>>()
        .join("\n")
}

/// What identifies an entry for duplicate checks. A transaction is its date and postings (with
/// the elided amount filled in), not its words: a bank file and a hand-entered copy often describe
/// the same payment differently. Everything else is its text, ignoring spacing and comments.
fn key_of(e: &Entry, lines: &[String]) -> String {
    if e.txn().is_some() {
        let booked = book_transaction(e).txn;
        let t = booked.as_ref().unwrap_or(e).txn().expect("a transaction");
        let mut postings: Vec<String> = t
            .postings
            .iter()
            .map(|p| match &p.units {
                Some(u) => format!("{} {} {}", p.account, plain(u.number), u.currency),
                None => format!("{} ", p.account),
            })
            .collect();
        postings.sort();
        return format!("txn|{}|{}", e.date, postings.join("|"));
    }
    format!("{}|{}", kind_of(e).name(), squash(lines))
}

/// Counts keys, so two identical payments in the file against one in the ledger adds one.
#[derive(Default)]
struct Counter(HashMap<String, usize>);

impl Counter {
    fn add(&mut self, k: String) {
        *self.0.entry(k).or_insert(0) += 1;
    }

    fn take(&mut self, k: &str) -> bool {
        match self.0.get_mut(k) {
            Some(n) if *n > 0 => {
                *n -= 1;
                true
            }
            _ => false,
        }
    }
}

/// The ledger an import is compared with.
pub struct ImportLedger<'a> {
    pub entries: Vec<&'a Entry>,
    /// The text of a ledger file.
    pub text: &'a dyn Fn(&str) -> Option<&'a str>,
    /// What the ledger opens, and whether it closes it.
    pub accounts: HashMap<String, bool>,
}

/// The lines of entries, reading each file's text once.
struct LinesOf<'a> {
    text: &'a dyn Fn(&str) -> Option<&'a str>,
    cache: HashMap<String, Vec<String>>,
}

impl LinesOf<'_> {
    fn of(&mut self, e: &Entry) -> Vec<String> {
        let text = self.text;
        let l = self
            .cache
            .entry(e.file.to_string())
            .or_insert_with(|| split_lines(text(&e.file).unwrap_or("")).lines);
        let from = (e.line as usize).saturating_sub(1).min(l.len());
        let to = (e.end_line as usize).clamp(from, l.len());
        l[from..to].to_vec()
    }
}

/// Compares imported files with the ledger's entries.
pub fn preview_import(files: &IndexMap<String, String>, ledger: &ImportLedger) -> ImportPreview {
    let Some(root) = import_root(files) else {
        let message = "Choose one ledger file, or main.beancount and the files it includes.";
        return ImportPreview {
            errors: vec![Diagnostic::new(&"".into(), 0, "import", message)],
            ..Default::default()
        };
    };
    let source = MemorySource(files.iter().map(|(k, v)| (k.clone(), v.clone())).collect());
    let loaded = load(&root, &source, None);
    let mut ignored: Vec<String> = loaded
        .options
        .keys()
        .map(|k| format!("option \"{k}\""))
        .collect();
    ignored.extend(loaded.plugins.iter().map(|p| format!("plugin {:?}", p.key)));
    if !loaded.errors.is_empty() {
        return ImportPreview {
            root,
            ignored,
            errors: loaded.errors,
            ..Default::default()
        };
    }

    let mut ledger_lines = LinesOf {
        text: ledger.text,
        cache: HashMap::new(),
    };
    let mut have = Counter::default();
    for e in &ledger.entries {
        if e.open().is_none() && e.close().is_none() {
            have.add(key_of(e, &ledger_lines.of(e)));
        }
    }

    let file_text = |p: &str| files.get(p).map(String::as_str);
    let mut import_lines = LinesOf {
        text: &file_text,
        cache: HashMap::new(),
    };
    let mut add = Vec::new();
    let mut duplicates = 0;
    let mut opened = HashSet::new();
    for e in &loaded.entries {
        let lines = import_lines.of(e);
        let known = match &e.body {
            Body::Open(o) => ledger.accounts.contains_key(&o.account),
            Body::Close(c) => ledger.accounts.get(&c.account) == Some(&true),
            _ => have.take(&key_of(e, &lines)),
        };
        if known {
            duplicates += 1;
            continue;
        }
        let account = match &e.body {
            Body::Open(o) => {
                opened.insert(o.account.clone());
                Some(o.account.clone())
            }
            Body::Close(c) => Some(c.account.clone()),
            _ => None,
        };
        add.push(ImportItem {
            kind: kind_of(e),
            date: e.date,
            lines,
            account,
        });
    }

    // Accounts the new transactions use that nobody opens: open them on the day they are first
    // used.
    let mut first_use: HashMap<&str, Date> = HashMap::new();
    for e in &loaded.entries {
        let Some(t) = e.txn() else { continue };
        for p in &t.postings {
            if ledger.accounts.contains_key(&p.account) || opened.contains(&p.account) {
                continue;
            }
            let d = first_use.entry(&p.account).or_insert(e.date);
            *d = (*d).min(e.date);
        }
    }
    let mut opens: Vec<String> = first_use.keys().map(|a| (*a).to_owned()).collect();
    opens.sort();
    for a in &opens {
        let date = first_use[a.as_str()];
        add.push(ImportItem {
            kind: ImportKind::Open,
            date,
            lines: vec![format!("{date} open {a}")],
            account: Some(a.clone()),
        });
    }

    add.sort_by_key(|a| a.date);
    ImportPreview {
        root,
        add,
        duplicates,
        opens,
        ignored,
        errors: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(list: &[(&str, &str)]) -> IndexMap<String, String> {
        list.iter()
            .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
            .collect()
    }

    #[test]
    fn roots() {
        assert_eq!(
            import_root(&files(&[("a.bean", "x"), ("main.beancount", "")])).as_deref(),
            Some("main.beancount")
        );
        assert_eq!(
            import_root(&files(&[
                ("a.bean", "x"),
                ("b.bean", "; c\ninclude \"a.bean\"")
            ]))
            .as_deref(),
            Some("b.bean")
        );
        assert_eq!(
            import_root(&files(&[("a.bean", "x"), ("b.bean", "includes")])),
            None
        );
        assert_eq!(
            import_root(&files(&[("only.bean", "")])).as_deref(),
            Some("only.bean")
        );
    }
}
