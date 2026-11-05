//! Reading ledger files from a commit, and the core's `Store` over them.

use super::*;

/// The tree a commit records.
pub(super) fn tree_of(repo: &Repository, commit: &ObjectId) -> Result<ObjectId, StoreError> {
    let obj = repo.odb().read(commit).map_err(g("read commit"))?;
    Ok(parse_commit(&obj.data).map_err(g("read commit"))?.tree)
}

/// Every file under `tree`, by path, as `(mode, blob)`, in git's order.
pub(super) fn tree_entries(
    repo: &Repository,
    tree: &ObjectId,
    prefix: &str,
    out: &mut IndexMap<String, (u32, ObjectId)>,
) -> Result<(), StoreError> {
    let obj = repo.odb().read(tree).map_err(g("read tree"))?;
    for e in parse_tree(&obj.data).map_err(g("read tree"))? {
        let path = format!("{prefix}{}", String::from_utf8_lossy(&e.name));
        if e.mode == MODE_TREE {
            tree_entries(repo, &e.oid, &format!("{path}/"), out)?;
        } else {
            out.insert(path, (e.mode, e.oid));
        }
    }
    Ok(())
}

/// The ledger files in a commit or a tree, by path.
pub(super) fn ledger_files(
    repo: &Repository,
    version: &ObjectId,
) -> Result<IndexMap<String, String>, StoreError> {
    let obj = repo.odb().read(version).map_err(g("read version"))?;
    let tree = if obj.kind == ObjectKind::Commit {
        parse_commit(&obj.data).map_err(g("read commit"))?.tree
    } else {
        *version
    };
    let mut entries = IndexMap::new();
    tree_entries(repo, &tree, "", &mut entries)?;
    let mut files = IndexMap::new();
    for (path, (_, id)) in entries.into_iter().filter(|(p, _)| is_ledger_file(p)) {
        let blob = repo.odb().read(&id).map_err(g("read file"))?;
        // Never read loosely: the next save would write the damaged text back.
        let text = String::from_utf8(blob.data).map_err(|_| {
            StoreError::Refused(format!(
                "{path} is not UTF-8: save it as UTF-8 and commit it"
            ))
        })?;
        files.insert(path, text);
    }
    Ok(files)
}

/// Whether the file at `path` holds someone's edit: it matches neither `head` (its blob in the base
/// commit) nor its index entry. A file that matches its index entry but not `head` is what a save
/// cut off between moving the branch and updating the files leaves behind, so it is not an edit. The
/// cost: a hand edit staged with `git add` also passes, though git still has its contents.
pub(super) fn is_dirty(
    repo: &Repository,
    work_tree: &Path,
    index: &Index,
    head: Option<ObjectId>,
    path: &str,
) -> bool {
    let blob = |id: Option<ObjectId>| id.and_then(|id| repo.odb().read(&id).ok()).map(|o| o.data);
    let on_disk = std::fs::read(work_tree.join(path)).ok();
    on_disk != blob(head) && on_disk != blob(index.get(path.as_bytes(), 0).map(|e| e.oid))
}

impl Store for GitStore {
    fn snapshot(&self) -> Result<Snapshot, StoreError> {
        let version = self.head();
        let files = if version.is_empty() {
            IndexMap::new()
        } else {
            ledger_files(&self.open()?, &oid(&version)?)?
        };
        Ok(Snapshot { version, files })
    }

    fn commit(
        &mut self,
        base: &str,
        changes: &[FileChange],
        info: &CommitInfo,
    ) -> Result<String, StoreError> {
        self.commit_as(base, changes, info, None)
    }

    fn find_op(&self, op_id: &str) -> Result<Option<CommitRecord>, StoreError> {
        if self.head().is_empty() {
            return Ok(None);
        }
        let repo = self.open()?;
        let mut found = None;
        self.walk(&repo, |id, c| {
            let (id_found, op_type) = parse_trailers(&c.message);
            if id_found.as_deref() != Some(op_id) {
                return true;
            }
            found = Some((id, c.parents.clone(), c.tree, op_type, c.message.clone()));
            false
        })?;
        let Some((version, parents, tree, op_type, message)) = found else {
            return Ok(None);
        };
        // The paths it changed, as `git diff-tree --root` lists them: none for a merge.
        let mut paths = Vec::new();
        if parents.len() <= 1 {
            let mut after = IndexMap::new();
            tree_entries(&repo, &tree, "", &mut after)?;
            let mut before = IndexMap::new();
            if let Some(p) = parents.first() {
                tree_entries(&repo, &tree_of(&repo, p)?, "", &mut before)?;
            }
            let all: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
            paths = all
                .into_iter()
                .filter(|p| before.get(*p) != after.get(*p))
                .cloned()
                .collect();
        }
        Ok(Some(CommitRecord {
            info: CommitInfo {
                op_id: op_id.to_owned(),
                op_type: op_type.unwrap_or_default(),
                summary: message.lines().next().unwrap_or("").to_owned(),
            },
            version: version.to_hex(),
            paths,
        }))
    }
}
