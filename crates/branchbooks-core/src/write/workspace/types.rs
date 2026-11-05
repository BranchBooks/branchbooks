//! What the write path takes and how it refuses.

use super::*;

/// Addresses an entry as the caller saw it: an edit or delete is refused if the text has changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryRef {
    pub file: String,
    pub line: u32,
    pub fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operation {
    Add(EntryInput),
    Edit(EntryRef, EntryInput),
    Delete(EntryRef),
    Reconcile(ReconcileInput),
    /// Renames an account and its sub-accounts everywhere they are written, in one commit.
    RenameAccount {
        from: String,
        to: String,
    },
}

/// A bank statement for `account` (R2, R3): its `balance` at the end of `date`, and the pending
/// transactions in it, which become cleared.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReconcileInput {
    pub account: String,
    pub date: String,
    pub balance: String,
    pub currency: Option<String>,
    pub clear: Vec<EntryRef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationErrorCode {
    Invalid,
    Rejected,
    Blocked,
    Stale,
    NotFound,
}

impl OperationErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            OperationErrorCode::Invalid => "invalid",
            OperationErrorCode::Rejected => "rejected",
            OperationErrorCode::Blocked => "blocked",
            OperationErrorCode::Stale => "stale",
            OperationErrorCode::NotFound => "not-found",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationError {
    pub code: OperationErrorCode,
    pub message: String,
    pub diagnostics: Vec<Diagnostic>,
}

impl OperationError {
    pub(super) fn new(
        code: OperationErrorCode,
        message: impl Into<String>,
        diagnostics: Vec<Diagnostic>,
    ) -> Self {
        OperationError {
            code,
            message: message.into(),
            diagnostics,
        }
    }
}

impl fmt::Display for OperationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for OperationError {}

impl From<InputError> for OperationError {
    fn from(e: InputError) -> Self {
        OperationError::new(OperationErrorCode::Invalid, e.0, Vec::new())
    }
}

pub(super) type Result<T> = std::result::Result<T, OperationError>;
