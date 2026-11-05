//! Small helpers for planning: file names, renames, summaries and comparing diagnostics.

use super::*;

/// `(^|/)(\d{4})\.(beancount|bean)$`: the year and extension of a year file.
pub(super) fn year_file(path: &str) -> Option<(u32, &str)> {
    let name = &path[path.rfind('/').map_or(0, |i| i + 1)..];
    let (stem, ext) = name.split_once('.')?;
    let ok = stem.len() == 4
        && stem.bytes().all(|b| b.is_ascii_digit())
        && matches!(ext, "beancount" | "bean");
    ok.then(|| (stem.parse().unwrap_or(0), ext))
}

/// The kind of an entry input or entry: `txn`, `open`, `close`, `assign`, `move`, `custom`,
/// `other`.
pub(super) fn input_kind(e: &EntryInput) -> &'static str {
    match e {
        EntryInput::Txn(_) => "txn",
        EntryInput::Open(_) => "open",
        EntryInput::Close(_) => "close",
        EntryInput::Assign(_) => "assign",
        EntryInput::Move(_) => "move",
        EntryInput::Target(_) => "target",
        EntryInput::Overspending(_) => "overspending",
        EntryInput::Balance(_) => "balance",
    }
}

pub(super) fn kind_of(e: &Entry) -> &'static str {
    crate::write::import::kind_of(e).name()
}

/// `a` under its new name, when it is `from` or one of its sub-accounts.
pub(super) fn renamed(a: &str, from: &str, to: &str) -> Option<String> {
    let rest = a.strip_prefix(from)?;
    (rest.is_empty() || rest.starts_with(':')).then(|| format!("{to}{rest}"))
}

/// `line` with each whole account name `from` (or a sub-account of it) changed to `to`. A name
/// is whole when the characters around it cannot be part of an account name.
pub(super) fn rename_in(line: &str, from: &str, to: &str) -> String {
    let part = |c: char| c.is_alphanumeric() || c == '-' || c == ':';
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(i) = rest.find(from) {
        let before = rest[..i].chars().next_back();
        let after = rest[i + from.len()..].chars().next();
        let whole = !before.is_some_and(part)
            && match after {
                None => true,
                Some(':') => rest[i + from.len() + 1..].chars().next().is_some_and(part),
                Some(c) => !part(c),
            };
        out.push_str(&rest[..i]);
        out.push_str(if whole { to } else { from });
        rest = &rest[i + from.len()..];
    }
    out.push_str(rest);
    out
}

pub(super) fn op_type_of(kind: &str, verb: &str) -> String {
    let noun = match kind {
        "txn" => "transaction",
        "open" => "account-open",
        "close" => "account-close",
        k => k,
    };
    if verb == "add" {
        return match kind {
            "txn" => "add-transaction".to_owned(),
            "open" => "open-account".to_owned(),
            "close" => "close-account".to_owned(),
            "assign" | "move" => kind.to_owned(),
            "target" => "set-target".to_owned(),
            "overspending" => "set-overspending".to_owned(),
            _ => format!("add-{noun}"),
        };
    }
    format!("{verb}-{noun}")
}

pub(super) fn describe(e: &EntryInput, currency: &str) -> String {
    match e {
        EntryInput::Txn(t) => {
            let what = match t.payee.as_deref().filter(|p| !p.is_empty()) {
                Some(p) => format!("{p}: {}", t.narration),
                None => t.narration.clone(),
            };
            format!("transaction {} {what:?}", t.date)
        }
        EntryInput::Open(o) => format!("open {} on {}", o.account, o.date),
        EntryInput::Close(c) => format!("close {} on {}", c.account, c.date),
        EntryInput::Assign(a) => {
            format!(
                "assign {} {} to {} on {}",
                a.amount,
                a.currency.as_deref().unwrap_or(currency),
                a.category,
                a.date
            )
        }
        EntryInput::Move(m) => format!(
            "move {} {} from {} to {} on {}",
            m.amount,
            m.currency.as_deref().unwrap_or(currency),
            m.from,
            m.to,
            m.date
        ),
        EntryInput::Target(t) => {
            let due = t
                .due
                .as_ref()
                .map_or(String::new(), |d| format!(" due {d}"));
            format!(
                "target {} {} for {} from {}{due}",
                t.amount,
                t.currency.as_deref().unwrap_or(currency),
                t.category,
                t.date
            )
        }
        EntryInput::Overspending(o) => format!(
            "{} overspending for {} from {}",
            if o.rollover { "roll over" } else { "reset" },
            o.category,
            o.date
        ),
        EntryInput::Balance(b) => format!(
            "balance {} {} {} on {}",
            b.account,
            b.amount,
            b.currency.as_deref().unwrap_or(currency),
            b.date
        ),
    }
}

pub(super) fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map_or_else(String::new, |f| {
        f.to_uppercase().collect::<String>() + c.as_str()
    })
}

fn diag_key(d: &Diagnostic) -> String {
    // Without the amounts: an assertion that already failed and still fails is not a new error.
    let message = match d.code.as_str() {
        "blocked" => d.message.split(" skipped").next().unwrap_or(""),
        "balance" => d.message.split(": expected").next().unwrap_or(""),
        _ => &d.message,
    };
    format!("{}|{message}", d.code)
}

/// Diagnostics in `after` that were not in `before`, compared without line numbers (lines
/// shift).
pub fn added(before: &[Diagnostic], after: &[Diagnostic]) -> Vec<Diagnostic> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for d in before {
        *counts.entry(diag_key(d)).or_insert(0) += 1;
    }
    after
        .iter()
        .filter(|d| match counts.get_mut(&diag_key(d)) {
            Some(n) if *n > 0 => {
                *n -= 1;
                false
            }
            _ => true,
        })
        .cloned()
        .collect()
}

/// The ledger errors `changes` would add to the files `before` (rooted at `root`), for a change
/// that did not come through `plan`: a commit a client sent, a scenario merge, a sync.
pub fn new_errors(
    root: &str,
    before: &IndexMap<String, String>,
    changes: &[FileChange],
) -> Vec<Diagnostic> {
    let mut after = before.clone();
    for c in changes {
        match &c.text {
            None => {
                after.shift_remove(&c.path);
            }
            Some(t) => {
                after.insert(c.path.clone(), t.clone());
            }
        }
    }
    if !after.contains_key(root) {
        return Vec::new();
    }
    let errors_of = |files: &IndexMap<String, String>| {
        if !files.contains_key(root) {
            return Vec::new();
        }
        let source = MemorySource(files.clone().into_iter().collect());
        Book::from_load(load(root, &source, None)).errors()
    };
    crate::ledger::sort_diagnostics(added(&errors_of(before), &errors_of(&after)))
}

pub(super) fn rejected(what: &str, errors: Vec<Diagnostic>) -> OperationError {
    let first = errors.first().map_or("", |e| e.message.as_str());
    let message = format!(
        "the {what} would add {} ledger error(s): {first}",
        errors.len()
    );
    OperationError::new(OperationErrorCode::Rejected, message, errors)
}

/// The envelope checks: an invalid envelope entry, or one that blocks another.
pub(super) fn check_envelope(new_budget: &[Diagnostic], new_blocked: &[Diagnostic]) -> Result<()> {
    let invalid: Vec<Diagnostic> = new_budget
        .iter()
        .filter(|d| d.code == "envelope")
        .cloned()
        .collect();
    if let Some(first) = invalid.first() {
        return Err(OperationError::new(
            OperationErrorCode::Invalid,
            first.message.clone(),
            invalid,
        ));
    }
    if let Some(first) = new_blocked.first() {
        let message = first.message.replacen(" skipped", "", 1);
        return Err(OperationError::new(
            OperationErrorCode::Blocked,
            message,
            new_blocked.to_vec(),
        ));
    }
    Ok(())
}
