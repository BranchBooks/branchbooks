//! Scenarios: what-if branches of the book.

use super::*;

impl GitStore {
    /// The same repository, reading and committing to a scenario's branch; `None` when there is
    /// no such scenario.
    pub fn scenario(&self, name: &str) -> Option<GitStore> {
        let rev = format!("{SCENARIOS}{name}");
        if !is_scenario_name(name) || self.rev_parse(&rev).is_empty() {
            return None;
        }
        Some(GitStore {
            dir: self.dir.clone(),
            identity: self.identity.clone(),
            rev,
            #[cfg(test)]
            race: std::sync::Mutex::new(None),
        })
    }

    /// Every scenario, by name.
    pub fn scenarios(&self) -> Result<Vec<Scenario>, StoreError> {
        let repo = self.open()?;
        let head = resolve_revision(&repo, "HEAD").ok();
        let mut out = Vec::new();
        for (refname, version) in Self::refs_under(&repo, SCENARIOS)? {
            let base = head
                .and_then(|h| merge_bases_all(&repo, &[h, version]).ok())
                .and_then(|b| b.first().map(ObjectId::to_hex))
                .unwrap_or_default();
            out.push(Scenario {
                name: refname,
                version: version.to_hex(),
                base,
            });
        }
        Ok(out)
    }

    /// The refs under `prefix`, by the rest of their name, sorted by name.
    pub(super) fn refs_under(
        repo: &Repository,
        prefix: &str,
    ) -> Result<Vec<(String, ObjectId)>, StoreError> {
        let mut found: Vec<(String, ObjectId)> = refs::list_refs(repo.git_dir(), prefix)
            .map_err(g("list refs"))?
            .into_iter()
            .map(|(name, id)| (name.strip_prefix(prefix).unwrap_or(&name).to_owned(), id))
            .collect();
        found.sort();
        Ok(found)
    }

    /// Starts a scenario at trunk commit `at` (trunk's latest when empty).
    pub fn fork(&self, name: &str, at: &str) -> Result<String, StoreError> {
        if !is_scenario_name(name) {
            return Err(StoreError::Refused(
                "a scenario name is lower-case letters, digits and dashes, up to 40".into(),
            ));
        }
        let repo = self.open()?;
        let head = self.head();
        let at = if at.is_empty() { head.as_str() } else { at };
        let at_id = resolve_revision(&repo, at).ok();
        let on_trunk = match (at_id, resolve_revision(&repo, "HEAD").ok()) {
            (Some(a), Some(h)) => is_ancestor(&repo, a, h).unwrap_or(false),
            _ => false,
        };
        let Some(at_id) = at_id.filter(|_| on_trunk) else {
            return Err(StoreError::Refused(format!("{at} is not a trunk version")));
        };
        let rev = format!("{SCENARIOS}{name}");
        update_ref(&repo, &rev, Some(at_id), None)
            .map_err(|_| StoreError::Refused(format!("there is already a scenario {name}")))?;
        Ok(at.to_owned())
    }

    /// The scenario merged with trunk's latest version, without committing anything.
    pub fn merged(&self, name: &str) -> Result<Merged, StoreError> {
        let branch = self
            .scenario(name)
            .ok_or_else(|| StoreError::Refused(format!("there is no scenario {name}")))?;
        let base = self.head();
        let repo = self.open()?;
        Ok(match merge3(&repo, oid(&base)?, oid(&branch.head())?)? {
            Ok(tree) => Merged::Clean {
                files: ledger_files(&repo, &tree)?,
                base,
            },
            Err(files) => Merged::Conflicts(files),
        })
    }

    /// Prunes a scenario: its branch moves under `refs/tags/archive/`, so its history stays.
    pub fn archive(&self, name: &str) -> Result<(), StoreError> {
        let branch = self
            .scenario(name)
            .ok_or_else(|| StoreError::Refused(format!("there is no scenario {name}")))?;
        self.set_aside(&branch.rev, &branch.head(), &format!("{ARCHIVE}{name}"))
    }

    /// Moves ref `rev` (now at `id`) to `<kept>-<seconds since 1970>`.
    pub(super) fn set_aside(&self, rev: &str, id: &str, kept: &str) -> Result<(), StoreError> {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let kept = format!("{kept}-{secs}");
        let repo = self.open()?;
        let id = oid(id)?;
        update_ref(&repo, &kept, Some(id), None).map_err(g("archive"))?;
        update_ref(&repo, rev, None, Some(id)).map_err(g("archive"))?;
        Ok(())
    }
}
