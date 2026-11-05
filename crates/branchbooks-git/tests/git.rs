//! The git store against real repositories.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use branchbooks_core::write::render::{AssignInput, EntryInput, PostingInput, TransactionInput};
use branchbooks_core::write::store::{CommitInfo, FileChange, Store, StoreError};
use branchbooks_core::write::workspace::{ApplyError, Operation, Workspace};
use branchbooks_git::{Author, GitStore, Merged, Synced, changes_between};

/// A temporary directory, deleted when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(kind: &str) -> Self {
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("branchbooks-{kind}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// A work tree holding the multi-file fixture and one file that is not a ledger file.
fn repo() -> TempDir {
    let tmp = TempDir::new("git");
    let dir = &tmp.0;
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.name", "BranchBooks Test"]);
    git(dir, &["config", "user.email", "test@example.invalid"]);
    git(dir, &["config", "core.autocrlf", "false"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/multifile");
    for f in ["main.beancount", "2025.beancount", "2026.beancount"] {
        std::fs::copy(fixtures.join(f), dir.join(f)).unwrap();
    }
    std::fs::write(dir.join("README.txt"), "not a ledger file\n").unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "Initial ledger"]);
    tmp
}

fn open(store: GitStore) -> Workspace<GitStore> {
    let mut n = 0;
    let ids = Box::new(move || {
        n += 1;
        format!("gen-{n}")
    });
    Workspace::open(store, None, ids).unwrap()
}

fn assign(amount: &str) -> Operation {
    Operation::Add(EntryInput::Assign(AssignInput {
        date: "2026-01-02".into(),
        category: "Expenses:Food:Groceries".into(),
        amount: amount.into(),
        currency: None,
    }))
}

fn info(op_id: &str, summary: &str) -> CommitInfo {
    CommitInfo {
        op_id: op_id.into(),
        op_type: "edit".into(),
        summary: summary.into(),
    }
}

#[test]
fn reads_ledger_files_from_head() {
    let tmp = repo();
    let snap = GitStore::new(&tmp.0, None).snapshot().unwrap();
    let mut paths: Vec<&str> = snap.files.keys().map(String::as_str).collect();
    paths.sort_unstable();
    assert_eq!(
        paths,
        ["2025.beancount", "2026.beancount", "main.beancount"]
    );
    assert_eq!(snap.version, git(&tmp.0, &["rev-parse", "HEAD"]).trim());
}

#[test]
fn records_who_made_a_change_as_the_author_with_the_store_as_committer() {
    let tmp = repo();
    let identity = Author {
        name: "BranchBooks".into(),
        email: "bb@example.invalid".into(),
    };
    let mut store = GitStore::new(&tmp.0, Some(identity));
    let snap = store.snapshot().unwrap();
    let text = format!("{}\n; note\n", snap.files["main.beancount"]);
    let ann = Author {
        name: "ann".into(),
        email: "bb@example.invalid".into(),
    };
    let change = FileChange {
        path: "main.beancount".into(),
        text: Some(text.clone()),
    };
    store
        .commit_as(&snap.version, &[change], &info("a", "By ann"), Some(&ann))
        .unwrap();
    assert_eq!(
        git(&tmp.0, &["log", "-1", "--format=%an|%cn"]),
        "ann|BranchBooks\n"
    );
    let snap2 = store.snapshot().unwrap();
    let change = FileChange {
        path: "main.beancount".into(),
        text: Some(format!("{text}; again\n")),
    };
    store
        .commit(&snap2.version, &[change], &info("b", "By the store"))
        .unwrap();
    let authors: Vec<String> = store
        .log(2)
        .unwrap()
        .into_iter()
        .map(|c| c.author)
        .collect();
    assert_eq!(authors, ["BranchBooks", "ann"]);
}

#[test]
fn commits_one_operation_per_commit_with_trailers_and_updates_the_working_tree() {
    let tmp = repo();
    let mut ws = open(GitStore::new(&tmp.0, None));
    ws.apply(&assign("50.00"), Some("op-1"), 0).unwrap();
    let txn = Operation::Add(EntryInput::Txn(TransactionInput {
        date: "2027-01-03".into(),
        narration: "New year".into(),
        postings: vec![
            PostingInput {
                account: "Expenses:Food:Groceries".into(),
                amount: Some("5.00".into()),
                ..Default::default()
            },
            PostingInput {
                account: "Assets:Checking".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    }));
    ws.apply(&txn, Some("op-2"), 0).unwrap();

    let log: Vec<_> = ws
        .store
        .log(3)
        .unwrap()
        .into_iter()
        .map(|c| (c.op_id, c.op_type))
        .collect();
    let some = |a: &str, b: &str| (Some(a.to_owned()), Some(b.to_owned()));
    assert_eq!(
        log,
        [
            some("op-2", "add-transaction"),
            some("op-1", "assign"),
            (None, None)
        ]
    );
    assert!(
        git(&tmp.0, &["log", "-1", "--format=%B"])
            .contains("\n\nOp-Id: op-2\nOp-Type: add-transaction\n")
    );
    let shown = git(&tmp.0, &["show", "--name-only", "--format=", "HEAD"]);
    let mut changed: Vec<&str> = shown.trim().lines().collect();
    changed.sort_unstable();
    assert_eq!(changed, ["2027.beancount", "main.beancount"]);
    assert_eq!(git(&tmp.0, &["status", "--porcelain"]), "");
    assert_eq!(
        std::fs::read_to_string(tmp.0.join("2027.beancount")).unwrap(),
        ws.text("2027.beancount").unwrap()
    );
    assert_eq!(ws.core.version, git(&tmp.0, &["rev-parse", "HEAD"]).trim());
}

#[test]
fn a_commit_made_elsewhere_is_a_conflict_and_refresh_then_retry_succeeds() {
    let tmp = repo();
    let mut ws = open(GitStore::new(&tmp.0, None));
    std::fs::write(tmp.0.join("notes.bean"), "; another device\n").unwrap();
    git(&tmp.0, &["add", "notes.bean"]);
    git(&tmp.0, &["commit", "-q", "-m", "Elsewhere"]);
    let op = assign("1.00");
    assert!(matches!(
        ws.apply(&op, None, 0),
        Err(ApplyError::Store(StoreError::Conflict(_)))
    ));
    ws.refresh().unwrap();
    ws.apply(&op, None, 0).unwrap();
    assert_eq!(
        git(&tmp.0, &["log", "--format=%s", "-1"]).trim(),
        "Assign 1.00 USD to Expenses:Food:Groceries on 2026-01-02"
    );
}

#[test]
fn retries_after_a_commit_made_elsewhere() {
    let tmp = repo();
    let mut ws = open(GitStore::new(&tmp.0, None));
    std::fs::write(tmp.0.join("notes.bean"), "; another device\n").unwrap();
    git(&tmp.0, &["add", "notes.bean"]);
    git(&tmp.0, &["commit", "-q", "-m", "Elsewhere"]);
    ws.apply(&assign("1.00"), None, 1).unwrap();
    assert_eq!(git(&tmp.0, &["rev-list", "--count", "HEAD"]).trim(), "3");
}

#[test]
fn refuses_to_commit_over_uncommitted_hand_edits_to_the_same_file() {
    let tmp = repo();
    let mut ws = open(GitStore::new(&tmp.0, None));
    let before = git(&tmp.0, &["rev-parse", "HEAD"]);
    let path = tmp.0.join("2026.beancount");
    let edited = format!("{}; hand edit\n", std::fs::read_to_string(&path).unwrap());
    std::fs::write(&path, edited).unwrap();
    assert!(matches!(
        ws.apply(&assign("1.00"), None, 3),
        Err(ApplyError::Store(StoreError::Refused(_)))
    ));
    assert_eq!(git(&tmp.0, &["rev-parse", "HEAD"]), before);
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("; hand edit")
    );
}

#[test]
fn works_on_a_bare_repository_and_finds_operations_by_op_id() {
    let work = repo();
    let bare = TempDir::new("bare");
    let (from, to) = (work.0.to_str().unwrap(), bare.0.to_str().unwrap());
    git(&work.0, &["clone", "-q", "--bare", from, to]);
    // A bare repository has no user config of its own; the store sets the identity itself.
    let writer = Author {
        name: "BranchBooks Writer".into(),
        email: "writer@example.invalid".into(),
    };
    let mut ws = open(GitStore::new(&bare.0, Some(writer)));
    let r = ws.apply(&assign("3.00"), Some("bare-1"), 0).unwrap();
    assert!(git(&bare.0, &["log", "-1", "--format=%B"]).contains("Op-Id: bare-1"));
    assert_eq!(
        git(&bare.0, &["log", "-1", "--format=%an <%ae> / %cn"]).trim(),
        "BranchBooks Writer <writer@example.invalid> / BranchBooks Writer"
    );
    let found = ws.store.find_op("bare-1").unwrap().unwrap();
    assert_eq!(found.version, r.version);
    assert_eq!(found.info.op_type, "assign");
    assert_eq!(found.paths, ["2026.beancount"]);
    assert_eq!(ws.store.find_op("bare-").unwrap(), None);
    let again = ws.apply(&assign("3.00"), Some("bare-1"), 0).unwrap();
    assert!(again.duplicate);
    assert_eq!(git(&bare.0, &["rev-list", "--count", "HEAD"]).trim(), "2");
}

#[test]
fn leaves_the_users_index_alone() {
    let tmp = repo();
    std::fs::write(tmp.0.join("staged.txt"), "work in progress\n").unwrap();
    git(&tmp.0, &["add", "staged.txt"]);
    let mut ws = open(GitStore::new(&tmp.0, None));
    ws.apply(&assign("3.00"), None, 0).unwrap();
    assert_eq!(
        git(&tmp.0, &["show", "--name-only", "--format=", "HEAD"]).trim(),
        "2026.beancount"
    );
    assert_eq!(git(&tmp.0, &["status", "--porcelain"]), "A  staged.txt\n");
}

#[test]
fn an_empty_repository_has_no_files_and_no_operations() {
    let tmp = TempDir::new("empty");
    git(&tmp.0, &["init", "-q", "-b", "main"]);
    let store = GitStore::new(&tmp.0, None);
    let snap = store.snapshot().unwrap();
    assert_eq!(snap.version, "");
    assert!(snap.files.is_empty());
    assert_eq!(store.find_op("x").unwrap(), None);
    assert!(store.log(5).unwrap().is_empty());
}

/// `text` replaces a file in one commit on `store`.
fn write(store: &GitStore, path: &str, text: &str, op_id: &str) -> String {
    let change = FileChange {
        path: path.into(),
        text: Some(text.into()),
    };
    store
        .commit_as(&store.head(), &[change], &info(op_id, op_id), None)
        .unwrap()
}

#[test]
fn a_scenario_takes_its_own_commits_and_merges_into_trunk_as_one() {
    let tmp = repo();
    let trunk = GitStore::new(&tmp.0, None);
    let fork_point = trunk.head();
    assert_eq!(trunk.fork("car-later", "").unwrap(), fork_point);
    assert!(matches!(
        trunk.fork("car-later", ""),
        Err(StoreError::Refused(_))
    ));
    assert!(matches!(
        trunk.fork("Car Later", ""),
        Err(StoreError::Refused(_))
    ));
    assert!(trunk.scenario("nope").is_none());

    // Changes on the scenario go to its branch; trunk stays where it was.
    let mut ws = open(trunk.scenario("car-later").unwrap());
    ws.apply(&assign("50.00"), Some("s-1"), 0).unwrap();
    assert_eq!(trunk.head(), fork_point);
    assert!(ws.store.find_op("s-1").unwrap().is_some());
    assert!(trunk.find_op("s-1").unwrap().is_none());
    // Meanwhile trunk moves on, in another file.
    let older = trunk.snapshot().unwrap().files["2025.beancount"].clone() + "; trunk note\n";
    write(&trunk, "2025.beancount", &older, "t-1");
    let listed = trunk.scenarios().unwrap();
    assert_eq!(
        (listed[0].name.as_str(), listed[0].base.as_str()),
        ("car-later", fork_point.as_str())
    );

    let Merged::Clean { base, files } = trunk.merged("car-later").unwrap() else {
        panic!("a clean merge")
    };
    let changes = changes_between(&trunk.snapshot().unwrap().files, &files);
    let paths: Vec<&str> = changes.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, ["2026.beancount"]);
    trunk
        .commit_as(
            &base,
            &changes,
            &info("m-1", "Merge scenario car-later"),
            None,
        )
        .unwrap();
    let snap = trunk.snapshot().unwrap();
    assert!(
        snap.files["2026.beancount"]
            .contains("custom \"envelope-assign\" Expenses:Food:Groceries 50.00 USD")
    );
    assert!(snap.files["2025.beancount"].ends_with("; trunk note\n"));
    // One commit with one parent: a squash, not a merge commit. The work tree has it too.
    assert_eq!(
        git(&tmp.0, &["rev-list", "--parents", "-n1", "HEAD"])
            .split(' ')
            .count(),
        2
    );
    assert!(
        std::fs::read_to_string(tmp.0.join("2026.beancount"))
            .unwrap()
            .contains("50.00 USD")
    );

    trunk.archive("car-later").unwrap();
    assert!(trunk.scenarios().unwrap().is_empty());
    let kept = git(
        &tmp.0,
        &["for-each-ref", "--format=%(refname)", "refs/tags/archive/"],
    );
    assert!(
        kept.starts_with("refs/tags/archive/scenarios/car-later-"),
        "{kept}"
    );
}

#[test]
fn a_scenario_and_trunk_changing_the_same_lines_is_a_conflict() {
    let tmp = repo();
    let trunk = GitStore::new(&tmp.0, None);
    trunk.fork("rent", "").unwrap();
    write(
        &trunk.scenario("rent").unwrap(),
        "main.beancount",
        "; the scenario's\n",
        "s-1",
    );
    write(&trunk, "main.beancount", "; trunk's\n", "t-1");
    assert_eq!(
        trunk.merged("rent").unwrap(),
        Merged::Conflicts(vec!["main.beancount".into()])
    );
}

/// A bare remote holding the fixture ledger, and two devices that cloned it over a file path.
fn remote_and_devices() -> (TempDir, [(TempDir, GitStore); 2]) {
    let work = repo();
    let remote = TempDir::new("remote");
    git(&remote.0, &["init", "-q", "--bare", "-b", "main"]);
    git(&work.0, &["push", "-q", remote.0.to_str().unwrap(), "main"]);
    let device = |name: &str| {
        let dir = TempDir::new(name);
        let who = Author {
            name: name.into(),
            email: format!("{name}@example.invalid"),
        };
        let url = remote.0.to_str().unwrap();
        let store = GitStore::clone_from(url, dir.0.join("book"), None, Some(who)).unwrap();
        (dir, store)
    };
    let devices = [device("laptop"), device("phone")];
    (remote, devices)
}

/// `text` added to the end of a file in one commit.
fn append(store: &GitStore, path: &str, text: &str, op_id: &str) -> String {
    let before = store.snapshot().unwrap().files[path].clone();
    write(store, path, &format!("{before}{text}"), op_id)
}

const ROOT: &str = "main.beancount";

#[test]
fn sync_pushes_from_one_device_and_the_other_pulls() {
    let (remote, [(_l, laptop), (_p, phone)]) = remote_and_devices();
    let v = append(&laptop, "2026.beancount", "; from the laptop\n", "l-1");
    assert_eq!(laptop.sync(ROOT, None).unwrap(), Synced::Pushed);
    assert_eq!(git(&remote.0, &["rev-parse", "main"]).trim(), v);

    assert_eq!(phone.sync(ROOT, None).unwrap(), Synced::Pulled);
    assert_eq!(phone.head(), v);
    let file = std::fs::read_to_string(phone.dir().join("2026.beancount")).unwrap();
    assert!(file.ends_with("; from the laptop\n"));
    assert_eq!(git(phone.dir(), &["status", "--porcelain"]), "");
    assert_eq!(phone.sync(ROOT, None).unwrap(), Synced::UpToDate);
}

#[test]
fn sync_merges_changes_from_both_devices_in_one_commit() {
    let (remote, [(_l, laptop), (_p, phone)]) = remote_and_devices();
    append(&laptop, "main.beancount", "; from the laptop\n", "l-1");
    append(&phone, "2026.beancount", "; from the phone\n", "p-1");
    assert_eq!(laptop.sync(ROOT, None).unwrap(), Synced::Pushed);

    assert_eq!(phone.sync(ROOT, None).unwrap(), Synced::Merged);
    let files = phone.snapshot().unwrap().files;
    assert!(files["main.beancount"].ends_with("; from the laptop\n"));
    assert!(files["2026.beancount"].ends_with("; from the phone\n"));
    let parents = git(phone.dir(), &["log", "-1", "--format=%P"]);
    assert_eq!(parents.split_whitespace().count(), 2);
    assert!(git(phone.dir(), &["log", "-1", "--format=%B"]).contains("Op-Type: sync"));
    assert_eq!(git(phone.dir(), &["status", "--porcelain"]), "");
    assert_eq!(git(&remote.0, &["rev-parse", "main"]).trim(), phone.head());

    assert_eq!(laptop.sync(ROOT, None).unwrap(), Synced::Pulled);
    assert_eq!(laptop.head(), phone.head());
    git(&remote.0, &["fsck", "--strict"]);
}

#[test]
fn sync_stops_when_both_devices_change_the_same_lines() {
    let (remote, [(_l, laptop), (_p, phone)]) = remote_and_devices();
    write(&laptop, "main.beancount", "; the laptop's\n", "l-1");
    let mine = write(&phone, "main.beancount", "; the phone's\n", "p-1");
    let theirs = laptop.head();
    laptop.sync(ROOT, None).unwrap();

    let r = phone.sync(ROOT, None);
    assert!(
        matches!(&r, Err(StoreError::Refused(m)) if m.contains("main.beancount")),
        "{r:?}"
    );
    assert_eq!(phone.head(), mine);
    assert_eq!(git(&remote.0, &["rev-parse", "main"]).trim(), theirs);
}

#[test]
fn sync_refuses_a_merge_that_breaks_a_balance_assertion() {
    let (remote, [(_l, laptop), (_p, phone)]) = remote_and_devices();
    // Each is fine alone: Checking is 2,169.75 after the January paycheck.
    let assertion = "\n2026-02-01 balance Assets:Checking 2169.75 USD\n";
    append(&laptop, "main.beancount", assertion, "l-1");
    let spend =
        "\n2026-01-20 * \"Market\"\n  Expenses:Food:Groceries 30.00 USD\n  Assets:Checking\n";
    let mine = append(&phone, "2026.beancount", spend, "p-1");
    let theirs = laptop.head();
    laptop.sync(ROOT, None).unwrap();

    let r = phone.sync(ROOT, None);
    assert!(
        matches!(&r, Err(StoreError::Refused(m)) if m.contains("ledger error")),
        "{r:?}"
    );
    assert_eq!(phone.head(), mine);
    assert_eq!(git(&remote.0, &["rev-parse", "main"]).trim(), theirs);
}

#[test]
fn a_book_with_no_remote_is_not_synced() {
    let tmp = repo();
    let r = GitStore::new(&tmp.0, None).sync(ROOT, None);
    assert!(matches!(r, Err(StoreError::Refused(_))), "{r:?}");
}

#[test]
fn a_new_book_takes_its_first_commit_and_git_reads_it() {
    let tmp = TempDir::new("init");
    let who = Author {
        name: "ann".into(),
        email: "ann@example.invalid".into(),
    };
    let store = GitStore::init(tmp.0.join("book"), Some(who)).unwrap();
    assert_eq!(store.head(), "");
    let v = write(&store, "main.beancount", "; a new book\n", "n-1");
    assert_eq!(git(store.dir(), &["rev-parse", "main"]).trim(), v);
    assert_eq!(
        git(store.dir(), &["log", "-1", "--format=%an"]).trim(),
        "ann"
    );
    assert_eq!(git(store.dir(), &["status", "--porcelain"]), "");
    git(store.dir(), &["fsck", "--strict"]);
}

#[test]
fn refuses_a_path_outside_the_book() {
    let tmp = repo();
    let store = GitStore::new(&tmp.0, None);
    let before = store.head();
    for path in [
        "../escaped.beancount",
        ".git/hooks/pre-commit",
        "a\\b.beancount",
        "C:x.bean",
    ] {
        let change = FileChange {
            path: path.into(),
            text: Some("; no\n".into()),
        };
        let r = store.commit_as(&before, &[change], &info("p", "p"), None);
        assert!(matches!(r, Err(StoreError::Refused(_))), "{path}: {r:?}");
    }
    assert_eq!(store.head(), before);
    assert!(!tmp.0.parent().unwrap().join("escaped.beancount").exists());
}

#[test]
fn the_author_comes_from_the_books_own_git_config() {
    let tmp = repo();
    git(&tmp.0, &["config", "user.name", "Book Owner"]);
    git(&tmp.0, &["config", "user.email", "owner@example.invalid"]);
    let who = GitStore::new(&tmp.0, None).configured_author().unwrap();
    assert_eq!(
        (who.name.as_str(), who.email.as_str()),
        ("Book Owner", "owner@example.invalid")
    );
}

#[test]
fn a_local_book_given_a_remote_pushes_to_it_and_can_drop_it() {
    let tmp = repo();
    let local = GitStore::new(&tmp.0, None);
    assert_eq!(local.remote_url(), None);
    // An empty bare remote, so the first sync pushes the whole book.
    let bare = TempDir::new("bare");
    git(&bare.0, &["init", "-q", "--bare", "-b", "main"]);
    local.set_remote(Some(bare.0.to_str().unwrap())).unwrap();
    assert!(local.has_remote());
    assert_eq!(local.sync(ROOT, None).unwrap(), Synced::Pushed);
    assert_eq!(git(&bare.0, &["rev-parse", "main"]).trim(), local.head());
    local.set_remote(None).unwrap();
    assert!(!local.has_remote() && local.remote_url().is_none());
}

#[test]
fn moving_a_book_to_a_new_empty_remote_pushes_it_there() {
    let tmp = repo();
    let local = GitStore::new(&tmp.0, None);
    let (old, new) = (TempDir::new("bare"), TempDir::new("bare"));
    git(&old.0, &["init", "-q", "--bare", "-b", "main"]);
    git(&new.0, &["init", "-q", "--bare", "-b", "main"]);
    local.set_remote(Some(old.0.to_str().unwrap())).unwrap();
    assert_eq!(local.sync(ROOT, None).unwrap(), Synced::Pushed);

    local.set_remote(Some(new.0.to_str().unwrap())).unwrap();
    assert_eq!(local.sync(ROOT, None).unwrap(), Synced::Pushed);
    assert_eq!(git(&new.0, &["rev-parse", "main"]).trim(), local.head());
    assert_eq!(local.sync(ROOT, None).unwrap(), Synced::UpToDate);
}

#[test]
fn a_token_is_refused_for_anything_but_https() {
    let tmp = repo();
    let local = GitStore::new(&tmp.0, None);
    local
        .set_remote(Some("http://example.invalid/b.git"))
        .unwrap();
    for r in [
        local.sync(ROOT, Some("secret")).map(|_| ()),
        local.sync_refs(Some("secret")).map(|_| ()),
        local.replace_remote(Some("secret")),
    ] {
        assert!(
            matches!(&r, Err(StoreError::Refused(m)) if m.contains("HTTPS")),
            "{r:?}"
        );
    }
    let fresh = TempDir::new("clone");
    let r = GitStore::clone_from(
        "http://example.invalid/b.git",
        fresh.0.join("b"),
        Some("secret"),
        None,
    );
    assert!(matches!(r, Err(StoreError::Refused(_))));
}

/// A commit on `remote` with a file at `path`, which git itself refuses to check out: made with
/// plumbing, as a hostile remote would.
fn hostile_commit(remote: &Path, path: &str) {
    let work = TempDir::new("attacker");
    git(remote, &["config", "receive.fsckObjects", "false"]);
    let from = remote.to_str().unwrap();
    git(&work.0, &["clone", "-q", from, "c"]);
    let dir = work.0.join("c");
    std::fs::write(dir.join("payload"), "#!/bin/sh\necho owned\n").unwrap();
    let id = git(&dir, &["hash-object", "-w", "payload"]);
    let entry = format!("100644,{},{path}", id.trim());
    git(
        &dir,
        &[
            "-c",
            "core.protectNTFS=false",
            "update-index",
            "--add",
            "--cacheinfo",
            &entry,
        ],
    );
    git(&dir, &["config", "user.name", "a"]);
    git(&dir, &["config", "user.email", "a@example.invalid"]);
    git(
        &dir,
        &["-c", "core.protectNTFS=false", "commit", "-q", "-m", "x"],
    );
    git(&dir, &["push", "-q", "origin", "HEAD:main"]);
}

#[test]
fn sync_refuses_a_remote_file_that_would_land_inside_git() {
    let (remote, [(_l, laptop), _]) = remote_and_devices();
    hostile_commit(&remote.0, "git~1/hooks/post-commit");
    let r = laptop.sync(ROOT, None);
    assert!(
        matches!(&r, Err(StoreError::Refused(m)) if m.contains("not a ledger path")),
        "{r:?}"
    );
    assert!(!laptop.dir().join(".git/hooks/post-commit").exists());
}

#[test]
fn clone_does_not_write_a_remote_file_inside_git() {
    let (remote, _devices) = remote_and_devices();
    hostile_commit(&remote.0, "git~1/hooks/post-commit");
    let fresh = TempDir::new("clone");
    let at = fresh.0.join("b");
    let _ = GitStore::clone_from(remote.0.to_str().unwrap(), &at, None, None);
    assert!(!at.join(".git/hooks/post-commit").exists());
}

#[test]
fn a_save_cut_off_before_updating_the_files_does_not_lock_the_book() {
    let tmp = repo();
    let mut ws = open(GitStore::new(&tmp.0, None));
    let old = git(&tmp.0, &["rev-parse", "HEAD"]).trim().to_owned();
    ws.apply(&assign("1.00"), None, 0).unwrap();
    let new = git(&tmp.0, &["rev-parse", "HEAD"]).trim().to_owned();
    // The branch moved, the files and index did not.
    git(&tmp.0, &["reset", "-q", "--hard", &old]);
    git(&tmp.0, &["update-ref", "refs/heads/main", &new]);

    let mut ws = open(GitStore::new(&tmp.0, None));
    ws.apply(&assign("2.00"), None, 0).unwrap();
    assert_eq!(git(&tmp.0, &["status", "--porcelain"]), "");
    let text = std::fs::read_to_string(tmp.0.join("2026.beancount")).unwrap();
    assert!(
        text.contains("1.00 USD") && text.contains("2.00 USD"),
        "{text}"
    );
}

#[test]
fn a_file_that_is_not_utf8_is_refused_by_name_not_damaged() {
    let tmp = repo();
    std::fs::write(tmp.0.join("latin.bean"), b"; caf\xe9\n").unwrap();
    git(&tmp.0, &["add", "latin.bean"]);
    git(&tmp.0, &["commit", "-q", "-m", "Latin-1"]);
    let r = GitStore::new(&tmp.0, None).snapshot();
    assert!(
        matches!(&r, Err(StoreError::Refused(m)) if m.contains("latin.bean")),
        "{r:?}"
    );
}

#[test]
fn a_new_book_keeps_text_as_committed() {
    let tmp = TempDir::new("init");
    GitStore::init(tmp.0.join("b"), None).unwrap();
    assert_eq!(
        git(&tmp.0.join("b"), &["config", "--local", "core.autocrlf"]).trim(),
        "false"
    );
}

#[test]
fn scenarios_and_closes_travel_and_so_does_setting_them_aside() {
    let (remote, [(_l, laptop), (_p, phone)]) = remote_and_devices();
    let head = laptop.head();
    laptop.fork("rent", &head).unwrap();
    laptop.close("2026-01".parse().unwrap(), None).unwrap();
    assert_eq!(laptop.sync_refs(None).unwrap(), Vec::<String>::new());
    assert!(
        git(&remote.0, &["for-each-ref", "--format=%(refname)"])
            .contains("refs/heads/scenarios/rent")
    );

    assert_eq!(phone.sync_refs(None).unwrap(), Vec::<String>::new());
    assert!(phone.scenario("rent").is_some());
    assert_eq!(phone.closes().unwrap().len(), 1);

    // The phone works on the scenario; the laptop fast-forwards to it.
    write(
        &phone.scenario("rent").unwrap(),
        "main.beancount",
        "; the phone's\n",
        "p-1",
    );
    phone.sync_refs(None).unwrap();
    laptop.sync_refs(None).unwrap();
    assert_eq!(
        laptop.scenario("rent").unwrap().head(),
        phone.scenario("rent").unwrap().head()
    );

    // Set aside on the laptop: gone on the remote, then on the phone, both keeping the archive.
    laptop.archive("rent").unwrap();
    laptop.reopen("2026-01".parse().unwrap()).unwrap();
    laptop.sync_refs(None).unwrap();
    let on_remote = git(&remote.0, &["for-each-ref", "--format=%(refname)"]);
    assert!(
        !on_remote.contains("refs/heads/scenarios/") && !on_remote.contains("refs/tags/close/"),
        "{on_remote}"
    );
    assert!(on_remote.contains("refs/tags/archive/scenarios/rent-"));
    phone.sync_refs(None).unwrap();
    assert!(phone.scenario("rent").is_none());
    assert!(phone.closes().unwrap().is_empty());
}

#[test]
fn a_scenario_changed_on_both_devices_is_kept_and_named() {
    let (_remote, [(_l, laptop), (_p, phone)]) = remote_and_devices();
    let head = laptop.head();
    laptop.fork("rent", &head).unwrap();
    laptop.sync_refs(None).unwrap();
    phone.sync_refs(None).unwrap();
    write(
        &laptop.scenario("rent").unwrap(),
        "main.beancount",
        "; the laptop's\n",
        "l-1",
    );
    write(
        &phone.scenario("rent").unwrap(),
        "main.beancount",
        "; the phone's\n",
        "p-1",
    );
    laptop.sync_refs(None).unwrap();
    let notes = phone.sync_refs(None).unwrap();
    assert_eq!(
        notes,
        ["scenario rent changed both here and on another device: kept here"]
    );
    assert!(
        phone.scenario("rent").unwrap().snapshot().unwrap().files["main.beancount"]
            .contains("phone")
    );
}

#[test]
fn a_separate_book_is_refused_by_sync_and_can_replace_the_remote_keeping_its_history() {
    let (remote, [(_l, laptop), (_p, phone)]) = remote_and_devices();
    // As a protected branch on a git host: no force pushes.
    git(
        &remote.0,
        &["config", "receive.denyNonFastForwards", "true"],
    );
    let head = laptop.head();
    laptop.fork("old-plan", &head).unwrap();
    laptop.sync_refs(None).unwrap();
    let old = git(&remote.0, &["rev-parse", "main"]).trim().to_owned();

    // A new book with its own history, given the same remote.
    let fresh = TempDir::new("fresh");
    let who = Author {
        name: "ann".into(),
        email: "ann@example.invalid".into(),
    };
    let book = GitStore::init(fresh.0.join("book"), Some(who)).unwrap();
    write(
        &book,
        "main.beancount",
        "option \"operating_currency\" \"USD\"\n",
        "n-1",
    );
    book.set_remote(Some(remote.0.to_str().unwrap())).unwrap();
    let r = book.sync(ROOT, None);
    assert!(
        matches!(&r, Err(StoreError::Refused(m)) if m.contains("separate books")),
        "{r:?}"
    );

    book.replace_remote(None).unwrap();
    assert_eq!(git(&remote.0, &["rev-parse", "main"]).trim(), book.head());
    let refs = git(
        &remote.0,
        &["for-each-ref", "--format=%(refname) %(objectname)"],
    );
    assert_eq!(
        git(&remote.0, &["rev-parse", "main^2"]).trim(),
        old,
        "the old trunk is kept in history"
    );
    assert_eq!(
        book.snapshot().unwrap().files.len(),
        1,
        "with this book's files"
    );
    assert!(
        refs.contains("refs/tags/archive/replaced-") && refs.contains("/heads/scenarios/old-plan"),
        "{refs}"
    );
    assert!(!refs.contains("refs/heads/scenarios/"), "{refs}");
    assert_eq!(book.sync(ROOT, None).unwrap(), Synced::UpToDate);
    // A device with the old book takes the new one.
    assert_eq!(phone.sync(ROOT, None).unwrap(), Synced::Pulled);
    assert_eq!(phone.head(), book.head());
}
