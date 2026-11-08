//! Walking the book's history.

use super::*;

impl GitStore {
    /// Commits reachable from this store's ref, newest first, as `git log` orders them: by
    /// commit date, ties in the order they were reached.
    pub(super) fn walk(
        &self,
        repo: &Repository,
        mut keep: impl FnMut(ObjectId, &CommitData) -> bool,
    ) -> Result<(), StoreError> {
        let Ok(start) = resolve_revision(repo, &self.rev) else {
            return Ok(());
        };
        let read = |id: &ObjectId| -> Result<CommitData, StoreError> {
            let obj = repo.odb().read(id).map_err(g("read commit"))?;
            parse_commit(&obj.data).map_err(g("read commit"))
        };
        let mut seen = HashSet::from([start]);
        let mut seq = 0u64;
        let first = read(&start)?;
        let mut queue = BinaryHeap::from([(ident_secs(&first.committer), Reverse(seq), start)]);
        while let Some((_, _, id)) = queue.pop() {
            let commit = read(&id)?;
            for p in &commit.parents {
                if seen.insert(*p) {
                    seq += 1;
                    queue.push((ident_secs(&read(p)?.committer), Reverse(seq), *p));
                }
            }
            if !keep(id, &commit) {
                break;
            }
        }
        Ok(())
    }

    /// Recent commits with their operation trailers, newest first.
    pub fn log(&self, limit: usize) -> Result<Vec<GitCommit>, StoreError> {
        let mut out = Vec::new();
        if limit == 0 || self.head().is_empty() {
            return Ok(out);
        }
        let repo = self.open()?;
        self.walk(&repo, |id, c| {
            let (op_id, op_type) = parse_trailers(&c.message);
            out.push(GitCommit {
                version: id.to_hex(),
                summary: c.message.lines().next().unwrap_or("").to_owned(),
                op_id,
                op_type,
                at: ident_date(&c.committer),
                author: ident_name(&c.author).to_owned(),
            });
            out.len() < limit
        })?;
        Ok(out)
    }
}
