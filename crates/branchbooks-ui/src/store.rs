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

/// Whether this is a dev build: a debug build, or one built with `BRANCHBOOKS_CHANNEL=dev`, which installs beside
/// the release with its own books.
fn dev() -> bool {
    cfg!(debug_assertions) || option_env!("BRANCHBOOKS_CHANNEL") == Some("dev")
}

/// Where this device keeps its books, one git repository each: `BRANCHBOOKS_BOOKS` when set, else
/// `%LOCALAPPDATA%\BranchBooks Data\books` on Windows (not roaming, and beside the install folder
/// rather than in it, so uninstalling never takes them unasked), the app's own folder on Android,
/// `~/.local/share/branchbooks/books` elsewhere. A dev build (`dev`) uses
/// `%LOCALAPPDATA%\BranchBooks-dev\books` (`branchbooks-dev` elsewhere), so trying a change never
/// touches real books.
pub fn books_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("BRANCHBOOKS_BOOKS").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    if cfg!(target_os = "android") {
        // shortcut: the main user's folder, and Dioxus.toml's identifier repeated; ask Android.
        return PathBuf::from("/data/data/dev.branchbooks.app/files/books");
    }
    let data = data_dir();
    // Books from before the move stay in the roaming folder while it could not be moved.
    match roaming_dir() {
        Some(old) if old.exists() && !data.exists() => old,
        _ => data,
    }
    .join("books")
}

/// The app's own folder: its books and this device's settings.
fn data_dir() -> PathBuf {
    let (windows, unix) = if dev() {
        ("BranchBooks-dev", ".local/share/branchbooks-dev")
    } else {
        ("BranchBooks Data", ".local/share/branchbooks")
    };
    std::env::var_os("LOCALAPPDATA")
        .map(|d| PathBuf::from(d).join(windows))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(unix)))
        .unwrap_or_else(|| PathBuf::from("branchbooks"))
}

/// Where a release kept its data before it moved out of the roaming profile.
fn roaming_dir() -> Option<PathBuf> {
    if dev() || std::env::var_os("BRANCHBOOKS_BOOKS").is_some() {
        return None;
    }
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("BranchBooks"))
}

/// Moves the books and settings out of the roaming profile, once, at startup: a rename, so nothing
/// is copied twice or half moved. When it cannot (`AppData` redirected to another drive, a file
/// held open), they stay where they were and `books_dir` keeps using them.
pub fn move_out_of_roaming() {
    if let Some(old) = roaming_dir() {
        move_once(&old, &data_dir());
    }
}

/// Renames `old` to `new` when only `old` exists.
fn move_once(old: &Path, new: &Path) {
    if old.exists() && !new.exists() {
        if let Some(parent) = new.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::rename(old, new);
    }
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

#[cfg(test)]
mod tests {
    #[test]
    fn the_data_folder_moves_once_and_never_over_another() {
        let root = std::env::temp_dir().join(format!("branchbooks-move-{}", std::process::id()));
        let (old, new) = (
            root.join("Roaming/BranchBooks"),
            root.join("Local/BranchBooks Data"),
        );
        std::fs::create_dir_all(old.join("books/a")).unwrap();
        super::move_once(&old, &new);
        assert!(new.join("books/a").exists() && !old.exists());
        // Already moved: an old folder that turns up again is left alone, not merged.
        std::fs::create_dir_all(old.join("books/b")).unwrap();
        super::move_once(&old, &new);
        assert!(old.join("books/b").exists() && !new.join("books/b").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_debug_build_never_opens_the_real_books() {
        if std::env::var_os("BRANCHBOOKS_BOOKS").is_some() {
            return;
        }
        let dir = super::books_dir();
        let dev = dir
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str());
        assert!(
            matches!(dev, Some("BranchBooks-dev" | "branchbooks-dev")),
            "{dir:?}"
        );
    }
}
