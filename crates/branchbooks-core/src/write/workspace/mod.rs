//! The write path. A workspace holds one version of a ledger. `plan` turns an operation into
//! minimal text edits and validates it; once the caller has committed the changes to its store,
//! `commit_planned` brings the Book up to date. Nothing outside the edited lines changes.
//!
//! `shift_lines` currently rewrites in-memory entry line numbers to match the text. A later
//! split of a UUID domain model from the text serializer would drop fingerprint-by-line matching.
//! Transactions and envelope entries are validated and applied incrementally; opens, closes and
//! new files reload the ledger.
//!
//! The store calls, Op-Id lookups and retries stay with the caller: [`Workspace`] here over any
//! [`Store`], or the app's own save loop.

use std::collections::HashMap;
use std::fmt;

use indexmap::IndexMap;

use crate::book::{Book, EntryId};
use crate::date::Date;
use crate::format;
use crate::ledger::type_rank;
use crate::loader::{LoadResult, MemorySource, ParseCache, dirname, join_path, load, seq_of};
use crate::money::parse_decimal;
use crate::parser::{IncludeLine, parse};
use crate::types::{Body, Diagnostic, Entry, EntryType};
use crate::write::import::{ImportItem, ImportKind, ImportLedger, ImportPreview, preview_import};
use crate::write::render::{
    BalanceInput, BudgetFlag, EntryInput, InputError, amount_text, is_account, parse_date,
    render_entry,
};
use crate::write::store::{CommitInfo, FileChange, Store, StoreError};
use crate::write::text::{
    Inserted, Lines, Separators, entry_lines, fingerprint, insert_block, join_lines, remove_lines,
    replace_lines, split_lines,
};

mod driver;
mod helpers;
mod importing;
mod place;
mod plan;
mod types;

pub use driver::*;
use helpers::*;
pub use helpers::{added, new_errors};
pub use types::*;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Shift {
    file: String,
    from: u32,
    delta: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Placed {
    file: String,
    line: u32,
    block: Vec<String>,
}

/// The text edits an operation makes, and what they do to line numbers.
struct Plan {
    op_type: String,
    summary: String,
    envelope: bool,
    target: Option<EntryId>,
    edits: IndexMap<String, Lines>,
    /// Applied in order to the entries already in the Book, after the target is removed.
    shifts: Vec<Shift>,
    placed: Option<Placed>,
    /// A file was created or an account directive changed: the fast path does not apply.
    structural: bool,
}

/// How a planned change reaches the Book once it is committed.
enum Apply {
    /// The candidate ledger, loaded in full.
    Full {
        book: Box<Book>,
        file_order: Vec<String>,
    },
    /// Changes to make to the current Book in place.
    Incremental {
        target: Option<EntryId>,
        shifts: Vec<Shift>,
        entry: Option<Box<Entry>>,
        from: Date,
        /// Blocked entries from `from` on, before the change.
        before: Vec<Diagnostic>,
        new_budget: Vec<Diagnostic>,
    },
}

/// A validated operation: the changes to commit, and what to do once they are committed.
pub struct Planned {
    pub op_type: String,
    pub summary: String,
    /// Files to write, in the order first edited.
    pub changes: Vec<FileChange>,
    /// Where the added or edited entry will be.
    pub entry: Option<EntryRef>,
    /// Consequences that do not block the operation. `commit_planned` returns them again with
    /// the line numbers they have after the change.
    pub warnings: Vec<Diagnostic>,
    files: HashMap<String, String>,
    apply: Apply,
}

/// Plans operations against one version of a ledger and the Book over it.
pub struct WorkspaceCore {
    pub root: String,
    cache: ParseCache,
    files: HashMap<String, String>,
    file_order: Vec<String>,
    current: Book,
    pub version: String,
}

/// Where an entry may go: an entry with its lines after the planned shifts.
#[derive(Clone)]
struct Peer {
    date: Date,
    ty: EntryType,
    line: u32,
    end_line: u32,
}

impl Peer {
    fn of(e: &Entry) -> Self {
        Peer {
            date: e.date,
            ty: e.entry_type(),
            line: e.line,
            end_line: e.end_line,
        }
    }

    /// Sorts before an entry of type `ty` on `date`, a tie included.
    fn before(&self, date: Date, ty: EntryType) -> bool {
        (self.date, type_rank(self.ty)) <= (date, type_rank(ty))
    }

    fn is_account(&self) -> bool {
        matches!(self.ty, EntryType::Open | EntryType::Close)
    }
}

/// `/^\s*;/`: a comment line.
fn is_comment(line: Option<&String>) -> bool {
    line.is_some_and(|l| l.trim_start().starts_with(';'))
}

impl WorkspaceCore {
    pub fn new(root: Option<&str>) -> Self {
        WorkspaceCore {
            root: root.unwrap_or("main.beancount").to_owned(),
            cache: ParseCache::new(),
            files: HashMap::new(),
            file_order: Vec::new(),
            current: Book::empty(Default::default()),
            version: String::new(),
        }
    }

    fn load_files(&mut self, files: &HashMap<String, String>) -> LoadResult {
        let source = MemorySource(files.clone());
        load(&self.root, &source, Some(&mut self.cache))
    }

    /// Loads a version of the ledger (from a store snapshot).
    pub fn reset(&mut self, version: &str, files: impl IntoIterator<Item = (String, String)>) {
        self.files = files.into_iter().collect();
        self.version = version.to_owned();
        let loaded = self.load_files(&self.files.clone());
        self.file_order = loaded.files.clone();
        self.current = Book::from_load(loaded);
    }

    pub fn book(&mut self) -> &mut Book {
        &mut self.current
    }

    pub fn text(&self, path: &str) -> Option<&str> {
        self.files.get(path).map(String::as_str)
    }

    /// Every entry in the current version, in the Book's order.
    pub fn entries(&self) -> impl Iterator<Item = (EntryId, &Entry)> {
        self.current.entries()
    }

    /// The format the book says it is (see [`mod@crate::format`]).
    pub fn format(&self) -> u32 {
        self.current.format().format
    }

    /// Why this book is read-only here: its format is newer than this build reads.
    pub fn read_only(&self) -> Option<String> {
        self.current.read_only()
    }

    fn check_format(&self) -> Result<()> {
        self.read_only().map_or(Ok(()), |message| {
            Err(OperationError::new(
                OperationErrorCode::Newer,
                message,
                Vec::new(),
            ))
        })
    }

    /// Raises the book's format entry to what the book will hold, at least `need`: rewrites the
    /// entry and its comment, or adds both at the end of the root file. Returns the new format,
    /// or None when the book already says enough.
    fn raise_format(&self, edits: &mut IndexMap<String, Lines>, need: u32) -> Option<u32> {
        let entries = || self.current.entries().map(|(_, e)| e);
        let need = need.max(format::needed(entries()));
        if need <= format::recorded(entries()).format {
            return None;
        }
        let existing =
            entries().find(|e| matches!(&e.body, Body::Custom(c) if c.name == format::NAME));
        if let Some(e) = existing {
            // Found by its text: other edits may have moved lines in the same file.
            let marker = format!("custom \"{}\"", format::NAME);
            let l = self.lines(edits, &e.file.clone());
            let at = l.lines.iter().position(|r| r.contains(&marker))?;
            let date = l.lines[at]
                .split_whitespace()
                .next()
                .unwrap_or(format::DATE);
            let [comment, entry] = format::lines(date, need);
            l.lines[at] = entry;
            match at.checked_sub(1) {
                Some(above) if format::is_comment(&l.lines[above]) => l.lines[above] = comment,
                _ => l.lines.insert(at, comment),
            }
        } else {
            let root = self.root.clone();
            let l = self.lines(edits, &root);
            if l.lines.last().is_some_and(|r| !r.trim().is_empty()) {
                l.lines.push(String::new());
            }
            l.lines.extend(format::lines(format::DATE, need));
        }
        Some(need)
    }

    /// Every transaction dated `from` to `to` (inclusive), booked or not, latest first, as
    /// (the entry as written, as booked): the same twice when it could not be booked. `account`
    /// is matched against the booked postings.
    pub fn transaction_rows(
        &mut self,
        from: Option<Date>,
        to: Option<Date>,
        account: Option<&str>,
    ) -> Vec<(Entry, Entry)> {
        let in_range = |d: Date| from.is_none_or(|f| d >= f) && to.is_none_or(|t| d <= t);
        let booked: HashMap<(String, u32), Entry> = self
            .current
            .transactions()
            .into_iter()
            .map(|t| ((t.file.to_string(), t.line), t.clone()))
            .collect();
        let mut rows: Vec<(Entry, Entry)> = self
            .entries()
            .map(|(_, e)| e)
            .filter(|e| e.entry_type() == EntryType::Txn && in_range(e.date))
            .map(|e| {
                let b = booked.get(&(e.file.to_string(), e.line)).unwrap_or(e);
                (e.clone(), b.clone())
            })
            .filter(|(_, b)| {
                account.is_none_or(|a| {
                    b.txn()
                        .is_some_and(|t| t.postings.iter().any(|p| p.account == a))
                })
            })
            .collect();
        rows.sort_by(|a, b| b.0.date.cmp(&a.0.date).then(b.0.seq.cmp(&a.0.seq)));
        rows
    }

    /// Every payee (or narration, when a transaction has no payee), once each, sorted.
    pub fn payees(&mut self) -> Vec<String> {
        let mut names: Vec<String> = self
            .current
            .transactions()
            .into_iter()
            .filter_map(|t| t.txn())
            .map(|t| t.payee.clone().unwrap_or_else(|| t.narration.clone()))
            .filter(|n| !n.is_empty())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    pub fn ref_of(&self, e: &Entry) -> EntryRef {
        let l = split_lines(self.text(&e.file).unwrap_or(""));
        EntryRef {
            file: e.file.to_string(),
            line: e.line,
            fingerprint: fingerprint(entry_lines(&l, e.line, e.end_line)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rename_takes_whole_names_and_sub_accounts_only() {
        let r = |l: &str| rename_in(l, "Expenses:Food", "Expenses:Meals");
        assert_eq!(
            r("  Expenses:Food:Dining  5.00 USD"),
            "  Expenses:Meals:Dining  5.00 USD"
        );
        assert_eq!(
            r("  category: \"Expenses:Food\""),
            "  category: \"Expenses:Meals\""
        );
        assert_eq!(r("open Expenses:Foodie USD"), "open Expenses:Foodie USD");
        assert_eq!(r("open Expenses:Food: USD"), "open Expenses:Food: USD");
        assert_eq!(r("Assets:Expenses:Food"), "Assets:Expenses:Food");
        assert_eq!(
            renamed("Expenses:Food:Dining", "Expenses:Food", "Expenses:Meals").as_deref(),
            Some("Expenses:Meals:Dining")
        );
        assert_eq!(
            renamed("Expenses:Foodie", "Expenses:Food", "Expenses:Meals"),
            None
        );
    }

    #[test]
    fn year_files() {
        assert_eq!(year_file("2026.beancount"), Some((2026, "beancount")));
        assert_eq!(year_file("x/2026.bean"), Some((2026, "bean")));
        assert_eq!(year_file("12026.bean"), None);
        assert_eq!(year_file("2026.txt"), None);
        assert_eq!(op_type_of("txn", "add"), "add-transaction");
        assert_eq!(op_type_of("open", "edit"), "edit-account-open");
        assert_eq!(op_type_of("assign", "delete"), "delete-assign");
        assert_eq!(op_type_of("custom", "add"), "add-custom");
    }
}
