//! The books on this device: one git repository each in `books_dir()`, the folder named by the
//! book's id, its name in the repository's `description` file. A book cloned from a remote syncs
//! with it; the remote's token is kept in the system's credential store, never in a file.

use std::path::{Path, PathBuf};

use branchbooks_core::book::Book as Ledger;
use branchbooks_core::date::Month;
use branchbooks_core::diff::{BudgetDiff, diff_books};
use branchbooks_core::loader::MemorySource;
use branchbooks_core::write::render::is_currency;
use branchbooks_core::write::store::{CommitInfo, FileChange, Store, StoreError};
use branchbooks_core::write::workspace::new_errors;
use branchbooks_git::{
    Close, GitStore, Merged, Scenario, Synced, book_id, changes_between, is_scenario_name,
    valid_book_id,
};

use crate::store::{Backend, books_dir, device_author, new_id};

/// The ledger's main file, as the workspace reads it.
const ROOT: &str = "main.beancount";
const SAMPLE: &str = include_str!("../../../fixtures/examples-1-5.bean");

#[derive(Clone, Debug, PartialEq)]
pub struct Book {
    pub id: String,
    pub name: String,
    /// It has a remote to sync with.
    pub remote: bool,
}

/// How a new book starts.
#[derive(Clone, Debug, PartialEq)]
pub enum Start {
    /// No entries yet, in this currency.
    Empty {
        currency: String,
    },
    Sample,
    /// A clone of `url`; `token` is the password for HTTPS, kept for later syncs.
    Clone {
        url: String,
        token: String,
    },
    /// A `GnuCash` book: the file's bytes (compressed XML, as `GnuCash` saves it), full history.
    GnuCash(Vec<u8>),
}

fn path(id: &str) -> PathBuf {
    books_dir().join(id)
}

/// The book `id`, or scenario `scenario` in it.
pub fn backend(id: &str, scenario: Option<&str>) -> Backend {
    Backend {
        dir: path(id),
        author: device_author(&path(id)),
        scenario: scenario.map(str::to_owned),
    }
}

/// Why something asked of a book did not happen. Each says so in words for the person.
#[derive(Debug)]
pub enum BookError {
    /// Refused, and the message says what to do.
    Refused(String),
    Store(StoreError),
    Io(std::io::Error),
    Import(branchbooks_gnucash::ConvertError),
}

impl std::fmt::Display for BookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BookError::Refused(m) => f.write_str(m),
            BookError::Store(e) => e.fmt(f),
            BookError::Io(e) => e.fmt(f),
            BookError::Import(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for BookError {}

impl From<StoreError> for BookError {
    fn from(e: StoreError) -> Self {
        BookError::Store(e)
    }
}

impl From<std::io::Error> for BookError {
    fn from(e: std::io::Error) -> Self {
        BookError::Io(e)
    }
}

impl From<branchbooks_gnucash::ConvertError> for BookError {
    fn from(e: branchbooks_gnucash::ConvertError) -> Self {
        BookError::Import(e)
    }
}

fn refused(message: impl Into<String>) -> BookError {
    BookError::Refused(message.into())
}

/// Every book on this device, by name; none on a new device, where the app asks for the first.
pub fn list() -> Result<Vec<Book>, BookError> {
    Ok(read(&books_dir()))
}

/// Whether this device can keep a token for later syncs. Only Windows can so far.
pub fn keeps_tokens() -> bool {
    cfg!(windows)
}

/// A name for a book cloned from `url`: the repository's, `book` from `https://host/you/book.git`.
pub fn name_from_url(url: &str) -> String {
    let path = url.trim().trim_end_matches(['/', '\\']);
    let last = path.rsplit(['/', '\\', ':']).next().unwrap_or_default();
    last.strip_suffix(".git").unwrap_or(last).trim().to_owned()
}

fn read(dir: &Path) -> Vec<Book> {
    let mut books: Vec<Book> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let id = e.file_name().into_string().ok()?;
            let git = e.path().join(".git");
            if !valid_book_id(&id) || !git.exists() {
                return None;
            }
            let name = std::fs::read_to_string(git.join("description"))
                .ok()
                .map(|t| t.trim().to_owned())
                // git's own placeholder, in a repository made before books had names.
                .filter(|t| !t.is_empty() && !t.starts_with("Unnamed repository"))
                .unwrap_or_else(|| id.clone());
            let remote = GitStore::new(e.path(), None).has_remote();
            Some(Book { id, name, remote })
        })
        .collect();
    books.sort_by_key(|b| (b.name.to_lowercase(), b.id.clone()));
    books
}

/// Makes a book named `name`; returns its id and what was done, to tell the person. A clone that
/// fails leaves nothing behind, and one given no name takes the repository's.
pub fn create(name: &str, start: Start) -> Result<(String, String), BookError> {
    create_in(&books_dir(), name, start)
}

fn create_in(dir: &Path, name: &str, start: Start) -> Result<(String, String), BookError> {
    let named = match &start {
        Start::Clone { url, .. } if name.trim().is_empty() => name_from_url(url),
        _ => name.trim().to_owned(),
    };
    let name = named.as_str();
    if name.is_empty() {
        return Err(refused("Give the book a name."));
    }

    std::fs::create_dir_all(dir)?;
    let taken: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    let id = book_id(name, taken.iter().map(String::as_str));
    let at = dir.join(&id);
    let root = |text: String| vec![(ROOT.to_owned(), text)];
    let (ledger, said) = match &start {
        Start::Clone { url, token } => {
            check_url(url, token)?;
            // Before cloning, so a token this device cannot keep leaves no book that cannot sync.
            if !token.is_empty() && !keeps_tokens() {
                return Err(refused(
                    "This device cannot keep a token yet: leave it empty to copy a public remote.",
                ));
            }
            let token = Some(token.as_str()).filter(|t| !t.is_empty());
            if let Err(e) = GitStore::clone_from(url.trim(), &at, token, None) {
                // Only the folder this call just made: the id was free.
                let _ = std::fs::remove_dir_all(&at);
                return Err(e.into());
            }
            (None, format!("Cloned {name}."))
        }
        Start::Empty { currency } => {
            if !is_currency(currency) {
                return Err(refused(format!(
                    "{currency:?} is not a currency, such as USD or EUR."
                )));
            }
            let text = format!("option \"operating_currency\" \"{currency}\"\n");
            (Some(root(text)), format!("Started {name}."))
        }
        Start::Sample => (Some(root(SAMPLE.to_owned())), format!("Started {name}.")),
        Start::GnuCash(bytes) => {
            let c = branchbooks_gnucash::convert(bytes)?;
            let mut said = format!(
                "Imported {} accounts and {} transactions from GnuCash.",
                c.accounts, c.transactions
            );
            for n in &c.notes {
                said.push_str(&format!(" Note: {n}."));
            }
            (Some(c.files.into_iter().collect()), said)
        }
    };
    if let Some(files) = ledger {
        std::fs::create_dir_all(&at)?;
        GitStore::init(&at, None)?;
        let store = GitStore::new(&at, Some(device_author(&at)));
        let changes: Vec<FileChange> = files
            .into_iter()
            .map(|(path, text)| FileChange {
                path,
                text: Some(text),
            })
            .collect();
        let info = CommitInfo {
            op_id: new_id(),
            op_type: "import".into(),
            summary: if matches!(start, Start::GnuCash(_)) {
                format!("Import the book {name} from GnuCash")
            } else {
                format!("Start the book {name}")
            },
        };
        store.commit_as("", &changes, &info, None)?;
    }
    std::fs::write(at.join(".git/description"), format!("{name}\n"))?;
    if let Start::Clone { token, .. } = &start
        && !token.is_empty()
    {
        save_token(&id, token)
            .map_err(|e| refused(format!("Cloned, but the token was not saved: {e}")))?;
    }
    Ok((id, said))
}

/// Syncs the book with its remote; true when its files changed here.
/// Then scenarios, closed months and what was set aside; anything kept as it is here because it
/// changed on both sides is added to the message.
pub fn sync(id: &str) -> Result<(bool, String), BookError> {
    let store = GitStore::new(path(id), Some(device_author(&path(id))));
    let token = token(id);
    let (changed, said) = match store.sync(ROOT, token.as_deref())? {
        Synced::UpToDate => (false, "Up to date."),
        Synced::Pushed => (false, "Sent this device's changes."),
        Synced::Pulled => (true, "Brought in changes from the remote."),
        Synced::Merged => (true, "Merged the remote's changes with this device's."),
    };
    let mut said = said.to_owned();
    match store.sync_refs(token.as_deref()) {
        Ok(notes) => {
            for n in notes {
                said.push_str(&format!(" Note: {n}."));
            }
        }
        Err(e) => said.push_str(&format!(" Scenarios and closes not synced: {e}.")),
    }
    Ok((changed, said))
}

// ---- Scenarios and closed months: git branches and tags in the book's repository.

fn trunk(id: &str) -> GitStore {
    trunk_at(&path(id))
}

fn trunk_at(dir: &Path) -> GitStore {
    GitStore::new(dir, Some(device_author(dir)))
}

pub fn scenarios(id: &str) -> Result<Vec<Scenario>, BookError> {
    Ok(trunk(id).scenarios()?)
}

/// Starts scenario `name` from the book as it is now.
pub fn start_scenario(id: &str, name: &str) -> Result<(), BookError> {
    let name = name.trim();
    if !is_scenario_name(name) {
        return Err(refused(
            "A scenario's name is lower-case letters, digits and dashes, such as rent-2027.",
        ));
    }
    let store = trunk(id);
    store.fork(name, &store.head())?;
    Ok(())
}

/// Sets scenario `name` aside (kept in the book's history, gone from the list).
pub fn archive_scenario(id: &str, name: &str) -> Result<(), BookError> {
    Ok(trunk(id).archive(name)?)
}

/// Merges scenario `name` into the book as one commit, then sets it aside. Refused when the book
/// and the scenario changed the same lines, or when the merge would add ledger errors.
pub fn merge_scenario(id: &str, name: &str) -> Result<String, BookError> {
    merge_at(&path(id), name)
}

fn merge_at(dir: &Path, name: &str) -> Result<String, BookError> {
    let store = trunk_at(dir);
    let (base, files) = match store.merged(name)? {
        Merged::Clean { base, files } => (base, files),
        Merged::Conflicts(files) => {
            return Err(refused(format!(
                "The book and {name} both changed the same lines in {}: change one of them first.",
                files.join(", ")
            )));
        }
    };
    let snap = store.snapshot()?;
    let changes = changes_between(&snap.files, &files);
    if !changes.is_empty() {
        let errors = new_errors(ROOT, &snap.files, &changes);
        if let Some(first) = errors.first() {
            return Err(refused(format!(
                "Merging {name} would add {} ledger error(s): {}",
                errors.len(),
                first.message
            )));
        }
        let info = CommitInfo {
            op_id: new_id(),
            op_type: "merge-scenario".into(),
            summary: format!("Merge scenario {name}"),
        };
        store.commit_as(&base, &changes, &info, None)?;
    }
    store.archive(name)?;
    Ok(if changes.is_empty() {
        format!("{name} had nothing new; set it aside.")
    } else {
        format!("Merged {name} into the book.")
    })
}

/// How scenario `name` differs from the book in the months `from` to `to`.
pub fn compare(id: &str, name: &str, from: Month, to: Month) -> Result<BudgetDiff, BookError> {
    let store = trunk(id);
    let branch = store
        .scenario(name)
        .ok_or_else(|| refused(format!("There is no scenario {name}.")))?;
    let load = |s: &GitStore| -> Result<Ledger, BookError> {
        let snap = s.snapshot()?;
        let files: Vec<(&str, &str)> = snap
            .files
            .iter()
            .map(|(p, t)| (p.as_str(), t.as_str()))
            .collect();
        Ok(Ledger::load(ROOT, &MemorySource::new(files)))
    };
    Ok(diff_books(
        &mut load(&store)?,
        &mut load(&branch)?,
        from,
        to,
    ))
}

/// Makes this book the remote's, for a book whose history is separate from the remote's; the
/// remote's old book stays in trunk's history, its scenarios and closes under refs/tags/archive/.
pub fn replace_remote(id: &str) -> Result<(), BookError> {
    Ok(trunk(id).replace_remote(token(id).as_deref())?)
}

pub fn closes(id: &str) -> Vec<Close> {
    trunk(id).closes().unwrap_or_default()
}

/// Closes `month` on the book as it is now, as this device's author.
pub fn close_month(id: &str, month: Month) -> Result<(), BookError> {
    let who = device_author(&path(id));
    trunk(id).close(month, Some(&who))?;
    Ok(())
}

pub fn reopen_month(id: &str, month: Month) -> Result<(), BookError> {
    Ok(trunk(id).reopen(month)?)
}
/// Whether a web address carries a login (`https://user:token@host/...`).
fn has_login(url: &str) -> bool {
    url.trim().split_once("://").is_some_and(|(scheme, rest)| {
        (scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http"))
            && rest
                .split('/')
                .next()
                .is_some_and(|host| host.contains('@'))
    })
}

/// A token goes only over HTTPS; a plain-http URL with one is refused. A URL is saved as typed, in
/// the book's git config, so one holding a login is refused too: tokens never go in a file.
fn check_url(url: &str, token: &str) -> Result<(), BookError> {
    if has_login(url) {
        return Err(refused(
            "Put the token in the token field, not in the URL: a URL is saved in a plain file.",
        ));
    }
    if !token.is_empty() && !url.trim().starts_with("https://") {
        return Err(refused(
            "A token is sent only over HTTPS: the URL must start https://.",
        ));
    }
    Ok(())
}

/// What Settings shows for a book.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub name: String,
    pub remote: Option<String>,
    /// Who commits to it: git's identity for the book, else this device's guess.
    pub author_name: String,
    pub author_email: String,
}

pub fn settings(id: &str) -> Settings {
    let store = GitStore::new(path(id), None);
    let who = device_author(&path(id));
    Settings {
        name: read(&books_dir())
            .into_iter()
            .find(|b| b.id == id)
            .map_or_else(|| id.to_owned(), |b| b.name),
        remote: store.remote_url(),
        author_name: who.name,
        author_email: who.email,
    }
}

pub fn rename(id: &str, name: &str) -> Result<(), BookError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(refused("Give the book a name."));
    }
    Ok(std::fs::write(
        path(id).join(".git/description"),
        format!("{name}\n"),
    )?)
}

/// Who commits to this book, in its own git config; both blank: git's global identity again.
pub fn set_author(id: &str, name: &str, email: &str) -> Result<(), BookError> {
    let (name, email) = (name.trim(), email.trim());
    if name.is_empty() != email.is_empty() {
        return Err(refused("Give both a name and an email, or neither."));
    }
    let store = GitStore::new(path(id), None);
    let set = !name.is_empty();
    store.set_config("user.name", set.then_some(name))?;
    store.set_config("user.email", set.then_some(email))?;
    Ok(())
}

/// Gives the book a remote, or removes it (`url` blank). The next sync pushes the book there. A blank
/// token keeps the one saved only while the address stays the same: a saved token never goes to a
/// new address.
pub fn set_remote(id: &str, url: &str, token: &str) -> Result<(), BookError> {
    let store = GitStore::new(path(id), None);
    let url = url.trim();
    if url.is_empty() {
        delete_token(id);
        return Ok(store.set_remote(None)?);
    }
    check_url(url, token)?;
    if !token.is_empty() {
        save_token(id, token)?;
    } else if store.remote_url().as_deref() != Some(url) {
        delete_token(id);
    }
    Ok(store.set_remote(Some(url))?)
}

/// Moves the book to `.deleted` in the books folder, so it can be restored by hand, and forgets
/// its token.
pub fn delete(id: &str) -> Result<(), BookError> {
    let trash = books_dir().join(".deleted");
    std::fs::create_dir_all(&trash)?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    std::fs::rename(path(id), trash.join(format!("{id}-{stamp}")))?;
    delete_token(id);
    Ok(())
}

#[cfg(windows)]
fn entry(id: &str) -> Result<keyring_core::Entry, BookError> {
    static STORE: std::sync::Once = std::sync::Once::new();
    STORE.call_once(|| {
        if let Ok(s) = windows_native_keyring_store::Store::new() {
            keyring_core::set_default_store(s);
        }
    });
    keyring_core::Entry::new("BranchBooks", id).map_err(|e| refused(e.to_string()))
}

/// Keeps the book's token in Windows Credential Manager.
#[cfg(windows)]
fn save_token(id: &str, token: &str) -> Result<(), BookError> {
    entry(id)?
        .set_password(token)
        .map_err(|e| refused(e.to_string()))
}

#[cfg(windows)]
fn token(id: &str) -> Option<String> {
    entry(id).ok()?.get_password().ok()
}

#[cfg(windows)]
fn delete_token(id: &str) {
    if let Ok(e) = entry(id) {
        let _ = e.delete_credential();
    }
}

// shortcut: tokens are kept on Windows only; add Android's keystore when its app syncs.
#[cfg(not(windows))]
fn save_token(_id: &str, _token: &str) -> Result<(), BookError> {
    Err(refused("tokens are kept only on Windows so far"))
}

#[cfg(not(windows))]
fn token(_id: &str) -> Option<String> {
    None
}

#[cfg(not(windows))]
fn delete_token(_id: &str) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn usd() -> Start {
        Start::Empty {
            currency: "USD".into(),
        }
    }

    #[test]
    fn a_url_holding_a_login_is_refused_and_leaves_nothing_behind() {
        for url in [
            "https://user:secret@example.invalid/b.git",
            "https://secret@example.invalid/b.git",
            "HTTP://user:secret@example.invalid/b.git",
        ] {
            let e = check_url(url, "").unwrap_err().to_string();
            assert!(e.contains("token field"), "{url}: {e}");
            assert!(!e.contains("secret"), "{e}");
        }
        // An @ further along is not a login, and neither is a folder.
        assert!(check_url("https://example.invalid/you@work/b.git", "").is_ok());
        assert!(check_url("/a/local/path", "").is_ok());

        let dir = std::env::temp_dir().join(format!("branchbooks-login-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let leaky = Start::Clone {
            url: "https://user:secret@example.invalid/b.git".into(),
            token: String::new(),
        };
        assert!(create_in(&dir, "Leaky", leaky).is_err());
        assert!(!dir.join("leaky").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn books_are_made_named_listed_and_cloned() {
        let dir = std::env::temp_dir().join(format!("branchbooks-books-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            create_in(&dir, "Rental", Start::Sample).unwrap().0,
            "rental"
        );
        assert_eq!(create_in(&dir, "rental", usd()).unwrap().0, "rental-2");
        assert!(create_in(&dir, " ", usd()).is_err());
        let bad = Start::Empty {
            currency: "us dollars".into(),
        };
        assert!(
            create_in(&dir, "Odd", bad)
                .unwrap_err()
                .to_string()
                .contains("not a currency")
        );

        // A token never goes over plain HTTP, and the refused clone leaves no folder.
        let http = Start::Clone {
            url: "http://example.invalid/b.git".into(),
            token: "t".into(),
        };
        assert!(
            create_in(&dir, "Leaky", http)
                .unwrap_err()
                .to_string()
                .contains("HTTPS")
        );
        let bad = Start::Clone {
            url: dir.join("missing").display().to_string(),
            token: String::new(),
        };
        assert!(create_in(&dir, "Gone", bad).is_err());
        assert!(!dir.join("gone").exists() && !dir.join("leaky").exists());

        let copy = Start::Clone {
            url: dir.join("rental").display().to_string(),
            token: String::new(),
        };
        assert_eq!(create_in(&dir, "Copy", copy).unwrap().0, "copy");
        let books = read(&dir);
        let seen: Vec<(&str, &str, bool)> = books
            .iter()
            .map(|b| (b.id.as_str(), b.name.as_str(), b.remote))
            .collect();
        assert_eq!(
            seen,
            [
                ("copy", "Copy", true),
                ("rental", "Rental", false),
                ("rental-2", "rental", false)
            ]
        );
        let ledger = std::fs::read_to_string(dir.join("copy").join(ROOT)).unwrap();
        assert_eq!(ledger, SAMPLE);

        // Another device opening the same book needs no name: it takes the repository's.
        let unnamed = Start::Clone {
            url: format!("{}/", dir.join("rental").display()),
            token: String::new(),
        };
        let (id, said) = create_in(&dir, " ", unnamed).unwrap();
        assert_eq!((id.as_str(), said.as_str()), ("rental-3", "Cloned rental."));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_clone_is_named_after_its_repository() {
        for (url, name) in [
            ("https://git.example/you/Household.git", "Household"),
            ("https://git.example/you/household/", "household"),
            ("https://git.example/you/book", "book"),
            (r"D:\books\rental", "rental"),
            ("", ""),
        ] {
            assert_eq!(name_from_url(url), name, "{url}");
        }
    }

    #[test]
    fn a_token_this_device_cannot_keep_is_refused_before_cloning() {
        if keeps_tokens() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("branchbooks-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let with_token = Start::Clone {
            url: "https://example.invalid/b.git".into(),
            token: "t".into(),
        };
        let e = create_in(&dir, "B", with_token).unwrap_err().to_string();
        assert!(e.contains("cannot keep a token"), "{e}");
        assert!(!dir.join("b").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod scenario_tests {
    use super::*;
    use branchbooks_core::write::store::Store;

    fn info(op: &str) -> CommitInfo {
        CommitInfo {
            op_id: op.into(),
            op_type: "test".into(),
            summary: op.into(),
        }
    }

    /// Appends `text` to main.beancount on `store` in one commit.
    fn append(store: &GitStore, text: &str, op: &str) {
        let snap = store.snapshot().unwrap();
        let change = FileChange {
            path: ROOT.into(),
            text: Some(format!("{}{text}", snap.files[ROOT])),
        };
        store
            .commit_as(&snap.version, &[change], &info(op), None)
            .unwrap();
    }

    #[test]
    fn a_scenario_merges_into_the_book_once_and_conflicts_are_refused() {
        let dir =
            std::env::temp_dir().join(format!("branchbooks-scenarios-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let id = create_in(&dir, "Plan", Start::Sample).unwrap().0;
        let at = dir.join(&id);
        let book = trunk_at(&at);
        book.fork("rent", &book.head()).unwrap();
        let assign = "\n2026-11-03 custom \"envelope-assign\" Expenses:Housing:Rent 100.00 USD\n";
        append(&book.scenario("rent").unwrap(), assign, "s-1");
        assert_eq!(merge_at(&at, "rent").unwrap(), "Merged rent into the book.");
        assert!(book.snapshot().unwrap().files[ROOT].contains("Rent 100.00 USD"));
        assert!(book.scenario("rent").is_none(), "set aside after merging");

        // Both change the same lines: refused, nothing merged.
        book.fork("clash", &book.head()).unwrap();
        append(
            &book.scenario("clash").unwrap(),
            "; the scenario's\n",
            "s-2",
        );
        append(&book, "; the book's\n", "t-1");
        let err = merge_at(&at, "clash").unwrap_err().to_string();
        assert!(err.contains("both changed the same lines"), "{err}");
        assert!(book.scenario("clash").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
