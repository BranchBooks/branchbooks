//! Writing to the book: commits built from objects, one per operation.

use super::*;

impl GitStore {
    /// The current commit, or empty when there are no commits yet.
    pub fn head(&self) -> String {
        self.rev_parse(&self.rev)
    }

    pub(super) fn rev_parse(&self, rev: &str) -> String {
        self.open()
            .ok()
            .and_then(|r| resolve_revision(&r, rev).ok())
            .map(|id| id.to_hex())
            .unwrap_or_default()
    }

    /// `Name <email> <seconds> <+hhmm>` for now: `who`, else the store's identity, else git's
    /// `user.name` and `user.email`.
    pub(super) fn ident(
        &self,
        repo: &Repository,
        who: Option<&Author>,
    ) -> Result<String, StoreError> {
        let who = if let Some(a) = who.or(self.identity.as_ref()) {
            a.clone()
        } else {
            let config = repo.config().map_err(g("read config"))?;
            match (config.get("user.name"), config.get("user.email")) {
                (Some(name), Some(email)) => Author { name, email },
                _ => return Err(failed("set user.name and user.email in git's config")),
            }
        };
        let now = chrono::Local::now();
        Ok(format!(
            "{} <{}> {} {}",
            who.name,
            who.email,
            now.timestamp(),
            now.format("%z")
        ))
    }

    /// The branch this store commits to: the one `HEAD` names on trunk, else the scenario's.
    pub(super) fn branch(&self, repo: &Repository) -> Result<String, StoreError> {
        if self.rev != "HEAD" {
            return Ok(self.rev.clone());
        }
        refs::read_symbolic_ref(repo.git_dir(), "HEAD")
            .map_err(g("read HEAD"))?
            .ok_or_else(|| failed("HEAD is not on a branch"))
    }

    /// Commits on top of `base`. `author`: who made this change, if not the store's own identity
    /// (which stays the committer).
    pub fn commit_as(
        &self,
        base: &str,
        changes: &[FileChange],
        info: &CommitInfo,
        author: Option<&Author>,
    ) -> Result<String, StoreError> {
        if changes.is_empty() {
            return Err(failed("nothing to commit"));
        }
        let head = self.head();
        if head != base {
            return Err(StoreError::Conflict(format!(
                "HEAD is {}, operation was based on {}",
                version_name(&head),
                version_name(base)
            )));
        }
        let repo = self.open()?;
        let mut files = IndexMap::new();
        let parent = if base.is_empty() {
            None
        } else {
            let id = oid(base)?;
            tree_entries(&repo, &tree_of(&repo, &id)?, "", &mut files)?;
            Some(id)
        };
        for c in changes {
            match &c.text {
                None => {
                    files.shift_remove(&c.path);
                }
                Some(text) => {
                    let id = repo
                        .odb()
                        .write(ObjectKind::Blob, text.as_bytes())
                        .map_err(g("write blob"))?;
                    files.insert(c.path.clone(), (MODE_REGULAR, id));
                }
            }
        }
        let mut tree_index = Index::new();
        for (path, (mode, id)) in &files {
            tree_index.add_or_replace(index_entry(path, *id, *mode, 0));
        }
        let tree = write_tree_from_index(repo.odb(), &tree_index, "").map_err(g("write tree"))?;
        let parents = parent.into_iter().collect();
        let commit = self.write_commit(&repo, tree, parents, &commit_message(info), author)?;
        self.advance(&repo, commit, parent)?;
        Ok(commit.to_hex())
    }

    /// Writes a commit object; the store's identity is the committer, and the author unless
    /// `author` is given.
    pub(super) fn write_commit(
        &self,
        repo: &Repository,
        tree: ObjectId,
        parents: Vec<ObjectId>,
        message: &str,
        author: Option<&Author>,
    ) -> Result<ObjectId, StoreError> {
        let committer = self.ident(repo, None)?;
        let author = match author {
            Some(a) => self.ident(repo, Some(a))?,
            None => committer.clone(),
        };
        let raw = serialize_commit(&CommitData {
            tree,
            parents,
            author,
            committer,
            author_raw: Vec::new(),
            committer_raw: Vec::new(),
            encoding: None,
            message: message.to_owned(),
            raw_message: None,
            extra_headers: Vec::new(),
        });
        repo.odb()
            .write(ObjectKind::Commit, &raw)
            .map_err(g("write commit"))
    }

    /// Moves this store's branch from `old` to `new`, a compare-and-swap. On trunk in a non-bare
    /// repository, the working tree and index then change for exactly the paths that differ;
    /// refused before anything moves when one of them has uncommitted changes.
    pub(super) fn advance(
        &self,
        repo: &Repository,
        new: ObjectId,
        old: Option<ObjectId>,
    ) -> Result<(), StoreError> {
        let branch = self.branch(repo)?;
        let mut before = IndexMap::new();
        if let Some(o) = old {
            tree_entries(repo, &tree_of(repo, &o)?, "", &mut before)?;
        }
        let mut after = IndexMap::new();
        tree_entries(repo, &tree_of(repo, &new)?, "", &mut after)?;
        let changed: BTreeSet<&String> = before
            .keys()
            .chain(after.keys())
            .filter(|p| before.get(*p) != after.get(*p))
            .collect();
        // A change from a client, or a tree from a remote, could name a path outside the book.
        if let Some(bad) = changed.iter().find(|p| !is_safe_path(p)) {
            return Err(StoreError::Refused(format!("{bad} is not a ledger path")));
        }
        // Only trunk is checked out; a scenario's branch has no working tree.
        let work_tree = (self.rev == "HEAD")
            .then(|| repo.work_tree().map(Path::to_path_buf))
            .flatten();
        let mut index = None;
        if let Some(wt) = &work_tree {
            let loaded = repo.index_handle().load().map_err(g("read index"))?;
            let dirty: Vec<&str> = changed
                .iter()
                .map(|p| p.as_str())
                .filter(|p| is_dirty(repo, wt, &loaded, before.get(*p).map(|e| e.1), p))
                .collect();
            if !dirty.is_empty() {
                return Err(StoreError::Refused(format!(
                    "uncommitted changes in {}",
                    dirty.join(", ")
                )));
            }
            index = Some(loaded);
        }

        #[cfg(test)]
        if let Some(race) = self.race.lock().unwrap().as_mut() {
            race();
        }

        if let Err(e) = update_ref(repo, &branch, Some(new), old) {
            let base = old.map(|o| o.to_hex()).unwrap_or_default();
            let now = self.head();
            if now != base {
                return Err(StoreError::Conflict(format!(
                    "HEAD moved to {now} while committing; operation was based on {}",
                    version_name(&base)
                )));
            }
            return Err(failed(format!("update {branch}: {e}")));
        }

        if let (Some(wt), Some(mut index)) = (work_tree, index) {
            for path in changed {
                let file = wt.join(path);
                if let Some(&(mode, id)) = after.get(path) {
                    let blob = repo.odb().read(&id).map_err(g("read file"))?;
                    if let Some(dir) = file.parent() {
                        std::fs::create_dir_all(dir).map_err(g("write working tree"))?;
                    }
                    std::fs::write(&file, &blob.data).map_err(g("write working tree"))?;
                    index.add_or_replace(index_entry(path, id, mode, blob.data.len()));
                } else {
                    index.remove(path.as_bytes());
                    let _ = std::fs::remove_file(&file);
                }
            }
            repo.index_handle()
                .write(&mut index)
                .map_err(g("write index"))?;
        }
        Ok(())
    }

    /// Starts a book with no history: a new repository in `dir`, on branch `main`, with no
    /// remote until one is added.
    pub fn init(dir: impl Into<PathBuf>, author: Option<Author>) -> Result<GitStore, StoreError> {
        let dir = dir.into();
        // Refs as loose files and packed-refs, grit-lib's default, which real git reads too.
        let opts = InitOptions::new().with_initial_branch("main".to_owned());
        Repository::init(&dir, &opts).map_err(g("init"))?;
        let store = GitStore::new(dir, author);
        // As a clone does: text stays as committed, whatever the global git config says.
        store.set_config("core.autocrlf", Some("false"))?;
        Ok(store)
    }
}
