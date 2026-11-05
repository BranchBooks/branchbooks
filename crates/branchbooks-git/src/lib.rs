//! Git backend for the versioned file store: one commit per operation, with Op-Id and Op-Type
//! trailers. It uses grit-lib, a git engine in Rust, so it needs no `git` program: the same code
//! runs in the CLI, on Windows and on Android, on any repository git can open.
//!
//! Commits are built from objects: blobs and a tree written to the object database, a commit,
//! then a compare-and-swap update of the branch, so a commit made by anyone else in between is a
//! conflict, never a lost update. In a non-bare repository the changed files are then written
//! into the working tree, and the index is updated for exactly those paths.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, HashSet};
use std::path::{Path, PathBuf};

use branchbooks_core::date::Month;
use branchbooks_core::write::store::{
    CommitInfo, CommitRecord, FileChange, Snapshot, Store, StoreError, commit_message,
    parse_trailers,
};
use branchbooks_core::write::workspace::new_errors;
use grit_lib::config::ConfigSet;
use grit_lib::diff::merge_trees::{
    DirectoryRenameMode, TreeMergeConflictPresentation, WhitespaceMergeOptions,
    merge_trees_three_way,
};
use grit_lib::index::write_tree::write_tree_from_index;
use grit_lib::index::{Index, IndexEntry, MODE_REGULAR};
use grit_lib::objects::{
    CommitData, ObjectId, ObjectKind, parse_commit, parse_tag, parse_tree, serialize_commit,
};
use grit_lib::progress::NeverCancel;
use grit_lib::refs;
use grit_lib::remote::clone::{CloneOptions, clone};
use grit_lib::remote::credentials::{Credential, CredentialProvider};
use grit_lib::remote::fetch::NoProgress;
use grit_lib::remote::push_report::PushRefStatus;
use grit_lib::remote::transfer::{FetchOptions, PushRefSpec};
use grit_lib::remote::transport::http::HttpClient;
use grit_lib::remote::transport::http::ureq_client::UreqHttpClient;
use grit_lib::remote::{HttpClientFactory, Remote, RemoteResult};
use grit_lib::repo::{InitOptions, Repository};
use grit_lib::revwalk::merge_base::{is_ancestor, merge_bases_all};
use grit_lib::revwalk::rev_parse::resolve_revision;
use indexmap::IndexMap;
use unicode_normalization::UnicodeNormalization;
mod commits;
mod history;
mod months;
mod scenarios;
mod store;
mod sync;

use store::*;
use sync::*;

/// Where scenario branches live: ordinary git branches, `scenarios/<name>`.
const SCENARIOS: &str = "refs/heads/scenarios/";
/// Where a pruned scenario goes, so its history is kept. Under `refs/tags/` because some git hosts
/// take pushes only to branches and tags.
const ARCHIVE: &str = "refs/tags/archive/scenarios/";
/// Where closed months live: annotated tags `close/<YYYY-MM>`.
const CLOSES: &str = "refs/tags/close/";
/// Where a reopened month's close goes, so the record stays.
const ARCHIVE_CLOSES: &str = "refs/tags/archive/closes/";
/// Everything set aside (scenarios and closes), as sync carries it.
const ARCHIVED: &str = "refs/tags/archive/";
/// Where sync fetches the remote's scenarios, closes and archive, to compare with this device's.
const THEIR_SCENARIOS: &str = "refs/remotes/origin/scenarios/";
const THEIR_CLOSES: &str = "refs/remotes/origin-closes/";
const THEIR_ARCHIVED: &str = "refs/remotes/origin-archive/";
/// A tree entry's mode for a directory.
const MODE_TREE: u32 = 0o40000;

/// A closed month: the tag `close/<month>` on trunk's version when it was closed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Close {
    pub month: Month,
    pub version: String,
    /// Who closed it, and when (ISO 8601).
    pub by: String,
    pub at: String,
}

/// A scenario name: lower-case letters, digits and dashes, up to 40, starting with a letter or
/// digit.
pub fn is_scenario_name(name: &str) -> bool {
    (1..=40).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
}

/// A book id: lower-case letters, digits and dashes, up to 40, starting and ending with a letter
/// or digit, so it is safe as a folder name.
pub fn valid_book_id(id: &str) -> bool {
    let b = id.as_bytes();
    let ok = |c: &u8| c.is_ascii_lowercase() || c.is_ascii_digit();
    (1..=40).contains(&b.len())
        && ok(&b[0])
        && ok(&b[b.len() - 1])
        && b.iter().all(|c| ok(c) || *c == b'-')
}

/// A book id made from its name: "Rental property" is rental-property; taken ids get -2, -3...
pub fn book_id<'a>(name: &str, taken: impl IntoIterator<Item = &'a str>) -> String {
    let mut slug = String::new();
    for c in name.to_lowercase().nfkd() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let trimmed = slug.trim_matches('-');
    let cut: String = trimmed.chars().take(32).collect();
    let base = match cut.trim_end_matches('-') {
        "" => "book".to_owned(),
        s => s.to_owned(),
    };
    let used: HashSet<&str> = taken.into_iter().collect();
    if !used.contains(base.as_str()) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|id| !used.contains(id.as_str()))
        .expect("some suffix is free")
}

/// A scenario branch: a what-if fork of trunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scenario {
    pub name: String,
    pub version: String,
    /// The trunk commit it forked from: where it and trunk last met.
    pub base: String,
}

/// A scenario merged with trunk, before anything is committed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Merged {
    /// Trunk's version, and the ledger files once merged.
    Clean {
        base: String,
        files: IndexMap<String, String>,
    },
    /// Files both changed in ways git cannot combine.
    Conflicts(Vec<String>),
}

/// The changes that turn the files `before` into `after`.
pub fn changes_between(
    before: &IndexMap<String, String>,
    after: &IndexMap<String, String>,
) -> Vec<FileChange> {
    let removed = before
        .keys()
        .filter(|p| !after.contains_key(*p))
        .map(|p| FileChange {
            path: p.clone(),
            text: None,
        });
    let changed = after
        .iter()
        .filter(|(p, t)| before.get(*p) != Some(*t))
        .map(|(p, t)| FileChange {
            path: p.clone(),
            text: Some(t.clone()),
        });
    removed.chain(changed).collect()
}

fn is_ledger_file(path: &str) -> bool {
    path.ends_with(".beancount") || path.ends_with(".bean")
}

/// A path inside the working tree on every system: no empty, `.` or `..` part, no part that names
/// `.git`, and no backslash or colon, which Windows reads as a separator or a drive. Windows also
/// drops trailing dots and spaces from a name, and knows `.git` by its short name `git~1`, as git's
/// own `core.protectNTFS` knows.
fn is_safe_path(path: &str) -> bool {
    !path.contains(['\\', ':'])
        && path.split('/').all(|p| {
            let name = p.trim_end_matches(['.', ' ']).to_ascii_lowercase();
            !name.is_empty() && name != ".git" && name != "git~1"
        })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Author {
    pub name: String,
    pub email: String,
}

/// A commit as `log` reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitCommit {
    pub version: String,
    pub summary: String,
    pub op_id: Option<String>,
    pub op_type: Option<String>,
    /// Commit date, ISO 8601.
    pub at: String,
    /// Author name: who made the change (the committer is the store's own identity).
    pub author: String,
}

pub struct GitStore {
    dir: PathBuf,
    /// Author and committer for operation commits; git's `user.name` and `user.email` when `None`.
    identity: Option<Author>,
    /// The ref this store reads and commits to: `HEAD` (trunk), or a scenario's branch.
    rev: String,
    /// Runs between the HEAD check and the ref update, so a test can commit in that gap.
    #[cfg(test)]
    race: std::sync::Mutex<Option<Box<dyn FnMut() + Send>>>,
}

fn failed(m: impl Into<String>) -> StoreError {
    StoreError::Failed(m.into())
}

/// Maps a grit-lib error to a store failure, naming what was being done.
fn g<E: std::fmt::Display>(what: &str) -> impl FnOnce(E) -> StoreError + '_ {
    move |e| failed(format!("{what}: {e}"))
}

/// Sets ref `name` to `new` (deletes it when `None`), only if it is now `old` (absent when
/// `None`): an atomic compare-and-swap under the ref's lock.
// shortcut: no reflog line (grit-lib wants a `time` timestamp); add one if a reflog is wanted.
fn update_ref(
    repo: &Repository,
    name: &str,
    new: Option<ObjectId>,
    old: Option<ObjectId>,
) -> grit_lib::error::Result<()> {
    refs::commit_ref_update_on_store(
        repo.refs(),
        refs::store::RefUpdate {
            name: name.to_owned(),
            new_value: new.map(refs::store::RawRef::Direct),
            expected: old.map_or(refs::store::Expected::Missing, refs::store::Expected::Oid),
            reflog: None,
            flags: Default::default(),
        },
    )
}

fn oid(hex: &str) -> Result<ObjectId, StoreError> {
    ObjectId::from_hex(hex).map_err(g("object id"))
}

/// A git identity line's name: everything before ` <`.
fn ident_name(ident: &str) -> &str {
    ident.split(" <").next().unwrap_or("")
}

/// A git identity line's date (`<seconds> <+hhmm>` at its end) as ISO 8601, as `%cI` writes it.
fn ident_date(ident: &str) -> String {
    let mut it = ident.rsplit(' ');
    let (tz, secs) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
    let (Ok(secs), Ok(tz)) = (secs.parse::<i64>(), tz.parse::<i32>()) else {
        return String::new();
    };
    let offset = (tz / 100 * 60 + tz % 100) * 60;
    chrono::FixedOffset::east_opt(offset)
        .and_then(|o| chrono::DateTime::from_timestamp(secs, 0).map(|d| d.with_timezone(&o)))
        .map_or_else(String::new, |d| {
            d.to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
        })
}

/// The seconds in a git identity line, for ordering commits.
fn ident_secs(ident: &str) -> i64 {
    ident
        .rsplit(' ')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

fn index_entry(path: &str, oid: ObjectId, mode: u32, size: usize) -> IndexEntry {
    let path = path.as_bytes().to_vec();
    IndexEntry {
        ctime_sec: 0,
        ctime_nsec: 0,
        mtime_sec: 0,
        mtime_nsec: 0,
        dev: 0,
        ino: 0,
        mode,
        uid: 0,
        gid: 0,
        size: size as u32,
        oid,
        flags: path.len().min(0xfff) as u16,
        flags_extended: None,
        path,
        base_index_pos: 0,
    }
}

impl GitStore {
    /// `author`: author and committer for operation commits. Without it, git's own configuration
    /// is used; set this to avoid depending on the host's global git config.
    pub fn new(dir: impl Into<PathBuf>, author: Option<Author>) -> Self {
        GitStore {
            dir: dir.into(),
            identity: author,
            rev: "HEAD".to_owned(),
            #[cfg(test)]
            race: std::sync::Mutex::new(None),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Who git's config says commits here: the repository's own `user.name` and `user.email`, else
    /// the user's global ones. `None` unless both are set.
    pub fn configured_author(&self) -> Option<Author> {
        let config = self.open().ok()?.config().ok()?;
        Some(Author {
            name: config.get("user.name")?,
            email: config.get("user.email")?,
        })
    }

    /// Sets (`Some`) or removes (`None`) a key in the repository's own config.
    pub fn set_config(&self, key: &str, value: Option<&str>) -> Result<(), StoreError> {
        let repo = self.open()?;
        let config = repo.config_handle();
        match value {
            Some(v) => config.set_local(key, v),
            // Removing a key that is not there is fine.
            None => config.unset_local(key).or(Ok(())),
        }
        .map_err(g("write config"))
    }

    /// The remote's URL, when the book has one (`origin`).
    pub fn remote_url(&self) -> Option<String> {
        self.open().ok()?.config().ok()?.get("remote.origin.url")
    }

    /// Makes `url` the book's remote (`origin`), or removes the remote (`None`). Sync then
    /// pushes to it; nothing is fetched or pushed here.
    pub fn set_remote(&self, url: Option<&str>) -> Result<(), StoreError> {
        self.set_config("remote.origin.url", url)?;
        let fetch = url.map(|_| "+refs/heads/*:refs/remotes/origin/*");
        self.set_config("remote.origin.fetch", fetch)
    }

    /// True when the book has a remote, `origin`, to sync with.
    pub fn has_remote(&self) -> bool {
        self.open()
            .is_ok_and(|repo| repo.remotes().get("origin").is_ok())
    }

    fn open(&self) -> Result<Repository, StoreError> {
        Repository::discover(Some(&self.dir), &Default::default()).map_err(g("open repository"))
    }
}

/// What a sync did to trunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Synced {
    /// This device and the remote had the same version.
    UpToDate,
    /// This device's new commits went to the remote.
    Pushed,
    /// The remote's new commits came to this device.
    Pulled,
    /// Both had new commits: merged in one commit and pushed.
    Merged,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn paths_that_reach_git_or_leave_the_book_are_unsafe() {
        for ok in [
            "main.beancount",
            "x/2026.bean",
            "git/a",
            ".github/a",
            ".gitignore",
        ] {
            assert!(is_safe_path(ok), "{ok}");
        }
        for bad in [
            "",
            "a//b",
            ".",
            "..",
            "../x",
            ".git/config",
            ".GIT/config",
            ".git./config",
            ".git /config",
            "git~1/hooks/post-commit",
            "GIT~1/hooks/post-commit",
            "a\\b",
            "C:/x",
            "a:stream",
        ] {
            assert!(!is_safe_path(bad), "{bad}");
        }
    }

    /// The push is leased: another device's push after the fetch is a conflict, and the remote keeps
    /// what that device pushed.
    #[test]
    fn a_push_after_the_remote_moved_is_a_conflict_and_overwrites_nothing() {
        let root =
            std::env::temp_dir().join(format!("branchbooks-git-lease-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (bare, book) = (root.join("remote.git"), root.join("book"));
        std::fs::create_dir_all(&bare).unwrap();
        let ok = Command::new("git")
            .args(["init", "-q", "--bare", "-b", "main"])
            .arg(&bare)
            .status()
            .unwrap();
        assert!(ok.success());
        let who = || {
            Some(Author {
                name: "t".into(),
                email: "t@example.invalid".into(),
            })
        };
        let info = |op: &str| CommitInfo {
            op_id: op.into(),
            op_type: "test".into(),
            summary: op.into(),
        };
        let file = |path: &str, text: &str| FileChange {
            path: path.into(),
            text: Some(text.into()),
        };
        let store = GitStore::init(&book, who()).unwrap();
        let base = store
            .commit_as(
                "",
                &[file("main.beancount", "; book\n")],
                &info("base"),
                None,
            )
            .unwrap();
        store.set_remote(Some(bare.to_str().unwrap())).unwrap();
        assert_eq!(store.sync("main.beancount", None).unwrap(), Synced::Pushed);

        // Both move on, so sync merges; the merge's ref update is between the fetch and the push.
        store
            .commit_as(&base, &[file("mine.bean", "; mine\n")], &info("mine"), None)
            .unwrap();
        let remote = GitStore::new(&bare, who());
        let theirs = remote
            .commit_as(
                &base,
                &[file("theirs.bean", "; theirs\n")],
                &info("theirs"),
                None,
            )
            .unwrap();
        let racer = GitStore::new(&bare, who());
        let racing = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let seen = racing.clone();
        let change = file("later.bean", "; later\n");
        let later = info("later");
        *store.race.lock().unwrap() = Some(Box::new(move || {
            let at = racer.head();
            let v = racer
                .commit_as(&at, std::slice::from_ref(&change), &later, None)
                .unwrap();
            *seen.lock().unwrap() = v;
        }));
        let r = store.sync("main.beancount", None);
        assert!(matches!(r, Err(StoreError::Conflict(_))), "{r:?}");
        let moved = racing.lock().unwrap().clone();
        assert_ne!(moved, theirs);
        assert_eq!(
            remote.head(),
            moved,
            "the remote keeps the other device's push"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn ids_come_from_names_and_do_not_collide() {
        assert_eq!(book_id("Rental property", []), "rental-property");
        assert_eq!(
            book_id("Rental property", ["rental-property", "rental-property-2"]),
            "rental-property-3"
        );
        assert_eq!(book_id("Café & Co.", []), "cafe-co");
        assert_eq!(book_id("!!!", []), "book");
        // Cut to 32 characters, which here ends on a separator: that dash goes too.
        assert_eq!(
            book_id(&format!("{} tail", "a".repeat(31)), []),
            "a".repeat(31)
        );
    }

    #[test]
    fn book_ids() {
        assert!(valid_book_id("main") && valid_book_id("a") && valid_book_id("a-1"));
        assert!(!valid_book_id("-a") && !valid_book_id("a-") && !valid_book_id("A"));
        assert!(!valid_book_id(&"a".repeat(41)) && !valid_book_id(""));
        assert!(!valid_book_id("../x"));
    }

    #[test]
    fn reads_dates_and_names_from_identity_lines() {
        let ident = "Ann Example <ann@example.invalid> 1793491200 -0400";
        assert_eq!(ident_name(ident), "Ann Example");
        assert_eq!(ident_date(ident), "2026-10-31T20:00:00-04:00");
        assert_eq!(ident_secs(ident), 1793491200);
    }

    /// The ref update is a compare-and-swap: a commit that lands after the HEAD check is a
    /// conflict.
    #[test]
    fn a_commit_after_the_head_check_is_a_conflict() {
        let dir = std::env::temp_dir().join(format!("branchbooks-git-race-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |dir: &Path, args: &[&str]| {
            let ok = Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(
                ok.status.success(),
                "{}",
                String::from_utf8_lossy(&ok.stderr)
            );
            String::from_utf8(ok.stdout).unwrap()
        };
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "user.name", "t"]);
        git(&dir, &["config", "user.email", "t@example.invalid"]);
        std::fs::write(dir.join("main.beancount"), "; ledger\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "Initial"]);
        let base = git(&dir, &["rev-parse", "HEAD"]).trim().to_owned();

        let store = GitStore::new(&dir, None);
        let racer = dir.clone();
        *store.race.lock().unwrap() = Some(Box::new(move || {
            std::fs::write(racer.join("other.bean"), "; racing writer\n").unwrap();
            git(&racer, &["add", "other.bean"]);
            git(&racer, &["commit", "-q", "-m", "Racing writer"]);
        }));
        let info = CommitInfo {
            op_id: "x".into(),
            op_type: "test".into(),
            summary: "Race".into(),
        };
        let change = FileChange {
            path: "main.beancount".into(),
            text: Some("; replaced\n".into()),
        };
        let r = store.commit_as(&base, &[change], &info, None);
        assert!(matches!(r, Err(StoreError::Conflict(_))), "{r:?}");
        assert_eq!(
            git(&dir, &["log", "-1", "--format=%s"]).trim(),
            "Racing writer"
        );
        let text = std::fs::read_to_string(dir.join("main.beancount")).unwrap();
        assert!(!text.contains("; replaced"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
