# BranchBooks

BranchBooks is an open-source double-entry accounting app with envelope and target budgeting on a
Beancount-compatible plain-text ledger, installed on Windows (Android planned). Owner: Joshua Crump,
who holds the rights personally. License: GPL-3.0-only. Repository:
https://github.com/BranchBooks/branchbooks.

- [docs/BUDGET-RULES.md](docs/BUDGET-RULES.md): the budget, target and reconcile rules, and the worked
  examples the code must match.
- [docs/DESIGN.md](docs/DESIGN.md): how it works and why. Update it when a design choice changes.
- [README.md](README.md): install, setup, the CLI and the layout.

## Decisions to keep

- **Rust for everything:** the core, the git store, the CLI, the apps and the oracle, in `crates/`. Python
  only for Beancount and the reference models, run as separate programs. Money is exact decimals
  (`rust_decimal`), never floats.
- **The UI is Dioxus** (`crates/branchbooks-ui`): one Rust interface for Windows and Android.
- **Development is on native Windows, with no Docker.** CI runs directly on GitHub's runners, on Linux and Windows.
- **Installed apps, no server.** Every book is a git repository on the device, local only or synced over HTTPS
  with a token to a git remote, the source of truth. No server or browser dependencies in the core. No logins:
  the git host's access control replaces them.
- **The git store is grit-lib** (pure Rust), so no `git` program is needed. Sync stops, pushing nothing, on a text
  conflict or when a merge would add ledger errors.
- **Canonical format: Beancount text.** Budget entries and targets are `custom` directives, so files stay valid
  Beancount. Storage: one file per year, with `main.beancount` holding options and account open/close.
- **Windows:** `x86_64-pc-windows-msvc`, one exe, installed and updated with Velopack from GitHub releases.
- **One oracle per question.** Real Beancount 3.2.3 for ledger behaviour; the Python models in `reference/` for
  budget rules. Add a new external check only when it answers a question no current check answers.

## What must keep passing

1. All the worked examples, as automated tests.
2. The oracle comparison on every fixture: the same account balances as real Beancount 3.2.3, and the same errors.
3. A full recompute always equals the incremental result.
4. The benchmark (`cargo bench -p branchbooks-core`) prints load times at 5,000, 20,000 and 100,000 transactions.
   Report them next to real Beancount 3.2.3's reference numbers (0.3 s, 2.5 s and 15.4 s), without hiding a slow
   result.
5. The CLI prints the budget table for the fixtures.

## Working rules

- Keep all money as exact decimals. Never use floating-point arithmetic for amounts.
- Do not link, vendor or copy Beancount or rustledger code, and do not transcribe rustledger's source. Use them only
  as separate programs or references. Linking would limit relicensing.
- If something in the rules turns out to be wrong or unclear, record the answer in the docs instead of silently
  deviating.
- Do not commit secrets or real financial data. Fixtures are synthetic; real ledgers go in `private/`.
- Do not push or open pull requests unless asked.
- No npm in this repository. Anything that needs npm, such as a VS Code extension, lives in a separate repository.
- Write the docs as what BranchBooks is now, in the present tense, with no project history.

## Not built yet

Android, the rest of Beancount's syntax, custom reports, bank connections (SimpleFIN is planned first) and foreign
currency.
