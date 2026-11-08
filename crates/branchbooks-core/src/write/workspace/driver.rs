//! [`Workspace`]: a [`WorkspaceCore`] over a store, with Op-Id lookups and retries.

use super::*;

/// What an applied operation did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplyResult {
    pub op_id: String,
    pub op_type: String,
    pub version: String,
    /// Files written.
    pub changed: Vec<String>,
    /// Where the added or edited entry now is.
    pub entry: Option<EntryRef>,
    /// Consequences that do not block the operation, such as an existing move that is now
    /// blocked.
    pub warnings: Vec<Diagnostic>,
    /// True when the store already had this Op-Id: nothing was written this time.
    pub duplicate: bool,
}

/// Why `apply` failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyError {
    Operation(OperationError),
    Store(StoreError),
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApplyError::Operation(e) => e.fmt(f),
            ApplyError::Store(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for ApplyError {}

impl From<OperationError> for ApplyError {
    fn from(e: OperationError) -> Self {
        ApplyError::Operation(e)
    }
}

impl From<StoreError> for ApplyError {
    fn from(e: StoreError) -> Self {
        ApplyError::Store(e)
    }
}

/// Runs `step` until it stops failing with a conflict (another writer committed first), at most
/// `retries` times more. `step` is told when it is a retry, and then reloads the latest version
/// before planning again.
pub fn retry_on_conflict<T>(
    retries: usize,
    mut step: impl FnMut(bool) -> std::result::Result<T, ApplyError>,
) -> std::result::Result<T, ApplyError> {
    let mut attempt = 0;
    loop {
        match step(attempt > 0) {
            Err(ApplyError::Store(StoreError::Conflict(_))) if attempt < retries => attempt += 1,
            other => return other,
        }
    }
}

/// A workspace over a store: applies operations with Op-Id lookups and retries.
pub struct Workspace<S: Store> {
    pub store: S,
    pub core: WorkspaceCore,
    new_id: Box<dyn FnMut() -> String>,
}

impl<S: Store> Workspace<S> {
    pub fn open(
        store: S,
        root: Option<&str>,
        new_id: Box<dyn FnMut() -> String>,
    ) -> std::result::Result<Self, StoreError> {
        let mut ws = Workspace {
            store,
            core: WorkspaceCore::new(root),
            new_id,
        };
        ws.refresh()?;
        Ok(ws)
    }

    /// Reloads the latest version from the store (after a conflict, for example).
    pub fn refresh(&mut self) -> std::result::Result<(), StoreError> {
        let snap = self.store.snapshot()?;
        self.core.reset(&snap.version, snap.files);
        Ok(())
    }

    pub fn book(&mut self) -> &mut Book {
        self.core.book()
    }

    pub fn text(&self, path: &str) -> Option<&str> {
        self.core.text(path)
    }

    /// Validates, writes and commits one operation. Passing an `op_id` makes the call idempotent;
    /// with `retries`, a conflict reloads the latest version and applies the operation again.
    pub fn apply(
        &mut self,
        op: &Operation,
        op_id: Option<&str>,
        retries: usize,
    ) -> std::result::Result<ApplyResult, ApplyError> {
        let id = op_id.map_or_else(|| (self.new_id)(), str::to_owned);
        retry_on_conflict(retries, |again| {
            if again {
                self.refresh()?;
            }
            if (op_id.is_some() || again)
                && let Some(done) = self.store.find_op(&id)?
            {
                return Ok(ApplyResult {
                    op_id: id.clone(),
                    op_type: done.info.op_type,
                    version: done.version,
                    changed: done.paths,
                    entry: None,
                    warnings: Vec::new(),
                    duplicate: true,
                });
            }
            let planned = self.core.plan(op)?;
            Ok(self.commit(planned, &id)?)
        })
    }

    fn commit(
        &mut self,
        planned: Planned,
        op_id: &str,
    ) -> std::result::Result<ApplyResult, StoreError> {
        let info = CommitInfo {
            op_id: op_id.to_owned(),
            op_type: planned.op_type.clone(),
            summary: planned.summary.clone(),
        };
        let version = self
            .store
            .commit(&self.core.version, &planned.changes, &info)?;
        let changed = planned.changes.iter().map(|c| c.path.clone()).collect();
        let entry = planned.entry.clone();
        let warnings = self.core.commit_planned(planned, &version);
        Ok(ApplyResult {
            op_id: op_id.to_owned(),
            op_type: info.op_type,
            version,
            changed,
            entry,
            warnings,
            duplicate: false,
        })
    }

    pub fn preview_import(&mut self, files: &IndexMap<String, String>) -> ImportPreview {
        self.core.preview_import(files)
    }

    /// Adds the entries from other Beancount files that the ledger does not have yet, in one
    /// commit. The result is `None` when there was nothing new.
    pub fn import_files(
        &mut self,
        files: &IndexMap<String, String>,
        retries: usize,
    ) -> std::result::Result<(ImportPreview, Option<ApplyResult>), ApplyError> {
        retry_on_conflict(retries, |again| {
            if again {
                self.refresh()?;
            }
            let preview = self.core.preview_import(files);
            if let Some(bad) = preview.errors.first() {
                let at = if bad.file.is_empty() {
                    String::new()
                } else {
                    format!("{}:{}: ", bad.file, bad.line)
                };
                let e = OperationError::new(
                    OperationErrorCode::Invalid,
                    format!("{at}{}", bad.message),
                    preview.errors.clone(),
                );
                return Err(e.into());
            }
            if preview.add.is_empty() {
                return Ok((preview, None));
            }
            let planned = self.core.plan_import(&preview)?;
            let id = (self.new_id)();
            let done = self.commit(planned, &id)?;
            Ok((preview, Some(done)))
        })
    }
}
