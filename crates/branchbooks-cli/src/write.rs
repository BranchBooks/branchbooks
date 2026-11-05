//! Write commands: each one is a single operation, committed to the ledger's git repository.

use std::path::Path;
use std::process::ExitCode;

use branchbooks_core::write::render::{
    AssignInput, BudgetFlag, CloseInput, EntryInput, MetaInput, MetaInputValue, MoveInput,
    OpenInput, PostingInput, TransactionInput,
};
use branchbooks_core::write::store::StoreError;
use branchbooks_core::write::workspace::{ApplyError, Operation, Workspace};
use branchbooks_git::{GitStore, Synced};

use crate::{Opts, first};

pub const WRITE_USAGE: &str = r#"  branchbooks assign <repo> --date D --category ACCOUNT --amount N
  branchbooks move <repo> --date D --from ACCOUNT --to ACCOUNT --amount N
  branchbooks txn <repo> --date D --narration TEXT [--payee TEXT] --posting "ACCOUNT [N [CUR]]"... [--meta key=value]...
  branchbooks open <repo> --date D --account ACCOUNT [--currency CUR] [--budget on|off|excluded|placeholder]
  branchbooks close <repo> --date D --account ACCOUNT
  branchbooks delete <repo> --at FILE:LINE
  branchbooks log <repo>
  branchbooks sync <repo> [--root main.beancount]   (token for HTTPS remotes: BRANCHBOOKS_TOKEN)
  (write commands take [--root main.beancount] and [--op-id ID] to make a retry safe)"#;

pub const WRITE_COMMANDS: [&str; 8] = [
    "assign", "move", "txn", "open", "close", "delete", "log", "sync",
];

/// Why a command was not run.
enum Refusal {
    /// Bad arguments: the message, then the usage.
    Usage(String),
    /// Arguments that parse but that the write path would refuse, reported the way it reports
    /// them.
    Invalid(String),
}

fn one(opts: &Opts, key: &str) -> Result<String, Refusal> {
    match first(opts, key) {
        Some(v) if !v.is_empty() => Ok(v.to_owned()),
        _ => Err(Refusal::Usage(format!("missing --{key}"))),
    }
}

fn posting(text: &str) -> Result<PostingInput, Refusal> {
    let mut words = text.split_whitespace();
    let account = words
        .next()
        .ok_or_else(|| Refusal::Usage("empty --posting".to_owned()))?;
    let input = PostingInput {
        account: account.to_owned(),
        amount: words.next().map(str::to_owned),
        currency: words.next().map(str::to_owned),
        meta: MetaInput::new(),
    };
    match words.next() {
        Some(extra) => Err(Refusal::Usage(format!(
            "--posting \"{text}\": unexpected \"{extra}\" after the currency"
        ))),
        None => Ok(input),
    }
}

/// `--meta key=value` pairs; a repeated key keeps its first place and its last value.
fn meta(list: Option<&Vec<String>>) -> Result<MetaInput, Refusal> {
    let mut out = MetaInput::new();
    for kv in list.into_iter().flatten() {
        let Some((key, value)) = kv.split_once('=').filter(|(k, _)| !k.is_empty()) else {
            return Err(Refusal::Usage(format!("--meta needs key=value, got {kv}")));
        };
        let value = MetaInputValue::Text(value.to_owned());
        match out.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value,
            None => out.push((key.to_owned(), value)),
        }
    }
    Ok(out)
}

fn operation<S: branchbooks_core::write::store::Store>(
    cmd: &str,
    opts: &Opts,
    ws: &Workspace<S>,
) -> Result<Operation, Refusal> {
    let date = || one(opts, "date");
    let entry = match cmd {
        "assign" => EntryInput::Assign(AssignInput {
            date: date()?,
            category: one(opts, "category")?,
            amount: one(opts, "amount")?,
            currency: None,
        }),
        "move" => EntryInput::Move(MoveInput {
            date: date()?,
            from: one(opts, "from")?,
            to: one(opts, "to")?,
            amount: one(opts, "amount")?,
            currency: None,
        }),
        "txn" => EntryInput::Txn(TransactionInput {
            date: date()?,
            narration: one(opts, "narration")?,
            payee: first(opts, "payee").map(str::to_owned),
            meta: meta(opts.get("meta"))?,
            postings: opts
                .get("posting")
                .into_iter()
                .flatten()
                .map(|p| posting(p))
                .collect::<Result<_, _>>()?,
            ..Default::default()
        }),
        "open" => {
            let (date, account) = (date()?, one(opts, "account")?);
            let budget = match first(opts, "budget") {
                None | Some("") => None,
                Some(flag) => Some(
                    BudgetFlag::parse(flag)
                        .ok_or_else(|| Refusal::Invalid(format!("invalid budget flag {flag}")))?,
                ),
            };
            EntryInput::Open(OpenInput {
                date,
                account,
                currencies: opts.get("currency").cloned().unwrap_or_default(),
                budget,
                ..Default::default()
            })
        }
        "close" => EntryInput::Close(CloseInput {
            date: date()?,
            account: one(opts, "account")?,
        }),
        "delete" => {
            let at = one(opts, "at")?;
            let found = at.rsplit_once(':').and_then(|(file, line)| {
                let line: u32 = line.parse().ok()?;
                ws.core
                    .entries()
                    .map(|(_, e)| e)
                    .find(|e| &*e.file == file && e.line == line)
            });
            let Some(e) = found else {
                return Err(Refusal::Usage(format!("no entry starts at {at}")));
            };
            return Ok(Operation::Delete(ws.core.ref_of(e)));
        }
        _ => return Err(Refusal::Usage(format!("unknown command {cmd}"))),
    };
    Ok(Operation::Add(entry))
}

fn not_committed(e: &StoreError) -> ExitCode {
    match e {
        StoreError::Conflict(m) | StoreError::Refused(m) => eprintln!("not committed: {m}"),
        StoreError::Failed(m) => eprintln!("error: {m}"),
    }
    ExitCode::FAILURE
}

pub fn run(cmd: &str, repo: &Path, opts: &Opts) -> ExitCode {
    let store = GitStore::new(repo, None);
    if cmd == "log" {
        return match store.log(20) {
            Ok(commits) => {
                for c in commits {
                    let id = c.op_id.map_or_else(String::new, |id| format!("  [{id}]"));
                    let op_type = c.op_type.as_deref().unwrap_or("-");
                    let short = c.version.get(..10).unwrap_or(&c.version);
                    println!("{short}  {op_type:<16} {}{id}", c.summary);
                }
                ExitCode::SUCCESS
            }
            Err(e) => not_committed(&e),
        };
    }
    if cmd == "sync" {
        // From the environment, not an argument: an argument shows in `ps` and shell history.
        let token = std::env::var("BRANCHBOOKS_TOKEN").ok();
        let root = first(opts, "root").unwrap_or("main.beancount");
        return match store.sync(root, token.as_deref()) {
            Ok(s) => {
                println!(
                    "{}",
                    match s {
                        Synced::UpToDate => "up to date",
                        Synced::Pushed => "pushed",
                        Synced::Pulled => "pulled",
                        Synced::Merged => "merged and pushed",
                    }
                );
                // Then scenarios, closed months and what was set aside.
                match store.sync_refs(token.as_deref()) {
                    Ok(notes) => {
                        for n in notes {
                            println!("note: {n}");
                        }
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("scenarios and closes not synced: {e}");
                        ExitCode::FAILURE
                    }
                }
            }
            Err(StoreError::Failed(m)) => {
                eprintln!("error: {m}");
                ExitCode::FAILURE
            }
            Err(StoreError::Conflict(m) | StoreError::Refused(m)) => {
                eprintln!("not synced: {m}");
                ExitCode::FAILURE
            }
        };
    }
    let ids = Box::new(|| uuid::Uuid::new_v4().to_string());
    let mut ws = match Workspace::open(store, first(opts, "root"), ids) {
        Ok(ws) => ws,
        Err(e) => return not_committed(&e),
    };
    let op = match operation(cmd, opts, &ws) {
        Ok(op) => op,
        Err(Refusal::Usage(m)) => {
            eprintln!("{m}\nusage:\n{WRITE_USAGE}");
            return ExitCode::from(2);
        }
        Err(Refusal::Invalid(m)) => {
            eprintln!("refused (invalid): {m}");
            return ExitCode::FAILURE;
        }
    };
    // Retries re-apply on the latest version if another writer committed first; --op-id makes a
    // repeated command (say, after a dropped connection) a no-op instead of a second commit.
    match ws.apply(&op, first(opts, "op-id"), 3) {
        Ok(r) if r.duplicate => {
            let short = r.version.get(..10).unwrap_or(&r.version);
            println!(
                "already committed as {short} (Op-Id {}); nothing to do",
                r.op_id
            );
            ExitCode::SUCCESS
        }
        Ok(r) => {
            let short = r.version.get(..10).unwrap_or(&r.version);
            println!("committed {short}  {}  Op-Id {}", r.op_type, r.op_id);
            let at = r
                .entry
                .map_or_else(String::new, |e| format!("; entry at {}:{}", e.file, e.line));
            println!("changed: {}{at}", r.changed.join(", "));
            for w in r.warnings {
                println!("warning: {}:{}: {}", w.file, w.line, w.message);
            }
            ExitCode::SUCCESS
        }
        Err(ApplyError::Operation(e)) => {
            eprintln!("refused ({}): {}", e.code.as_str(), e.message);
            for d in e.diagnostics {
                eprintln!("  {}:{}: {}", d.file, d.line, d.message);
            }
            ExitCode::FAILURE
        }
        Err(ApplyError::Store(e)) => not_committed(&e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_posting_takes_account_amount_and_currency_and_nothing_more() {
        let p = posting(" Assets:Cash  -10 EUR ").ok().unwrap();
        assert_eq!(
            (p.amount.as_deref(), p.currency.as_deref()),
            (Some("-10"), Some("EUR"))
        );
        assert!(matches!(
            posting("Assets:Cash -10 EUR @ 1.10 USD"),
            Err(Refusal::Usage(m)) if m.contains("unexpected \"@\"")
        ));
    }
}
