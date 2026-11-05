//! Versioned file store over Beancount text: one operation is one commit. The git store
//! (`branchbooks-git`) and the in-memory store implement this trait for the driver in
//! `workspace`.

use std::fmt;

use indexmap::IndexMap;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// Opaque version id; empty for an empty store.
    pub version: String,
    /// Ledger files (paths relative to the ledger root, forward slashes) and their text.
    pub files: IndexMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    /// New text, or `None` to delete the file.
    pub text: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitInfo {
    pub op_id: String,
    pub op_type: String,
    /// One-line summary; the store adds the Op-Id and Op-Type trailers.
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitRecord {
    pub info: CommitInfo,
    pub version: String,
    pub paths: Vec<String>,
}

/// Why a commit failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    /// Another writer committed first: the operation was based on an old version.
    Conflict(String),
    /// The store would not write, such as a git work tree with uncommitted edits to the same files.
    /// Nothing was written, and retrying will not help until someone acts.
    Refused(String),
    /// Anything else, such as a lost connection. The commit may or may not have happened.
    Failed(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Conflict(m) | StoreError::Refused(m) | StoreError::Failed(m) => {
                f.write_str(m)
            }
        }
    }
}

impl std::error::Error for StoreError {}

pub trait Store {
    fn snapshot(&self) -> Result<Snapshot, StoreError>;
    /// Commits on top of `base`. Fails with `StoreError::Conflict` if the store has moved past
    /// `base`.
    fn commit(
        &mut self,
        base: &str,
        changes: &[FileChange],
        info: &CommitInfo,
    ) -> Result<String, StoreError>;
    /// The commit that carried this Op-Id, if any: lets a retried operation be recognised, not
    /// repeated.
    fn find_op(&self, op_id: &str) -> Result<Option<CommitRecord>, StoreError>;
}

/// The commit message every backend writes: summary, blank line, trailers.
pub fn commit_message(info: &CommitInfo) -> String {
    format!(
        "{}\n\nOp-Id: {}\nOp-Type: {}\n",
        info.summary, info.op_id, info.op_type
    )
}

/// Parses Op-Id / Op-Type trailers back out of a commit message: `(op_id, op_type)`.
pub fn parse_trailers(message: &str) -> (Option<String>, Option<String>) {
    let find = |key: &str| {
        message.lines().find_map(|l| {
            let v = l.strip_prefix(key)?.trim();
            (!v.is_empty() && !v.contains(char::is_whitespace)).then(|| v.to_owned())
        })
    };
    (find("Op-Id:"), find("Op-Type:"))
}

/// In-memory store, for tests.
#[derive(Clone, Debug, Default)]
pub struct MemoryStore {
    files: IndexMap<String, String>,
    version: String,
    pub log: Vec<CommitRecord>,
}

impl MemoryStore {
    pub fn new<'a>(files: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let files: IndexMap<String, String> = files
            .into_iter()
            .map(|(p, t)| (p.to_owned(), t.to_owned()))
            .collect();
        let version = if files.is_empty() {
            String::new()
        } else {
            "v0".to_owned()
        };
        MemoryStore {
            files,
            version,
            log: Vec::new(),
        }
    }

    pub fn file(&self, path: &str) -> Option<&str> {
        self.files.get(path).map(String::as_str)
    }

    /// Simulates another writer (another device, or a hand edit committed elsewhere).
    pub fn external(&mut self, path: &str, text: &str) {
        self.files.insert(path.to_owned(), text.to_owned());
        self.version.push('x');
    }
}

impl Store for MemoryStore {
    fn snapshot(&self) -> Result<Snapshot, StoreError> {
        Ok(Snapshot {
            version: self.version.clone(),
            files: self.files.clone(),
        })
    }

    fn find_op(&self, op_id: &str) -> Result<Option<CommitRecord>, StoreError> {
        Ok(self.log.iter().find(|c| c.info.op_id == op_id).cloned())
    }

    fn commit(
        &mut self,
        base: &str,
        changes: &[FileChange],
        info: &CommitInfo,
    ) -> Result<String, StoreError> {
        if base != self.version {
            let name = |v: &str| {
                if v.is_empty() {
                    "(empty)".to_owned()
                } else {
                    v.to_owned()
                }
            };
            return Err(StoreError::Conflict(format!(
                "store is at {}, operation was based on {}",
                name(&self.version),
                name(base)
            )));
        }
        for c in changes {
            match &c.text {
                None => {
                    self.files.shift_remove(&c.path);
                }
                Some(t) => {
                    self.files.insert(c.path.clone(), t.clone());
                }
            }
        }
        self.version = format!("v{}", self.log.len() + 1);
        self.log.push(CommitRecord {
            info: info.clone(),
            version: self.version.clone(),
            paths: changes.iter().map(|c| c.path.clone()).collect(),
        });
        Ok(self.version.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailers_round_trip() {
        let info = CommitInfo {
            op_id: "op-1".into(),
            op_type: "assign".into(),
            summary: "Assign".into(),
        };
        assert_eq!(
            parse_trailers(&commit_message(&info)),
            (Some("op-1".into()), Some("assign".into()))
        );
        assert_eq!(parse_trailers("nothing"), (None, None));
    }
}
