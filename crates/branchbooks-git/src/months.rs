//! Closed months: annotated tags on the book as it was.

use super::*;

impl GitStore {
    /// Closes `month`: an annotated tag `close/<month>` on trunk's latest version, by `by` (the
    /// store's identity when `None`). A month is closed once.
    pub fn close(&self, month: Month, by: Option<&Author>) -> Result<String, StoreError> {
        let head = self.rev_parse("HEAD");
        if head.is_empty() {
            return Err(StoreError::Refused("there is nothing to close yet".into()));
        }
        let rev = format!("{CLOSES}{month}");
        if !self.rev_parse(&rev).is_empty() {
            return Err(StoreError::Refused(format!("{month} is already closed")));
        }
        let repo = self.open()?;
        // The tagger is the committer. The tag is written as `git tag -a` would.
        let tagger = self.ident(&repo, by)?;
        let tag = format!(
            "object {head}\ntype commit\ntag close/{month}\ntagger {tagger}\n\nClose {month}\n"
        );
        let id = repo
            .odb()
            .write(ObjectKind::Tag, tag.as_bytes())
            .map_err(g("write tag"))?;
        update_ref(&repo, &rev, Some(id), None).map_err(g("close"))?;
        Ok(head)
    }

    /// Reopens a closed month: its tag moves under `refs/tags/archive/closes/`, so the record stays.
    pub fn reopen(&self, month: Month) -> Result<(), StoreError> {
        let rev = format!("{CLOSES}{month}");
        // The tag object itself, not the commit it points to.
        let id = self
            .open()
            .ok()
            .and_then(|r| refs::resolve_ref(r.git_dir(), &rev).ok())
            .map(|id| id.to_hex())
            .unwrap_or_default();
        if id.is_empty() {
            return Err(StoreError::Refused(format!("{month} is not closed")));
        }
        self.set_aside(&rev, &id, &format!("{ARCHIVE_CLOSES}{month}"))
    }

    /// Every closed month, oldest first.
    pub fn closes(&self) -> Result<Vec<Close>, StoreError> {
        let repo = self.open()?;
        let mut out = Vec::new();
        for (name, id) in Self::refs_under(&repo, CLOSES)? {
            let Ok(month) = name.parse() else { continue };
            let obj = repo.odb().read(&id).map_err(g("read tag"))?;
            let tag = parse_tag(&obj.data).map_err(g("read tag"))?;
            let tagger = tag.tagger.unwrap_or_default();
            out.push(Close {
                month,
                version: tag.object.to_hex(),
                by: ident_name(&tagger).to_owned(),
                at: ident_date(&tagger),
            });
        }
        Ok(out)
    }
}
