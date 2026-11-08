//! Where a book is kept: a git repository in the app's folder, one per book, as the core's
//! `Store`.
//!
//! shortcut: saves and reloads run git on the UI thread, so each stays atomic. Move them to a
//! background thread, with a lock around plan, commit and keep, once a save takes long enough to feel.

use std::path::{Path, PathBuf};

use branchbooks_core::date::Date;
use branchbooks_core::write::store::{CommitInfo, FileChange, Snapshot, Store, StoreError};
use branchbooks_git::{Author, GitStore};

/// A book in a git repository on this device, committed to as `author`; on scenario `scenario`'s
/// branch when there is one, else on the book itself (trunk).
#[derive(Clone, Debug, PartialEq)]
pub struct Backend {
    pub dir: PathBuf,
    pub author: Author,
    pub scenario: Option<String>,
}

impl Backend {
    /// The scenario open, if any.
    pub fn scenario(&self) -> Option<&str> {
        self.scenario.as_deref()
    }

    fn git(&self) -> Result<GitStore, StoreError> {
        let trunk = GitStore::new(&self.dir, Some(self.author.clone()));
        match &self.scenario {
            None => Ok(trunk),
            Some(name) => trunk.scenario(name).ok_or_else(|| {
                StoreError::Refused(format!(
                    "there is no scenario {name}: it was merged or set aside"
                ))
            }),
        }
    }

    pub fn snapshot(&self) -> Result<Snapshot, StoreError> {
        self.git()?.snapshot()
    }

    /// The book's folder.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The latest `limit` commits, newest first.
    pub fn log(&self, limit: usize) -> Result<Vec<branchbooks_git::GitCommit>, StoreError> {
        self.git()?.log(limit)
    }

    /// Commits on top of `base`; `StoreError::Conflict` if the book has moved on.
    pub fn commit(
        &self,
        base: &str,
        changes: &[FileChange],
        info: &CommitInfo,
    ) -> Result<String, StoreError> {
        self.git()?.commit_as(base, changes, info, None)
    }
}

/// Where this device keeps its books, one git repository each: `%APPDATA%\BranchBooks\books` on
/// Windows, the app's own folder on Android, `~/.local/share/branchbooks/books` elsewhere.
pub fn books_dir() -> PathBuf {
    if cfg!(target_os = "android") {
        // shortcut: the main user's folder, and Dioxus.toml's identifier repeated; ask Android.
        return PathBuf::from("/data/data/dev.branchbooks.app/files/books");
    }
    std::env::var_os("APPDATA")
        .map(|d| PathBuf::from(d).join("BranchBooks"))
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/branchbooks"))
        })
        .unwrap_or_else(|| PathBuf::from("branchbooks"))
        .join("books")
}

/// Who commits to the book in `dir`: git's identity (the book's own config, which Book settings
/// sets, then the user's global ~/.gitconfig), else the signed-in user at this computer.
pub fn device_author(dir: &Path) -> Author {
    if let Some(a) = GitStore::new(dir, None).configured_author() {
        return a;
    }
    let var = |keys: [&str; 2], or: &str| {
        keys.iter()
            .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
            .unwrap_or_else(|| or.to_owned())
    };
    let name = var(["USERNAME", "USER"], "BranchBooks");
    let host = var(["COMPUTERNAME", "HOSTNAME"], "localhost");
    Author {
        email: format!("{}@{}", name.to_lowercase(), host.to_lowercase()),
        name,
    }
}

/// A new Op-Id.
pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Today's date where the person is.
pub fn local_today() -> Date {
    chrono::Local::now()
        .format("%Y-%m-%d")
        .to_string()
        .parse()
        .expect("chrono writes real dates")
}
