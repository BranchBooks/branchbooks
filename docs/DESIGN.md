# Design

How BranchBooks works and why. The budget's rules and worked examples are in
[BUDGET-RULES.md](BUDGET-RULES.md); this covers the choices around them.

## Shape

- **One engine, in Rust.** `branchbooks-core` is the parser, ledger, budget, reports and write path. It
  has no dependency on a file system, a network or a UI, and money is always an exact decimal.
- **Installed apps, no server.** The app is Dioxus on WebView2. Each book is a git repository on the
  device, local only or synced to a git remote, and the remote's own access control decides who can
  read it. There are no BranchBooks accounts.
- **Beancount text is the format.** Budget entries and targets are `custom` directives, so a book stays
  valid Beancount and opens in Beancount and Fava.
- **Git store without git.** `branchbooks-git` uses grit-lib, pure Rust, so no `git` program is needed
  on the device, and any git tool can open a book.
- **Tested from outside.** Real Beancount 3.2.3 runs as a separate program over every fixture and every
  ledger the write path produces, and account balances and errors must match. It is never linked.

## Budget decisions

Numbered because the code cites them.

**1. Rule 15 warns only for a correction dated after the spending.** Rule 14 applies transactions
before assignments on the same date, so a same-day assignment still acts as a correction, without a
warning. An assignment on the day after a purchase warns. Correcting a carried-in overspend in the next
month does not.

**2. Moves and negative assignments may not overspend a category.** One that would leave the category it
takes from below zero is skipped, changes nothing, and is reported as `blocked` in that month's view. A
move of exactly the Available is allowed. A negative move runs the other way and is checked on that side.
The write path refuses these when entered; the engine's check covers files edited by hand.

- Blocking is evaluated in date order, so a valid entry can become blocked when an earlier-dated
  transaction is added. The write path warns when that happens.
- Unassigned's "assigned to future months" sums future assignments as written. A negative future
  assignment that will be blocked still counts until its month.
- The credit portion is capped at the month's card spending, as a safety net.

**3. Several cards.** A category's credit portion is split across the cards it was charged to, in
account-name order, each capped at that card's spending. Payment envelopes and underfunding are per card.

**4. Mixed funding: cash first, then cards.** When postings in one transaction are funded by several
on-budget accounts, spending is charged to the cash outflow first, then to each card's charge in
account-name order. Refunds and other inflows match the same way against money arriving, and any remainder
counts as cash. A 200.00 Dining purchase paid 50.00 from checking and 150.00 by card, with 155.00
Available, leaves 45.00 of cash overspending and puts 150.00 in the card's payment envelope.

**5. Assets and liabilities with no `budget:` flag** are off-budget, with a budget diagnostic.

**6. Unassigned is based on the current date.** It is one "now" figure for the month containing today, the
same on every month view. "Assigned to future months" means months after today's month. Category rows show
the viewed month. The engine has no clock: callers pass today. Transactions dated later in the current
month count; the app marks them "upcoming".

**7. Money can be assigned straight to a card's payment envelope.**
`custom "envelope-assign" Liabilities:Card 50.00 USD` sets 50 aside to pay what the card already owes,
such as debt from before the budget. It carries forward like the rest of the envelope. A negative
assignment takes money back, but not below zero. Moves to or from a card are not allowed. A credit
overspend this month is covered in its category, not on the card: Underfunded leaves it out and names the
category (rule 7). Covering it on the card as well would fund the same purchase twice, and the card
is then flagged overfunded (rule 7a), with a Return button that lowers its Set aside.

**8. Card payments count whatever else the transaction holds.** Money arriving on a card that no refund
used is a payment. In a balance transfer the amount moves to the charged card's envelope and Unassigned
does not change. The whole amount moves even when the paid card's envelope held less, so totals always
agree and the result does not depend on what else happened that month. A cash advance (card to checking)
is not a transfer.

**9. Overspending resets at month end, unless a category rolls over.** By default, overspending not
corrected by month end leaves the category at 0 and comes out of the next month's Unassigned, the credit
portion going into the card's payment envelope (rule 6). The default and the rollover option work like
Actual Budget's, so overspending behaves as someone used to Actual expects.

- **Why reset.** Overspent money is already gone, so the next month's money to assign pays for it, and a
  category starts each month clean. A card's envelope ends the month holding everything its purchases
  cost, so overspending never turns into a card shortfall. Underfunded then only means debt from before
  the budget.
- **Why a rollover carries both portions.** A category that rolls over keeps the whole overspend in view,
  the way it was spent. Carrying only the cash would hide the card part in the payment envelope. The
  credit part waits in the category, so it is covered there, once, and only then reaches the card.
- **Why a dated entry.** The setting is a `custom "overspending"` entry, like a target (T1), not a flag on
  `open`: turning it on or off applies from that month, so past months, closed months and merged scenarios
  keep their numbers.

### Smaller rulings

- **Off-budget to on-budget money** is income under rule 3. Paying a card from an off-budget account lowers
  its payment envelope and raises Unassigned.
- **A category posting with no on-budget account** (an expense paid from the brokerage) is ignored by the
  budget and flagged `unfunded`.
- **Rule 8 and paychecks.** A paycheck paying into Checking and a 401k from income moves no on-budget money,
  so it is not flagged. A mortgage payment from Checking is, for its principal and escrow.
- **Rule 13.** An account flagged `on` whose open currencies exclude the budget currency is off-budget.
  Postings in other currencies on an on-budget account are ignored by the budget.
- **What the CLI shows.** Available = Carried + Assigned + Moved + Activity.
- **Placeholders.** A flag value rather than a new field, so an account cannot be both on budget and a
  placeholder. Beancount has no such idea and accepts a posting to one, so that is no ledger error: a posting
  written by hand or imported still counts, shows the row, and is flagged `placeholder`, so no money goes
  missing. An assignment, move or target naming one is flagged and changes nothing. The write path refuses new
  ones, and an account with transactions cannot become a placeholder. The app offers no Reconcile on one, since a
  statement belongs to a real account, and no Close, since closing a group would not close what is in it. The GnuCash import keeps GnuCash's own placeholders, unless one has
  transactions, which then comes over as an ordinary account and is named in the import's notes.

## Targets

Targets are `custom "target"` entries, not a format of their own. Allocations are computed, never written.
A savings goal is an Expenses category; the money stays in Checking until spent from the category. A monthly
`carry-shortfall` target catches up its full shortfall the next month.

Funding uses only the money on hand that month (T5), because a month must depend only on itself and earlier
months: the engine's month snapshots rely on that. So Unassigned can go negative when targets and a
future-dated assignment over-commit (T6), which shows the over-commitment.

`diff_books` compares two Books, such as the book and a scenario, in two layers: rules (targets in force, by
category and run of months) and effect (per category and month, the assignments, allocations, Available and
shortfall, plus Unassigned). It matches on category and month, so no ids are needed.

## Beancount compatibility

Beancount's behaviour is found by running it, and the oracle keeps it honest.

- **Supported:** `option`, `include`, `open`, `close`, `balance`, transactions with postings and metadata, and
  `custom`.
- **Accepted, no effect:** `commodity`, `price`, `event`, `note`, `document`, `query`. `plugin` lines are recorded
  and not run, so a ledger relying on a plugin will differ from Beancount.
- **Syntax errors, with file and line:** `pad`, cost `{...}` and price `@` annotations, `pushtag`/`poptag`,
  `pushmeta`/`popmeta`, arithmetic in amounts, multi-line strings, and renamed account roots. Never miscounted.
- **Custom entries.** Beancount does not check account references in `custom` entries, so these are not ledger
  errors. The budget reports an envelope entry naming a non-category as `envelope`.
- **Two elided amounts** are reported on the second elided posting's line, and the transaction is dropped.
- **File order** for rule 14 is the order files were first loaded (the root, then its includes depth-first), then
  the line. It affects only the budget's same-day order.
- **Options** are read from the root file only.
- **Numbers.** At most 20 integer digits and 28 after the point: a larger one is a syntax error. The 20-digit cap
  means sums of up to 10^8 amounts cannot overflow. A sum needing more than 28 digits in all is rounded to 28, as
  Python's default decimal context (Beancount's) does.
- **Dates and text.** Any valid calendar date from 0000 to 9999; a date with no such day is a syntax error. A byte
  order mark is an error, as it is in Beancount, and the message says to save the file as UTF-8 without BOM. Accepting
  it would make a book that Beancount and Fava can't open. Strings sort in code point order.

### Balance assertions

Checked as Beancount 3.2.3 does: at the start of the day (after the day's opens, before everything else), against
the account and all its sub-accounts, in the assertion's currency. The tolerance is the one written after `~`, else
one unit of the amount's last decimal place (0.01 for `10.00`, none for `10`). A failure is a `balance` error. The
budget ignores assertions, since they move no money. An assertion that already fails and still fails after a change
is not a new error, so it never blocks the change that fixes it.

## The write path

Every change is one operation and one commit, with `Op-Id` and `Op-Type` trailers. An operation becomes a minimal
text edit, so everything outside the edited lines stays byte for byte.

- **Entry identity** is file and line plus a fingerprint of the entry's text. No ids are written into the ledger. If
  the entry has moved, the same text is looked up: the nearest match in the same file, or a single match anywhere. If
  the text is gone, the operation is refused. Two byte-identical entries are interchangeable.
- **Placement.** Transactions and envelope entries go into their year file when the root includes `NNNN.beancount`
  files, otherwise the root, in date order after existing entries on the same date. Opens and closes go to the root
  beside the others. A new year creates its file and an `include` in year order.
- **Formatting.** New and edited entries use one style: two-space indent, amounts aligned within the entry, and
  normalised numbers. An edit rewrites the whole entry, so comments inside it are lost. Line breaks in a string are
  written `\n`. Line endings are kept per file. Deleting the last entry in a year file leaves the file and its include.
- **Refused or warned.** An operation that adds a ledger error is refused. An assign or move that would overspend a
  category, or that names a non-category, is refused. Any other operation that makes an existing envelope entry blocked
  is committed with a warning. The app also refuses a date more than 50 years from today, since one mistyped year makes
  the budget walk thousands of empty months on every later save.
- **Speed.** Transactions and envelope entries are validated incrementally: the changed entry is checked against the
  accounts, and the budget change is tried on the live engine and rolled back if refused. A randomized test compares the
  result with a fresh load after every step. Opens, closes, reconciles, imports and a new year's first entry reload the
  ledger. Back-dated writes cost more, since blocked moves are re-checked for every later month.
- **Retries.** A conflict reloads the latest version and applies the operation again; references resolve by content, so
  they survive other writers moving lines. A repeated Op-Id changes nothing.
- **Renames** rewrite every line naming the account or a sub-account, comments aside, in one commit. The type must stay
  (Assets stays Assets), and a name already taken is refused. Text in a narration that is exactly the account name is
  renamed too.

- **New accounts** are added by kind, which sets the root and the budget flag (a credit card is `Liabilities`, on budget), and
  named in words: "Chase Visa" is written `Liabilities:Chase-Visa`, as the account list shows hyphens as spaces. A balance today
  is a transaction against `Equity:Opening-Balances`, opened the first time it is needed. The open, that open and the
  transaction are separate commits, in that order, so a refused balance leaves the account opened and says so. An opening balance
  on cash arrives as money to assign (rule 3); on a card, it is owed and underfunded (decision 7 covers paying it down).

### Imports

An import previews what it would add. Only entries the ledger lacks are added: a transaction matches on its date and
postings (the elided amount filled in), not its words, since a bank file and a hand-entered copy describe a payment
differently. Other entries match on their text, ignoring spacing and comments. Counts are kept, so two identical payments
against one in the ledger add one. Accounts nobody opens are opened on first use. The imported files' options and plugins
are ignored. One commit, Op-Type `import`, refused on syntax errors or new ledger errors.

**GnuCash** (`branchbooks-gnucash`) reads a book saved as XML: the account tree, budget flags by type, and every
transaction. Shares and other currencies come over at their value in the transaction's currency, with the quantity as
metadata, and an account with such a split opens with no currency limit. The budget ignores postings outside its
currency. An unreadable amount is refused rather than read as zero. Scheduled transactions, prices and GnuCash budgets are
left out and counted in the import's notes.

### Reconcile

Beancount's `balance` counts every transaction up to its date, pending or not. So a pending transaction on or before the
statement date that the statement does not show would make the assertion fail: reconcile refuses then, and says to date it
after the statement. It never writes an assertion that fails.

Cleared means any flag but `!`. The balance is the account and its sub-accounts, in the budget currency, through the end of
the statement date. Liabilities are typed as the statement prints them, with an amount owed positive. The match is exact,
and the assertion uses the statement's own decimals. Reconciling the same statement twice writes a second, identical assertion.

## The git store

Commits are built from objects: blobs and a tree in the object database, a commit, then a compare-and-swap of the branch, so
a commit made by anyone else in between is a conflict, never a lost update. The changed files are then written to the working
tree, and the index is updated for exactly those paths.

- **Hand edits.** A file with uncommitted edits is never overwritten, and the save is refused. A file matching its index entry
  but not the branch is what a save cut off midway leaves behind, so it is not treated as an edit. The cost: a hand edit staged
  with `git add` passes too, though git keeps its contents until it cleans up.
- **Paths.** A remote's tree could name a path outside the book, so empty, `.` and `..` parts, backslashes, colons, and anything
  Windows reads as `.git` (`git~1`, or trailing dots and spaces) are refused.
- **Text** stays as committed: books set `core.autocrlf=false`, and a file that is not UTF-8 is refused by name rather than read
  loosely, since the next save would write the damage back.
- **Saves** run on the UI thread, which keeps each one atomic. Clone and sync run on a background thread.

### Sync

Fetch `origin` (pruned). Only this device moved: push. Only the remote moved: fast-forward. Both: a three-way merge, checked with
the core like any commit, then one commit with `Op-Type: sync`, pushed.

- It stops, with nothing pushed and the local commits kept, when both sides changed the same lines, when the merge would add ledger
  errors, or when the book's files have uncommitted changes. There is no automatic conflict resolution.
- Pushes are leased: a remote that moved again in between is a conflict, never overwritten.
- It runs when a book opens, after each save, and by button, one at a time.
- Scenarios, closes and archived refs travel too. Archives are a union, an archived tip goes on both sides, and changes on both sides
  are kept and named, never merged. Archived refs live under `refs/tags/archive/`, because some hosts take pushes only to branches and
  tags.
- "Replace the remote" joins both histories in one commit with this book's files rather than force pushing, which a protected branch
  refuses.

**Tokens** live in Windows Credential Manager, never in a file, so a URL holding a login is refused. The git store refuses a token for anything but HTTPS, on every clone,
fetch and push, and changing a book's address without giving a token forgets the saved one, so it never goes to a new address. A token goes only to its remote's host: a remote that redirects elsewhere gets no login sent to the other server.

### Scenarios

A scenario is a git branch, `scenarios/<name>`, in the book's repository. Merging is a three-way merge landing as one commit on the book
(Op-Type `merge-scenario`), refused if it adds ledger errors or both sides changed the same lines. T1 holds by itself, since the merge
brings dated target entries as written. Editing a past transaction in a scenario does change past months when merged, as the same edit on
the book would. Merged and set-aside scenarios move to `refs/tags/archive/scenarios/<name>-<seconds>`, so their history stays.

### Closing a month

A close is an annotated tag, `close/<YYYY-MM>`, on the book's latest version. Reopening moves it to
`refs/tags/archive/closes/<month>-<seconds>`, so the record stays.

## The Windows app

- **Books** are folders in `%APPDATA%\BranchBooks\books`, named by id, with the book's name in the repository's `description` file. Deleting
  moves a book to `.deleted`, to restore by hand.
- **More than one device.** A device with no books opens on a welcome screen that opens a book from git or starts a new one; there is
  no sample book until one is asked for. Opening from git clones the remote and keeps its token, so each device syncs the same book.
  The name is this device's (the repository's when left empty), since `description` never syncs.
- **Authors.** Commits use the book's own git identity, then the global one, then `USERNAME@COMPUTERNAME`.
- **Crashes.** A release build has no console, so panics go to `%LOCALAPPDATA%\BranchBooks\crash.log`. Ledger text can appear in a panic's
  message; the file stays on the device.
- **One exe.** `x86_64-pc-windows-msvc`, with the C runtime and WebView2's loader linked in. Release builds use thin LTO: three times faster to
  build than full LTO with one codegen unit, for 13% more exe.
- **Tests.** Logic is in plain unit tests, and what a screen shows is in HTML tests (`tests/render.rs`, `dioxus-ssr`, no browser).
- **Light and dark.** Each colour token is a `light-dark()` pair, so one list serves both. The app follows the system unless
  Settings chooses light or dark, which sets `data-theme` on the root and is kept in a `theme` file beside the books folder: it is
  this device's choice, not the book's, so it never syncs.
- **Look.** Neutral greys, with ledger green (`--brand`, `#1d6b43`) for the logo and for what can be clicked: buttons, links and the
  current page. Every other colour carries meaning, such as funded, overspent, or a positive or negative amount. Text on green
  passes WCAG AA in both themes.
- **Brand files** are in `crates/branchbooks-ui/icons`. The logo is hand-written SVG: `icon.svg` for 48 px and up, and `icon-small.svg`,
  with fewer and thicker lines, for 16 to 32 px. The wordmark's letters are IBM Plex Mono SemiBold (OFL) turned into outlines, so no
  font ships. The PNGs and `icon.ico` are rendered from the SVGs with `resvg`, and `build.rs` puts `icon.ico` in the exe. The
  splash and the social preview come in two versions: the logo outlined in white on ledger green (`splash.png`,
  `social-preview.png`), and the logo on the light background (`splash-light.png`, `social-preview-light.png`).

### Install and updates

Velopack: a per-user install with no admin prompt, delta updates, and a feed of plain files on a GitHub release.

- **Consent.** The download is automatic; the restart is the person's. The button is disabled during a sync, since restarting ends the process.
  An update not restarted into applies at the next start.
- **Failures are silent.** Offline is normal, and a failed check is tried at the next start.
- **Feed.** `HttpSource` on `releases/latest/download`, which needs a public repository: a token in an installed app would be readable by anyone.
  Velopack adds the local version, the app id and a random id it keeps to the feed's URL.
- **Trust.** HTTPS from GitHub. The feed's checksums come from the same place as the packages, and nothing is signed yet, so release rights can
  ship code to every install. That is why a release's compile job holds no write token. Code signing is the fix, and also ends the SmartScreen warning.
- **Provenance.** Every published file has a GitHub artifact attestation, signed with Sigstore by the release run's own identity, so anyone can
  check it was built by `release.yml` from a commit of this repository: `gh attestation verify <file> --repo BranchBooks/branchbooks`. Windows and
  the updater don't read it; it is for people checking a download.
- **Folders.** WebView2's data is in `%LOCALAPPDATA%\<exe name>`, also where Velopack installs, outside `current`, so updates keep it. Setup and
  uninstall can empty that folder. Books are in `%APPDATA%`, which neither touches.

## Checks and supply chain

- **One oracle per question.** Ledger behaviour: real Beancount 3.2.3 on every fixture. Budget rules: the Python models in `reference/`, whose assertions
  are in `cargo test`. A new external check is added only when it answers a question no current one answers.
- **CI** runs format, lints, docs, tests, `cargo deny` and the oracle on Linux, and lints and tests on real Windows, for pull requests and `main`. Rulesets
  require `ci-ok`, which passes when the checks passed or a change touched only documents. Weekly and by hand, it also runs the benchmark and the reference
  models.
- **Releases.** A version tag on `main` publishes. The job that compiles runs every dependency's build script, so it holds no write token and no cache; only
  `publish` can write, and it compiles nothing. Run by hand, `release.yml` is a rehearsal that publishes nothing.
- **Pinned.** Actions by commit SHA, CI's `cargo-deny` download by checksum, and the oracle's Python packages by version.
- **Licences.** `cargo deny` allows permissive licences only, so the owner can always relicense BranchBooks. Six crates in Dioxus's tree are named exceptions,
  none used directly: `cssparser`, `cssparser-macros`, `dtoa-short`, `option-ext` and `selectors` (MPL-2.0, copyleft per file, which does not reach BranchBooks'
  code) and `libfuzzer-sys` (NCSA, permissive).
- **Beancount and rustledger** are used only as separate programs or references: never linked, vendored or copied.
- **Real ledgers** stay out of the repository and CI: `private/` is git-ignored, and the oracle checks what is in it locally.

## Known limits

- **Android** is planned. grit-lib builds for Android from 0.5.29; the book's folder and token storage need Android's own. There is no Android build setup yet, and CI does not build the `mobile` feature.
- **The rest of Beancount's syntax**, custom reports, bank connections and foreign currency are not built.
- **Merged lines.** Two devices adding entries at the same place conflict, and there is no conflict editor: change one side so they no longer overlap.
- **The envelopes table** needs 423 px, more than some phones are wide.
