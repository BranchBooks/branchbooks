//! The `branchbooks` binary against golden files. Each `NNN.txt` is one read-only command over a
//! fixture; the `wNN.txt` files are one write session, in order, against a fresh repository
//! holding fixtures/multifile, ending with the ledger files in `w-final-*.txt`.
//!
//! Paths print absolute, so the repository root reads `<repo>` (and the session's repository
//! `<dir>`); commit hashes read `<hash>`.
//!
//! A change that alters a golden file is a change in behaviour, to be made on purpose: rerun with
//! `UPDATE_SNAPSHOTS=1 cargo test -p branchbooks-cli --test cli` and review the diff.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// `p` as the binary prints it: forward slashes, without Windows's `\\?\` prefix.
fn printed(p: &Path) -> String {
    let s = p.to_str().unwrap().replace('\\', "/");
    s.strip_prefix("//?/").map_or(s.clone(), str::to_owned)
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

struct Golden {
    args: Vec<String>,
    text: String,
}

fn golden(path: &Path) -> Golden {
    let text = std::fs::read_to_string(path).unwrap();
    let (first, _) = text.split_once('\n').unwrap();
    let args = serde_json::from_str(first).unwrap();
    Golden { args, text }
}

/// Runs the binary and formats the result the way the golden files are written.
fn run(cwd: &Path, args: &[String], norm: impl Fn(&str) -> String) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_branchbooks"))
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    let code = out.status.code().unwrap();
    format!(
        "--- exit {code}\n--- stdout\n{}--- stderr\n{}",
        norm(&String::from_utf8_lossy(&out.stdout)),
        norm(&String::from_utf8_lossy(&out.stderr))
    )
}

/// Replaces every 10-digit lowercase hex word (a short commit hash) with `<hash>`.
fn mask_hashes(s: &str) -> String {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find(word) {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let end = tail.find(|c| !word(c)).unwrap_or(tail.len());
        let w = &tail[..end];
        let hash = w.len() == 10
            && w.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        out.push_str(if hash { "<hash>" } else { w });
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// Checks `got` against the file's expected text, or rewrites the file under `UPDATE_SNAPSHOTS`.
fn check(path: &Path, header: &str, want: &str, got: &str, context: &str) {
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        if got != want {
            std::fs::write(path, format!("{header}{got}")).unwrap();
        }
    } else {
        assert_eq!(got, want, "{}: {context}", path.display());
    }
}

fn check_golden(path: &Path, g: &Golden, got: &str) {
    let (first, want) = g.text.split_once('\n').unwrap();
    let header = format!("{first}\n");
    check(path, &header, want, got, &format!("{:?}", g.args));
}

#[test]
fn read_commands_match_their_golden_files() {
    let repo = repo();
    let root = printed(&repo);
    let mut files: Vec<PathBuf> = std::fs::read_dir(golden_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            let name = p.file_name().unwrap().to_str().unwrap();
            name.as_bytes()[0].is_ascii_digit()
        })
        .collect();
    files.sort();
    assert!(files.len() > 60, "golden files missing");
    for path in files {
        let g = golden(&path);
        let got = run(&repo, &g.args, |s| s.replace(&root, "<repo>"));
        check_golden(&path, &g, &got);
    }
}

/// A fresh work tree holding the multi-file fixture, deleted when dropped.
struct Ledger(PathBuf);

impl Ledger {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("branchbooks-cli-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.name", "BranchBooks Test"]);
        git(&["config", "user.email", "test@example.invalid"]);
        let fixtures = repo().join("fixtures/multifile");
        for f in ["main.beancount", "2025.beancount", "2026.beancount"] {
            std::fs::copy(fixtures.join(f), dir.join(f)).unwrap();
        }
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "Initial ledger"]);
        Ledger(dir)
    }
}

impl Drop for Ledger {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_write_session_matches_its_golden_files() {
    let ledger = Ledger::new();
    let dir = ledger.0.to_str().unwrap().to_owned();
    let mut sessions: Vec<PathBuf> = std::fs::read_dir(golden_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            let name = p.file_name().unwrap().to_str().unwrap();
            name.starts_with('w') && !name.starts_with("w-final")
        })
        .collect();
    sessions.sort();
    assert_eq!(sessions.len(), 13, "golden files missing");
    for path in sessions {
        let g = golden(&path);
        let args: Vec<String> = g
            .args
            .iter()
            .map(|a| if a == "<dir>" { dir.clone() } else { a.clone() })
            .collect();
        let shown = printed(&ledger.0);
        let got = run(&repo(), &args, |s| {
            mask_hashes(&s.replace(&dir, "<dir>").replace(&shown, "<dir>"))
        });
        check_golden(&path, &g, &got);
    }
    for (file, saved) in [
        ("2026.beancount", "w-final-2026.txt"),
        ("main.beancount", "w-final-main.txt"),
    ] {
        let got = std::fs::read_to_string(ledger.0.join(file)).unwrap();
        let path = golden_dir().join(saved);
        let want = std::fs::read_to_string(&path).unwrap();
        check(&path, "", &want, &got, file);
    }
}

#[test]
fn masks_short_hashes_only() {
    assert_eq!(
        mask_hashes("committed 0123456789  x deadbeef00a [w1]"),
        "committed <hash>  x deadbeef00a [w1]"
    );
}
