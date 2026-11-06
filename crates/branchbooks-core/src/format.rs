//! The book format: which versions of `BranchBooks` can change a book. A book records the lowest
//! format that reads everything it holds, in its root file:
//!
//! ```beancount
//! ; Book format 2: BranchBooks 0.2.0 or later can change this book; older versions open it read-only.
//! 1970-01-01 custom "branchbooks-format" 2 "0.2.0"
//! ```
//!
//! The number decides; the version is there for people, and for the message an older app shows.
//! An app that finds a number above [`FORMAT`] opens the book read-only, so it never misreads
//! entries a newer app wrote. A book with no such entry is format 1. The number is raised in the
//! same commit as the change that needs it, and never lowered.

use crate::types::{Body, CustomValue, Entry};
use crate::write::import::ImportKind;
use crate::write::render::EntryInput;

/// Each format after the first: its number, the first version of `BranchBooks` that reads it, and
/// what it adds. A new format goes at the end, with the version it is released in.
const FORMATS: [(u32, &str, &str); 1] = [(2, "0.2.0", "custom \"overspending\" entries")];

/// The highest format this build reads.
pub const FORMAT: u32 = FORMATS[FORMATS.len() - 1].0;

/// The `custom` entry's name.
pub const NAME: &str = "branchbooks-format";

/// The date a new format entry gets: before everything, so it reads as a fact about the book.
pub const DATE: &str = "1970-01-01";

/// The format of a book with no format entry.
const FIRST: u32 = 1;

/// The start of the comment line above the entry.
const COMMENT: &str = "; Book format ";

/// What a book says about its format: the number, and the version it names, if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub format: u32,
    pub version: Option<String>,
}

/// The first version of `BranchBooks` that reads `format`, and what the format adds.
fn about(format: u32) -> Option<(&'static str, &'static str)> {
    FORMATS
        .iter()
        .find(|(n, ..)| *n == format)
        .map(|(_, v, what)| (*v, *what))
}

/// The lowest format that reads `e`.
pub fn needed_by(e: &Entry) -> u32 {
    match &e.body {
        Body::Custom(c) if c.name == "overspending" => 2,
        _ => FIRST,
    }
}

/// The lowest format that reads an entry about to be written.
pub fn needed_by_input(e: &EntryInput) -> u32 {
    match e {
        EntryInput::Overspending(_) => 2,
        _ => FIRST,
    }
}

/// The lowest format that reads an imported entry of `kind`.
pub fn needed_by_import(kind: ImportKind) -> u32 {
    match kind {
        ImportKind::Overspending => 2,
        _ => FIRST,
    }
}

/// The lowest format that reads every entry.
pub fn needed<'a>(entries: impl IntoIterator<Item = &'a Entry>) -> u32 {
    entries.into_iter().map(needed_by).max().unwrap_or(FIRST)
}

/// What the book says: its highest format entry, else format 1. A first value that is not a whole
/// number counts as newer than any format, since only a newer app could mean it. Values after
/// the version are ignored, for a later format to use.
pub fn recorded<'a>(entries: impl IntoIterator<Item = &'a Entry>) -> Stamp {
    entries
        .into_iter()
        .filter_map(|e| match &e.body {
            Body::Custom(c) if c.name == NAME => Some(Stamp {
                format: match c.values.first() {
                    Some(CustomValue::Number(n)) => n.to_string().parse().unwrap_or(u32::MAX),
                    _ => u32::MAX,
                },
                version: match c.values.get(1) {
                    Some(CustomValue::String(v)) => Some(v.clone()),
                    _ => None,
                },
            }),
            _ => None,
        })
        .max_by_key(|s| s.format)
        .unwrap_or(Stamp {
            format: FIRST,
            version: None,
        })
}

/// The entry's lines for `format`, dated `date` (`YYYY-MM-DD`): a comment saying what it means,
/// then the entry.
pub fn lines(date: &str, format: u32) -> [String; 2] {
    let version = about(format).map_or("a newer version", |(v, _)| v);
    [
        format!(
            "{COMMENT}{format}: BranchBooks {version} or later can change this book; older \
             versions open it read-only."
        ),
        format!("{date} custom \"{NAME}\" {format} \"{version}\""),
    ]
}

/// Whether `line` is the comment [`lines`] writes.
pub fn is_comment(line: &str) -> bool {
    line.starts_with(COMMENT)
}

/// The paragraph for the commit that raises a book to `format`.
pub fn commit_note(format: u32) -> String {
    match about(format) {
        Some((version, what)) => format!(
            "Book format {format}: this book now holds {what}, so it needs BranchBooks {version} \
             or later to change it. Older versions open it read-only."
        ),
        None => format!("Book format {format}."),
    }
}

/// Why a book cannot be changed here, when its format is newer than this build reads.
pub fn too_new(stamp: &Stamp) -> Option<String> {
    let this = crate::VERSION;
    (stamp.format > FORMAT).then(|| match (&stamp.version, stamp.format) {
        (Some(v), _) => format!(
            "This book needs BranchBooks {v} or later; this is {this}. Update BranchBooks to \
             change it."
        ),
        (None, u32::MAX) => format!(
            "This book is in a newer book format; this is BranchBooks {this}. Update BranchBooks \
             to change it."
        ),
        (None, n) => format!(
            "This book is in book format {n}, from a newer BranchBooks; this is {this}. Update \
             BranchBooks to change it."
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn entries(text: &str) -> Vec<Entry> {
        let p = parse(text, "main.beancount");
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        p.entries
    }

    fn stamp(format: u32, version: Option<&str>) -> Stamp {
        Stamp {
            format,
            version: version.map(Into::into),
        }
    }

    #[test]
    fn formats_count_up_from_two() {
        for (i, (n, ..)) in FORMATS.iter().enumerate() {
            assert_eq!(*n as usize, i + 2);
        }
    }

    #[test]
    fn this_build_is_at_least_the_version_its_newest_format_names() {
        let parts = |v: &str| -> Vec<u32> { v.split('.').map(|p| p.parse().unwrap()).collect() };
        let (_, newest, _) = FORMATS[FORMATS.len() - 1];
        assert!(
            parts(crate::VERSION) >= parts(newest),
            "format {FORMAT} names {newest}, but this build is {}: bump the version",
            crate::VERSION
        );
    }

    #[test]
    fn a_book_needs_the_format_of_its_newest_entry() {
        let old = entries("2026-01-01 open Assets:Bank USD\n2026-01-01 custom \"budget\" \"x\"\n");
        assert_eq!(needed(&old), 1);
        let new = entries("2026-01-01 custom \"overspending\" Expenses:Food \"rollover\"\n");
        assert_eq!(needed(&new), 2);
    }

    #[test]
    fn the_written_entry_explains_itself_and_reads_back() {
        let [comment, entry] = lines(DATE, 2);
        assert_eq!(
            comment,
            "; Book format 2: BranchBooks 0.2.0 or later can change this book; older versions \
             open it read-only."
        );
        assert_eq!(
            entry,
            "1970-01-01 custom \"branchbooks-format\" 2 \"0.2.0\""
        );
        assert!(is_comment(&comment));
        let text = format!("{comment}\n{entry}\n");
        assert_eq!(recorded(&entries(&text)), stamp(2, Some("0.2.0")));
        assert!(commit_note(2).contains("needs BranchBooks 0.2.0 or later"));
    }

    #[test]
    fn the_recorded_format_is_the_highest_entry_and_one_without() {
        assert_eq!(
            recorded(&entries("2026-01-01 open Assets:Bank USD\n")),
            stamp(1, None)
        );
        let bare = entries("1970-01-01 custom \"branchbooks-format\" 3\n");
        assert_eq!(recorded(&bare), stamp(3, None));
        let more = entries("1970-01-01 custom \"branchbooks-format\" 4 \"0.9.0\" TRUE\n");
        assert_eq!(recorded(&more), stamp(4, Some("0.9.0")));
        let odd = entries("1970-01-01 custom \"branchbooks-format\" \"three\"\n");
        assert_eq!(recorded(&odd).format, u32::MAX);
        let frac = entries("1970-01-01 custom \"branchbooks-format\" 2.5\n");
        assert_eq!(recorded(&frac).format, u32::MAX);
    }

    #[test]
    fn only_a_newer_format_is_refused_and_named_by_its_version() {
        assert_eq!(too_new(&stamp(FORMAT, Some("0.2.0"))), None);
        let named = too_new(&stamp(FORMAT + 1, Some("9.0.0"))).unwrap();
        assert!(
            named.starts_with("This book needs BranchBooks 9.0.0 or later; this is "),
            "{named}"
        );
        assert!(
            too_new(&stamp(FORMAT + 1, None))
                .unwrap()
                .contains("book format 3")
        );
        assert!(
            too_new(&stamp(u32::MAX, None))
                .unwrap()
                .contains("a newer book format")
        );
    }
}
