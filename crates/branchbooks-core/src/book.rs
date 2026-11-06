//! Book ties the ledger and the budget together. `Book::from_load` is the full recompute. The write
//! path keeps a Book current incrementally instead: `add_entry`, `remove_entry` and `shift_lines`
//! (when an edit moves the lines below it). Everything a Book derives is kept per entry, so
//! removing an entry removes exactly what it contributed. Both paths must agree; the
//! write-incremental tests check it.
//!
//! Entries live in an arena and are named by [`EntryId`]. Reads that may need a rebuild (after an `open` or `close` arrives) take `&mut self`.

use std::cmp::Ordering;

use crate::budget::{
    BudgetConfig, BudgetEngine, BudgetEvent, MonthView, classify_accounts, custom_event, shift,
    tx_event,
};
use crate::date::{Date, Month};
use crate::ledger::{
    Accounts, Balances, add_postings, book_transaction, check_account_refs, check_balances,
    collect_accounts, compare_entries, sort_diagnostics,
};
use crate::loader::{FileSource, LoadResult, Options, load};
use crate::money::Decimal;
use crate::types::{Diagnostic, Entry, EntryType};

/// Names an entry in a Book. Ids are never reused.
pub type EntryId = u64;

/// The id a budget event gets when it comes from [`Book::effect_of`] for an entry that is not in
/// the Book, so that [`Book::blocked_with`] can insert it and take it out again.
pub const PROBE_ID: EntryId = u64::MAX;

/// What one entry contributes, computed without changing the Book (used to validate before
/// writing).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EntryEffect {
    pub errors: Vec<Diagnostic>,
    pub booked: Option<Entry>,
    pub event: Option<BudgetEvent>,
    /// Budget diagnostics, such as an envelope entry naming something that is not a category.
    pub flags: Vec<Diagnostic>,
}

/// What [`Book::reconcile`] finds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reconciliation<'b> {
    pub cleared: Decimal,
    pub pending: Vec<(&'b Entry, Decimal)>,
}

#[derive(Clone, Debug)]
struct Slot {
    entry: Entry,
    effect: Option<EntryEffect>,
}

#[derive(Clone, Debug)]
pub struct Book {
    options: Options,
    load_errors: Vec<Diagnostic>,
    /// Indexed by id; `None` once removed.
    slots: Vec<Option<Slot>>,
    /// Ids in the order added.
    list: Vec<EntryId>,
    accounts: Accounts,
    account_errors: Vec<Diagnostic>,
    totals: Balances,
    config: BudgetConfig,
    engine: BudgetEngine,
    sorted_txns: Option<Vec<EntryId>>,
    /// Set when an open or close arrives incrementally: classification may change, so rebuild.
    dirty: bool,
}

impl Book {
    pub fn load(path: &str, source: &dyn FileSource) -> Book {
        Book::from_load(load(path, source, None))
    }

    pub fn from_load(loaded: LoadResult) -> Book {
        let mut book = Book::new(loaded.options, loaded.errors);
        for e in loaded.entries {
            book.push(e);
        }
        book.rebuild();
        book
    }

    /// An empty book for incremental use.
    pub fn empty(options: Options) -> Book {
        let mut book = Book::new(options, Vec::new());
        book.rebuild();
        book
    }

    fn new(options: Options, load_errors: Vec<Diagnostic>) -> Book {
        Book {
            options,
            load_errors,
            slots: Vec::new(),
            list: Vec::new(),
            accounts: Accounts::new(),
            account_errors: Vec::new(),
            totals: Balances::new(),
            config: BudgetConfig::default(),
            engine: BudgetEngine::new(BudgetConfig::default()),
            sorted_txns: None,
            dirty: false,
        }
    }

    fn push(&mut self, entry: Entry) -> EntryId {
        let id = self.slots.len() as EntryId;
        self.slots.push(Some(Slot {
            entry,
            effect: None,
        }));
        self.list.push(id);
        id
    }

    fn slot(&self, id: EntryId) -> Option<&Slot> {
        self.slots.get(id as usize).and_then(Option::as_ref)
    }

    fn rebuild(&mut self) {
        let mut sorted: Vec<EntryId> = self.list.clone();
        sorted.sort_by(|a, b| compare_entries(self.entry_unchecked(*a), self.entry_unchecked(*b)));
        let mut account_errors = Vec::new();
        self.accounts = collect_accounts(
            sorted.iter().map(|id| self.entry_unchecked(*id)),
            &mut account_errors,
        );
        self.account_errors = account_errors;
        self.config = classify_accounts(&self.accounts, &self.options);
        self.engine = BudgetEngine::new(self.config.clone());
        self.totals = Balances::new();
        self.sorted_txns = None;
        let mut events = Vec::new();
        for id in sorted {
            let slot = self.slots[id as usize]
                .as_mut()
                .expect("listed entries exist");
            slot.effect = None;
            if !matches!(
                slot.entry.entry_type(),
                EntryType::Txn | EntryType::Custom | EntryType::Balance
            ) {
                continue;
            }
            let fx = effect_with(&slot.entry, id, &self.accounts, &self.config);
            if let Some(ev) = &fx.event {
                events.push(ev.clone());
            }
            self.record(id, fx);
        }
        self.engine.insert_all(events);
        self.dirty = false;
    }

    fn entry_unchecked(&self, id: EntryId) -> &Entry {
        &self.slot(id).expect("listed entries exist").entry
    }

    fn fresh(&mut self) {
        if self.dirty {
            self.rebuild();
        }
    }

    /// The errors, booking and budget event an entry would have in this Book. Changes nothing.
    /// Its event has [`PROBE_ID`] as id.
    pub fn effect_of(&mut self, e: &Entry) -> EntryEffect {
        self.fresh();
        effect_with(e, PROBE_ID, &self.accounts, &self.config)
    }

    fn record(&mut self, id: EntryId, fx: EntryEffect) {
        if let Some(t) = fx.booked.as_ref().and_then(Entry::txn) {
            add_postings(&mut self.totals, &t.postings, false);
        }
        self.slots[id as usize]
            .as_mut()
            .expect("recorded entries exist")
            .effect = Some(fx);
        self.sorted_txns = None;
    }

    /// Adds an entry. Transactions and custom entries update balances and the budget in place.
    pub fn add_entry(&mut self, e: Entry) -> EntryId {
        let t = e.entry_type();
        let id = self.push(e);
        if matches!(t, EntryType::Open | EntryType::Close) {
            self.dirty = true;
            return id;
        }
        if self.dirty || !matches!(t, EntryType::Txn | EntryType::Custom | EntryType::Balance) {
            return id;
        }
        let fx = effect_with(self.entry_unchecked(id), id, &self.accounts, &self.config);
        if let Some(ev) = &fx.event {
            self.engine.insert(ev.clone());
        }
        self.record(id, fx);
        id
    }

    /// Removes an entry and everything it contributed, and returns it. `None` when it is not in
    /// the book.
    pub fn remove_entry(&mut self, id: EntryId) -> Option<Entry> {
        let slot = self.slots.get_mut(id as usize).and_then(Option::take)?;
        self.list.retain(|x| *x != id);
        if matches!(slot.entry.entry_type(), EntryType::Open | EntryType::Close) {
            self.dirty = true;
            return Some(slot.entry);
        }
        if self.dirty {
            return Some(slot.entry);
        }
        if let Some(fx) = slot.effect {
            if let Some(t) = fx.booked.as_ref().and_then(Entry::txn) {
                add_postings(&mut self.totals, &t.postings, true);
            }
            if let Some(ev) = &fx.event {
                let removed = self.engine.remove(ev);
                debug_assert!(removed, "an entry's event is in the engine");
            }
            self.sorted_txns = None;
        }
        Some(slot.entry)
    }

    /// Moves everything at or below `from_line` in `file` by `delta` lines: entries, their
    /// postings, budget events and every diagnostic. Called when an edit inserts or removes lines
    /// above them.
    pub fn shift_lines(&mut self, file: &str, from_line: u32, delta: i64) {
        if delta == 0 {
            return;
        }
        let shift_diag = |d: &mut Diagnostic| {
            if &*d.file == file && d.line >= from_line {
                d.line = shift(d.line, delta);
            }
        };
        let shift_entry = |e: &mut Entry| {
            if &*e.file != file || e.line < from_line {
                return;
            }
            e.line = shift(e.line, delta);
            e.end_line = shift(e.end_line, delta);
            e.seq = (e.seq as i64 + delta) as u64;
            if let Some(t) = e.txn_mut() {
                for p in t.postings.iter_mut().filter(|p| p.line >= from_line) {
                    p.line = shift(p.line, delta);
                }
            }
        };
        for slot in self.slots.iter_mut().flatten() {
            shift_entry(&mut slot.entry);
            let Some(fx) = &mut slot.effect else { continue };
            if let Some(b) = &mut fx.booked {
                shift_entry(b);
            }
            if let Some(ev) = &mut fx.event {
                if &*ev.base.file == file && ev.base.line >= from_line {
                    ev.base.line = shift(ev.base.line, delta);
                    ev.base.seq = (ev.base.seq as i64 + delta) as u64;
                }
                if let crate::budget::EventKind::Tx(t) = &mut ev.kind {
                    t.flags.iter_mut().for_each(shift_diag);
                }
            }
            fx.errors.iter_mut().for_each(shift_diag);
            fx.flags.iter_mut().for_each(shift_diag);
        }
        self.engine.shift_lines(file, from_line, delta);
        self.load_errors.iter_mut().for_each(shift_diag);
        self.account_errors.iter_mut().for_each(shift_diag);
        self.config.diagnostics.iter_mut().for_each(shift_diag);
        self.engine
            .config
            .diagnostics
            .iter_mut()
            .for_each(shift_diag);
    }

    /// Every entry, in the order added.
    pub fn entries(&self) -> impl Iterator<Item = (EntryId, &Entry)> {
        self.list.iter().map(|id| (*id, self.entry_unchecked(*id)))
    }

    pub fn entry(&self, id: EntryId) -> Option<&Entry> {
        self.slot(id).map(|s| &s.entry)
    }

    /// The format the book says it is (see [`mod@crate::format`]).
    pub fn format(&self) -> crate::format::Stamp {
        crate::format::recorded(self.entries().map(|(_, e)| e))
    }

    /// Why this book is read-only here: a newer app saved it in a format this build doesn't read.
    pub fn read_only(&self) -> Option<String> {
        crate::format::too_new(&self.format())
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn options(&self) -> &Options {
        &self.options
    }

    fn effects(&self) -> impl Iterator<Item = &EntryEffect> {
        self.slots
            .iter()
            .flatten()
            .filter_map(|s| s.effect.as_ref())
    }

    /// Ledger errors (what Beancount would report).
    pub fn errors(&mut self) -> Vec<Diagnostic> {
        self.fresh();
        let mut all = self.load_errors.clone();
        all.extend(self.account_errors.iter().cloned());
        all.extend(self.effects().flat_map(|fx| fx.errors.iter().cloned()));
        all.extend(self.balance_failures(None, None));
        sort_diagnostics(all)
    }

    /// Ledger errors attributed to one entry.
    pub fn errors_of(&mut self, id: EntryId) -> Vec<Diagnostic> {
        self.fresh();
        self.slot(id)
            .and_then(|s| s.effect.as_ref())
            .map_or_else(Vec::new, |fx| fx.errors.clone())
    }

    /// Failed balance assertions, as if `skip` were not in the Book and `extra` (a booked
    /// transaction or an assertion) were. One pass over the transactions; none without assertions.
    /// shortcut: re-sorts the transactions after every change, about 60 ms a write at 100,000
    /// transactions with assertions; keep running balances per assertion if writes feel slow.
    pub fn balance_failures<'s>(
        &'s mut self,
        skip: Option<EntryId>,
        extra: Option<&'s Entry>,
    ) -> Vec<Diagnostic> {
        let is_assertion = |e: &Entry| e.balance().is_some();
        let listed = |id: &EntryId| Some(*id) != skip;
        if !extra.is_some_and(is_assertion)
            && !self
                .list
                .iter()
                .filter(|id| listed(id))
                .any(|id| is_assertion(self.entry_unchecked(*id)))
        {
            return Vec::new();
        }
        let _ = self.transactions(); // fills the sorted cache
        let mut assertions: Vec<&Entry> = self
            .list
            .iter()
            .filter(|id| listed(id))
            .map(|id| self.entry_unchecked(*id))
            .filter(|e| is_assertion(e))
            .collect();
        assertions.extend(extra.filter(|e| is_assertion(e)));
        let mut txns: Vec<&Entry> = self
            .sorted_txns
            .as_ref()
            .map(|ids| {
                ids.iter()
                    .filter(|id| listed(id))
                    .filter_map(|id| self.slot(*id)?.effect.as_ref()?.booked.as_ref())
                    .collect()
            })
            .unwrap_or_default();
        if let Some(t) = extra.filter(|e| e.txn().is_some()) {
            let at = txns.partition_point(|x| compare_entries(x, t) != Ordering::Greater);
            txns.insert(at, t);
        }
        check_balances(&txns, &assertions, &self.accounts)
    }

    /// Budget-level diagnostics: flag syntax and invalid envelope entries.
    pub fn budget_diagnostics(&mut self) -> Vec<Diagnostic> {
        self.fresh();
        let mut all = self.config.diagnostics.clone();
        all.extend(self.effects().flat_map(|fx| fx.flags.iter().cloned()));
        sort_diagnostics(all)
    }

    pub fn currency(&mut self) -> &str {
        self.fresh();
        &self.config.currency
    }

    pub fn account_info(&mut self) -> &Accounts {
        self.fresh();
        &self.accounts
    }

    /// Whether `account` only groups its sub-accounts.
    pub fn is_placeholder(&mut self, account: &str) -> bool {
        self.fresh();
        self.config.is_placeholder(account)
    }

    /// Booked transactions in date order.
    pub fn transactions(&mut self) -> Vec<&Entry> {
        self.fresh();
        if self.sorted_txns.is_none() {
            let mut ids: Vec<(EntryId, &Entry)> = self
                .slots
                .iter()
                .enumerate()
                .filter_map(|(i, s)| {
                    Some((i as EntryId, s.as_ref()?.effect.as_ref()?.booked.as_ref()?))
                })
                .collect();
            ids.sort_by(|a, b| compare_entries(a.1, b.1));
            self.sorted_txns = Some(ids.into_iter().map(|(id, _)| id).collect());
        }
        self.sorted_txns
            .as_ref()
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.slot(*id)?.effect.as_ref()?.booked.as_ref())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Reconcile (R1, R2): the cleared balance of `account` and its sub-accounts in `currency` at
    /// the end of `date`, as a `balance` dated the day after counts it, and the pending (`!`)
    /// transactions up to then with their amount there. Any other flag counts as cleared.
    pub fn reconcile(&mut self, account: &str, date: Date, currency: &str) -> Reconciliation<'_> {
        let under = format!("{account}:");
        let mut out = Reconciliation {
            cleared: Decimal::ZERO,
            pending: Vec::new(),
        };
        for e in self.transactions() {
            if e.date > date {
                break;
            }
            let Some(t) = e.txn() else { continue };
            let mut here = t
                .postings
                .iter()
                .filter(|p| p.account == account || p.account.starts_with(&under))
                .filter_map(|p| p.units.as_ref().filter(|u| u.currency == currency))
                .peekable();
            if here.peek().is_none() {
                continue;
            }
            let amount: Decimal = here.map(|u| u.number).sum();
            if t.flag == "!" {
                out.pending.push((e, amount));
            } else {
                out.cleared += amount;
            }
        }
        out
    }

    /// Balances over all transactions.
    pub fn balances(&mut self) -> &Balances {
        self.fresh();
        &self.totals
    }

    /// Budget rows for `month`; Unassigned as of `today` (defaults to the viewed month).
    pub fn budget(&mut self, month: Month, today: Option<Date>) -> MonthView {
        self.fresh();
        self.engine.view(month, today)
    }

    /// Envelope entries skipped because they would overspend a category (decision 2 in docs/DESIGN.md).
    pub fn blocked(&mut self, from_date: Option<Date>) -> Vec<Diagnostic> {
        self.fresh();
        self.engine.blocked(from_date.map(Date::month))
    }

    /// Tries a budget change on the live engine and undoes it: returns the blocked entries from
    /// `from_date`'s month on, as they would be with `remove` taken out and `insert` added.
    pub fn blocked_with(
        &mut self,
        from_date: Date,
        remove: Option<&BudgetEvent>,
        insert: Option<&BudgetEvent>,
    ) -> Vec<Diagnostic> {
        self.fresh();
        if let Some(r) = remove {
            let removed = self.engine.remove(r);
            debug_assert!(removed, "the event being tried without is in the engine");
        }
        if let Some(i) = insert {
            self.engine.insert(i.clone());
        }
        let out = self.engine.blocked(Some(from_date.month()));
        if let Some(i) = insert {
            let removed = self.engine.remove(i);
            debug_assert!(removed, "the event just inserted is in the engine");
        }
        if let Some(r) = remove {
            self.engine.insert(r.clone());
        }
        out
    }

    /// The budget event an entry contributed, if any.
    pub fn event_of(&mut self, id: EntryId) -> Option<&BudgetEvent> {
        self.fresh();
        self.slot(id)?.effect.as_ref()?.event.as_ref()
    }
}

fn effect_with(e: &Entry, id: EntryId, accounts: &Accounts, config: &BudgetConfig) -> EntryEffect {
    match e.entry_type() {
        EntryType::Txn => {
            let b = book_transaction(e);
            let mut errors = b.errors;
            if let Some(t) = &b.txn {
                errors.extend(check_account_refs(t, accounts));
            }
            let event = b.txn.as_ref().and_then(|t| tx_event(t, config, id));
            EntryEffect {
                errors,
                booked: b.txn,
                event,
                flags: Vec::new(),
            }
        }
        EntryType::Custom => {
            let (event, flags) = custom_event(e, config, id);
            EntryEffect {
                errors: check_account_refs(e, accounts),
                booked: None,
                event,
                flags,
            }
        }
        // Whether it holds is checked across the ledger, in `balance_failures`.
        EntryType::Balance => EntryEffect {
            errors: check_account_refs(e, accounts),
            ..EntryEffect::default()
        },
        _ => EntryEffect::default(),
    }
}
