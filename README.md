<h1><img src="crates/branchbooks-ui/icons/wordmark.svg" alt="BranchBooks" height="56"></h1>

BranchBooks is a double-entry accounting app with envelope budgeting and target budgeting, installed
on your own device. Your books are plain-text, Beancount-compatible ledgers in git: stay local, or
sync over HTTPS to any git host you choose. There is no BranchBooks server and no account to create.

**Early software.** Keep your own copy of anything you put in it.

## Install on Windows

Run `BranchBooks-setup.exe` from the [latest release](https://gitlab.com/branchbooks/branchbooks/-/releases/permalink/latest).
It installs for your user alone, with no admin prompt, in `%LOCALAPPDATA%\Programs\BranchBooks`; run it
normally, not as administrator, which it refuses. It needs Microsoft's WebView2 runtime, which Windows 11
and an up-to-date Windows 10 already have; if it is missing, Setup says where to get it.

**Windows will warn you.** The app isn't code-signed yet, so Microsoft Defender SmartScreen may say
it is unrecognized when you first open it. If you got it from this repository, choose More info,
then Run anyway. Every file in a release is signed with [Sigstore](https://www.sigstore.dev) by the
pipeline that built it. To check a download was built from this repository, download its
`.sigstore.json` beside it and run [cosign](https://docs.sigstore.dev/cosign/system_config/installation/):

```sh
cosign verify-blob BranchBooks-setup.exe --bundle BranchBooks-setup.exe.sigstore.json \
  --certificate-oidc-issuer https://gitlab.com \
  --certificate-identity-regexp '^https://gitlab.com/branchbooks/branchbooks//\.gitlab-ci\.yml@refs/tags/v'
```

**Updates.** Each time it starts, the app reads `branchbooks.dev/updates/latest.txt`, which forwards
to the latest release on GitLab, and downloads a newer installer in the background, checking its
SHA-256 against that file. "Restart to update" then appears beside the book switcher: it runs the
installer silently and reopens the app. If you ignore it, the button is back the next time you open
the app, without downloading again. Those requests, to `branchbooks.dev` and then `gitlab.com`, are
the only network traffic BranchBooks makes apart from syncing books with the remote you chose, and
they carry nothing about you or your books. Offline, the app just runs as it is.

**Uninstalling** (Windows' Installed apps) removes the app, then asks whether to delete your books
and settings too, in `%LOCALAPPDATA%\BranchBooks Data`; the answer is No unless you choose Yes. Books
you sync to a remote are still there either way.

## What it does

**The Windows app** (`crates/branchbooks-ui`):

- **Books:** each one a git repository in `%LOCALAPPDATA%\BranchBooks Data\books`, local only or a clone of
  a remote. A switcher, and Add book: open a book from git, to use one book on several devices (a
  clone over HTTPS with a token, kept in Windows Credential Manager, never in a file), or start a
  new one, empty, the sample ledger or a GnuCash file (compressed XML, full history). A new device
  starts with no books and offers the same choice.
- **Sync:** fetch, then push, fast-forward, or merge in one commit. It stops with nothing pushed on
  a text conflict or new ledger errors. It runs when a book opens, after each save, and by button;
  scenarios, closed months and what was set aside travel too.
- **Views:** Budget (Assigned, targets, paying off loans and cards, moving money, the month's entries and notes,
  target shortfalls, the ledger's errors), Journal (add, edit, delete, filtered by an account or
  category), Accounts (add by kind, with today's balance; edit, rename everywhere, reconcile, close,
  reopen, delete), Overview,
  Reports, Files (errors, import, history), Scenarios (start, open, compare, merge, set aside) and
  closing a month.
- **Settings**, from the foot of the sidebar: the book's name, remote and token, who commits (the
  book's own git identity) and deleting it; light, dark or matching Windows; checking for updates;
  and where the books and the crash log are kept.

**Underneath:** one Rust engine for the app and the CLI: parser, ledger, envelope budget, target
budgeting, reports and the write path. The git store is grit-lib (pure Rust), so no `git` program is
needed. Every fixture is checked in CI against real Beancount 3.2.3, run as a separate program.

**Not yet:**

- **Android:** planned; it needs an Android build setup and Android storage for books and tokens
  ([#19](https://gitlab.com/branchbooks/branchbooks/-/issues/19),
  [#18](https://gitlab.com/branchbooks/branchbooks/-/issues/18)).
- **The rest of Beancount's syntax.** `pad`, tag and metadata stacks (`pushtag`, `pushmeta`), and costs
  and prices on postings are reported as syntax errors, never miscounted. `price` and `query`
  directives are read and ignored, and plugins are not run.
- **Bank connections and foreign currency.** One budget currency; accounts in other currencies are
  off-budget.

## Layout

| Path | What |
|---|---|
| `crates/` | All of BranchBooks, in Rust. `branchbooks-core` is the engine (parser, loader, ledger, budget, targets, reports and the write path's planning); `branchbooks-git` is the git store, on grit-lib, with sync; `branchbooks-gnucash` reads GnuCash files; `branchbooks-ui` is the app (Dioxus), with its books in `src/books.rs`, its updates in `src/update.rs`, and the logo, icons and splash in `icons/`; `branchbooks-cli` is the `branchbooks` command; `branchbooks-oracle` compares ledgers with real Beancount |
| `tools/ci` | `install-cargo-deny.sh`: the licence and advisory checker, at a fixed version and checksum |
| `tools/windows` | `build.ps1`: the Windows app, built on Windows into `dist\windows\` |
| `.gitlab-ci.yml` | The checks, and publishing a release from a version tag |
| `fixtures/` | Synthetic ledgers: the worked examples, the multi-file layout, targets, balance assertions, reconcile, and error cases |
| `reference/` | Python models of the budget rules (`sim4.py`, `sim5.py`), whose assertions the core is held to |
| `tools/oracle` | `beancount_dump.py`: real Beancount 3.2.3's balances and errors for a ledger, as JSON, for `branchbooks-oracle` (a separate program, never linked); `requirements.txt`: its exact versions |
| `docs/` | `BUDGET-RULES.md`: the budget, target and reconcile rules, with worked examples; `DESIGN.md`: how it works and why |
| `CLAUDE.md` | Working rules for AI coding tools |

## Setup

Everything is Rust, plus Python for the Beancount oracle. No Node or npm. The app is built on Windows.

### On Windows

Install [Rust](https://rustup.rs), [git](https://git-scm.com) (the tests use it) and Visual Studio Build
Tools with its "Desktop development with C++" workload, then work on a Dev Drive if you can: builds read
thousands of small files.

```powershell
dx serve -p branchbooks-ui --platform desktop   # run the app, with live UI reloading; its own books
tools\windows\build.ps1                         # the release app, into dist\windows
```

A dev build (a debug build such as `dx serve`, or one built with `BRANCHBOOKS_CHANNEL=dev`) keeps its books in
`%LOCALAPPDATA%\BranchBooks-dev\books`, never your real ones;
set `BRANCHBOOKS_BOOKS` to a folder to use that instead, in either build.

`dx` is the Dioxus CLI, at the same version as `dioxus` in `crates/branchbooks-ui/Cargo.toml`. You only
need it for `dx serve`.

### Tests and the oracle

These work on Windows and Linux, with Rust, git and Python:

```sh
cargo test                                       # the core, git store, CLI, oracle and app
cargo clippy --all-targets
cargo bench -p branchbooks-core                  # load times, beside Beancount's
# The oracle: real Beancount in its own virtualenv, a separate program
python3 -m venv tools/oracle/.venv
tools/oracle/.venv/bin/pip install -r tools/oracle/requirements.txt    # Windows: tools\oracle\.venv\Scripts\pip
cargo run -q -p branchbooks-oracle
```

### CI and releases

CI (`.gitlab-ci.yml`) runs the checks on Linux, including the oracle, and the lints and tests on real
Windows, for merge requests and for `main`. Run by hand (Build > Pipelines > Run pipeline), it also builds
and packs the Windows app and keeps it for 30 days as a download from the job, and runs the benchmark and
the reference simulations, as it does weekly from a pipeline schedule. `deny.toml` sets which licences and
crates are allowed, and Renovate (`renovate.json`) proposes updates weekly.

To release, set `version` in the root `Cargo.toml`, merge, then push a matching tag on `main`
(`git tag v0.1.0 && git push origin v0.1.0`). The tag's pipeline runs the same checks, builds the
installer with [Inno Setup](https://jrsoftware.org/isinfo.php) (`tools/windows/branchbooks.iss`), signs
every file, and publishes `BranchBooks-setup.exe` and `latest.txt` as a GitLab release. Installed apps
read `latest.txt` through `branchbooks.dev/updates`, which the website forwards to the latest release. Run the pipeline by hand first to rehearse: it builds and packs,
keeps the result as a download from the job, and publishes nothing.

The repository lives on [GitLab](https://gitlab.com/branchbooks/branchbooks); GitHub has a read-only
mirror of `main` and the tags. Issues and merge requests go on GitLab.

### Testing with your own ledger

Real ledgers stay out of the repository and CI. Put a copy in `private/` at the repository root
(gitignored): a folder with its `main.beancount` and year files, or a single `.bean` file. Then
`cargo run -q -p branchbooks-oracle` also compares each one with Beancount 3.2.3, and checks it has
no ledger errors, a budget for every month it covers, and the same balances after an import into
an empty ledger.

`fixtures/gnucash-import/` is a synthetic ledger shaped like a GnuCash import, for CI.

## CLI

`crates/branchbooks-cli` builds the `branchbooks` command: `cargo install --path crates/branchbooks-cli`, or
`cargo run -q -p branchbooks-cli -- budget <file> --month YYYY-MM` from the repository.

```
$ branchbooks budget fixtures/scenarios/ex09-mixed-overspend.bean --month 2026-11 --today 2026-11-06
Budget for 2026-11 (USD); Unassigned as of 2026-11-06

Envelope                        Carried     Assigned        Moved     Activity    Available  Overspent
Expenses:Food:Dining             155.00         0.00         0.00      -230.00       -75.00  cash 30.00 / credit 45.00
Expenses:Food:Groceries            0.00         0.00         0.00         0.00         0.00
Expenses:Housing:Rent          1,500.00         0.00         0.00         0.00     1,500.00
Expenses:Savings:Investing         0.00       500.00         0.00      -500.00         0.00

Liability payment envelope    Available         Owed  Underfunded
Liabilities:CreditCard           155.00       200.00         0.00
  45.00 overspent in Expenses:Food:Dining: cover it there

Assigned to future months          0.00
Unassigned                     1,080.00
```

`--today` defaults to the local date. Unassigned is computed for today's month and is the same
on every month view (docs/DESIGN.md, decision 6). A ledger with targets gets a Target column (what target
funding put in that month) and a `shortfall:` line for each target that month could not fully fund.
Card overspending still in a category is named under its liability, to cover there (docs/BUDGET-RULES.md,
rule 7).

`branchbooks check <file> [--json]` prints ledger errors and account balances, in the same shape
the oracle compares.

## Write path

Every change is one operation: add, edit or delete an entry, where an entry is a transaction, an
`envelope-assign`, an `envelope-move`, a `custom "target"`, or an account open or close. A
`reconcile` operation clears pending transactions and writes a `balance` assertion in one commit. The `Workspace` turns the
operation into a minimal text edit, so nothing outside the edited lines changes. It then reloads
the ledger with the change applied and commits one version to the store.

- **Where entries go.** Transactions and envelope entries go to their year file (`2026.beancount`)
  when `main.beancount` includes year files, and to the root file otherwise. Opens and closes go
  to the root file, next to the other opens. A new year gets its file, and an `include` in year
  order.
- **What is refused.** An operation is refused if it would add a ledger error, such as an
  unbalanced transaction, an unknown or inactive account, or a `balance` assertion that no longer
  holds, or if an assign or move would
  overspend a category (docs/DESIGN.md, decision 2). A transaction that makes an existing move blocked
  is committed with a warning.
- **Stale reads.** Edits and deletes address an entry by file, line and a fingerprint of its text.
  If the entry has changed since it was read, the operation is refused.
- **Commits.** Every backend writes one commit per operation with `Op-Id` and `Op-Type` trailers.
  A commit based on an old version is a conflict, which can reload and re-apply automatically.
  Repeating an operation with the same Op-Id changes nothing.
  The git store (grit-lib, no `git` program) commits with an atomic compare-and-swap of the branch,
  works on bare repositories, refuses file paths outside the book, and
  refuses to commit over uncommitted hand edits to a ledger file.

```
$ branchbooks assign ~/ledger --date 2026-01-02 --category Expenses:Food:Groceries --amount 50.00
committed 7b6d54a491  assign  Op-Id c9bc3828-3bf6-40b6-83fe-e98ed068e6ec
changed: 2026.beancount; entry at 2026.beancount:5

$ branchbooks move ~/ledger --date 2026-01-03 --from Expenses:Food:Groceries --to Expenses:Housing:Rent --amount 9999.00
refused (blocked): 2026-01-03 envelope-move: it would leave Expenses:Food:Groceries at -9779.25

$ branchbooks txn ~/ledger --date 2027-01-04 --payee Market --narration Groceries \
    --posting "Expenses:Food:Groceries 12.50" --posting "Liabilities:Visa"
committed ae36c90dfe  add-transaction  Op-Id 7316b822-e693-4f72-95e7-842526c32a74
changed: 2027.beancount, main.beancount; entry at 2027.beancount:1

$ branchbooks log ~/ledger
ae36c90dfe  add-transaction  Transaction 2027-01-04 "Market: Groceries"  [7316b822-...]
7b6d54a491  assign           Assign 50.00 USD to Expenses:Food:Groceries on 2026-01-02  [c9bc3828-...]
```

The commands are `assign`, `move`, `txn`, `open`, `close`, `delete --at FILE:LINE`, `log` and
`sync` (with the book's remote; an HTTPS token comes from `BRANCHBOOKS_TOKEN`, never an argument).
Run `branchbooks` with no arguments for the full usage.

## Budget flags

These are metadata on `open`:

```
2026-10-01 open Assets:Checking USD
  budget: "on"            ; or "off" (assets and liabilities)
2026-10-01 open Expenses:Tax:Federal USD
  budget: "excluded"      ; categories with no flag are budgeted
2026-10-01 open Expenses:Food
  budget: "placeholder"   ; only groups its sub-accounts: no envelope, no postings
```

Envelope entries and targets are Beancount `custom` directives, so the files stay valid Beancount:

```
2026-10-01 custom "envelope-assign" Expenses:Food:Groceries 600.00 USD
2026-10-10 custom "envelope-move" Expenses:Housing:Rent Expenses:Food:Groceries 20.00 USD
2026-01-01 custom "target" Expenses:Housing:Rent 1400.00 USD
  recurrence: "monthly"
2026-01-01 custom "target" Expenses:Savings:CarFund 6000.00 USD
  due: 2026-06-01
2026-10-01 custom "overspending" Expenses:Reimbursable "rollover"   ; or "reset"
```

Overspending not covered by month end resets the category and comes out of next month's Unassigned.
A category set to `"rollover"` carries it into next month as a negative balance instead
(docs/BUDGET-RULES.md, rules 6 and 6a). In the app, it is switched under a category's target editor.

## Contributing

Not accepting code contributions yet. Bug reports are welcome as issues; security problems go through
[SECURITY.md](SECURITY.md), not an issue.

## License

Copyright (C) 2026 Joshua Crump

BranchBooks is free software: you can redistribute it and/or modify it under the terms of the GNU
General Public License, version 3, as published by the Free Software Foundation. It comes with no
warranty. See [LICENSE](LICENSE).

The BranchBooks name and logo are trademarks, which the licence does not cover: see
[TRADEMARKS.md](TRADEMARKS.md).
