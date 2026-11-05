//! Sorted events and a snapshot at the start of every month, replayed from the nearest snapshot.

use super::*;

// ---------------------------------------------------------------------------------------------
// Engine: sorted events, a snapshot at the start of every month, replay from the nearest snapshot.

/// The state at the start of a month, and the index of that month's first event.
#[derive(Clone, Debug)]
struct Snapshot {
    state: BudgetState,
    index: usize,
}

#[derive(Debug)]
pub struct BudgetEngine {
    pub config: BudgetConfig,
    events: Vec<BudgetEvent>,
    /// By month. Filled lazily while reading (`state_at` takes `&self`), so behind a `Mutex`.
    snapshots: Mutex<BTreeMap<Month, Snapshot>>,
}

impl Clone for BudgetEngine {
    fn clone(&self) -> Self {
        BudgetEngine {
            config: self.config.clone(),
            events: self.events.clone(),
            snapshots: Mutex::new(lock(&self.snapshots).clone()),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl BudgetEngine {
    pub fn new(config: BudgetConfig) -> Self {
        BudgetEngine {
            config,
            events: Vec::new(),
            snapshots: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn events(&self) -> &[BudgetEvent] {
        &self.events
    }

    fn invalidate_after(&mut self, month: Month) {
        lock(&self.snapshots).retain(|k, _| *k <= month);
    }

    /// Inserts an event and invalidates only the snapshots after its month.
    pub fn insert(&mut self, e: BudgetEvent) {
        let i = self.upper_bound(&e);
        let month = e.date().month();
        self.events.insert(i, e);
        self.invalidate_after(month);
    }

    /// Bulk load: sort once, drop every snapshot.
    pub fn insert_all(&mut self, events: impl IntoIterator<Item = BudgetEvent>) {
        self.events.extend(events);
        self.events.sort_by(compare_events);
        lock(&self.snapshots).clear();
    }

    fn upper_bound(&self, e: &BudgetEvent) -> usize {
        self.events
            .partition_point(|x| compare_events(x, e) != Ordering::Greater)
    }

    /// First index whose event date is on or after the first day of `month`.
    fn month_start(&self, month: Month) -> usize {
        let first = month.first_day();
        self.events.partition_point(|x| x.base.date < first)
    }

    /// Removes the event with `e`'s id and invalidates the snapshots after its month. Returns
    /// whether it was present.
    pub fn remove(&mut self, e: &BudgetEvent) -> bool {
        let end = self.upper_bound(e);
        let Some(i) = (0..end)
            .rev()
            .find(|&i| self.events[i].base.id == e.base.id)
        else {
            return false;
        };
        self.events.remove(i);
        self.invalidate_after(e.base.date.month());
        true
    }

    /// Moves events at or below `from_line` in `file` by `delta` lines, with their flags. Sorted
    /// order is unchanged, since every seq in a file moves together.
    pub fn shift_lines(&mut self, file: &str, from_line: u32, delta: i64) {
        for e in &mut self.events {
            if &*e.base.file == file && e.base.line >= from_line {
                e.base.line = shift(e.base.line, delta);
                e.base.seq = (e.base.seq as i64 + delta) as u64;
            }
            if let EventKind::Tx(t) = &mut e.kind {
                for d in &mut t.flags {
                    if &*d.file == file && d.line >= from_line {
                        d.line = shift(d.line, delta);
                    }
                }
            }
        }
    }

    /// State at the end of `month` (all events dated in or before it).
    pub fn state_at(&self, month: Month) -> BudgetState {
        let mut snapshots = lock(&self.snapshots);
        let best = snapshots
            .range(..=month)
            .next_back()
            .map(|(_, s)| s.clone());
        let (mut state, mut i) = if let Some(snap) = best {
            (snap.state, snap.index)
        } else {
            let first_month = self.events.first().map_or(month, |e| e.date().month());
            let state = BudgetState::new(first_month.min(month));
            snapshots.insert(
                state.month,
                Snapshot {
                    state: state.clone(),
                    index: 0,
                },
            );
            (state, 0)
        };
        // Targets are funded once a month, at its end (T3, T5), before the next month opens.
        let fund = |state: &mut BudgetState| {
            if !state.targets.is_empty() {
                state.fund_targets();
            }
        };
        let mut advance_to = |state: &mut BudgetState, target: Month, i: usize| {
            while state.month < target {
                fund(state);
                state.rollover();
                snapshots.entry(state.month).or_insert_with(|| Snapshot {
                    state: state.clone(),
                    index: i,
                });
            }
        };
        while let Some(e) = self.events.get(i) {
            let m = e.date().month();
            if m > month {
                break;
            }
            advance_to(&mut state, m, i);
            state.apply(e);
            i += 1;
        }
        advance_to(&mut state, month, i);
        fund(&mut state);
        state
    }

    /// Every envelope entry the budget skipped as blocked, in `from` (default: the first month)
    /// and later.
    pub fn blocked(&self, from: Option<Month>) -> Vec<Diagnostic> {
        let (Some(first), Some(last)) = (self.events.first(), self.events.last()) else {
            return Vec::new();
        };
        let (first, last) = (first.date().month(), last.date().month());
        let mut m = from.map_or(first, |f| f.max(first));
        let mut out = Vec::new();
        while m <= last {
            out.extend(self.state_at(m).blocked);
            m = m.next();
        }
        out
    }

    /// Money assigned in months after `month` (rule 10).
    pub fn assigned_after(&self, month: Month) -> Decimal {
        self.events[self.month_start(month.next())..]
            .iter()
            .map(|e| match e.kind {
                EventKind::Assign { amount, .. } => amount,
                _ => ZERO,
            })
            .sum()
    }

    /// Unassigned as of `today`'s month, and what is assigned to later months: future means
    /// months after today, not after a viewed month.
    pub fn unassigned(&self, today: Date) -> (Decimal, Decimal) {
        let current = today.month();
        let future = self.assigned_after(current);
        (self.state_at(current).unassigned(future), future)
    }

    /// The budget for `month`. Rows show that month; Unassigned is one "now" figure computed
    /// for the month containing `today` (decision 6 in docs/DESIGN.md), the same on every month view.
    /// The engine has no clock: callers pass today. It defaults to the viewed month.
    pub fn view(&self, month: Month, today: Option<Date>) -> MonthView {
        let today = today.unwrap_or_else(|| month.first_day());
        let state = self.state_at(month);
        let (unassigned, assigned_to_future) = self.unassigned(today);
        let pay = state.pay();
        let names: IndexSet<&String> = self
            .config
            .categories
            .iter()
            .chain(state.cats.keys())
            .collect();
        let empty = CategoryState::default();
        let categories = sorted(names.into_iter())
            .into_iter()
            .map(|name| {
                let c = state.cats.get(name).unwrap_or(&empty);
                let s = split_of(c);
                CategoryRow {
                    category: name.clone(),
                    carried_in: c.carried_in,
                    assigned: c.assigned,
                    target_allocated: c.target_allocated,
                    moved: c.moved,
                    activity: -(c.cash + c.card_total()),
                    available: c.available(),
                    cash_overspent: s.cash,
                    credit_overspent: s.credit,
                    target: state.targets.get(name.as_str()).cloned(),
                    shortfall: c.shortfall,
                }
            })
            .collect();
        let card_names: IndexSet<&String> = self.config.cards.iter().chain(pay.keys()).collect();
        let cards = sorted(card_names.into_iter())
            .into_iter()
            .map(|card| {
                let available = pay.get(card).copied().unwrap_or(ZERO);
                let owed = state.owed.get(card).copied().unwrap_or(ZERO);
                CardRow {
                    card: card.clone(),
                    available,
                    owed,
                    underfunded: (owed - available).max(ZERO),
                }
            })
            .collect();
        let mut flags = Vec::new();
        for e in &self.events[self.month_start(month)..] {
            if e.date().month() != month {
                break;
            }
            if let EventKind::Tx(t) = &e.kind {
                flags.extend(t.flags.iter().cloned());
            }
        }
        flags.extend(state.blocked.iter().cloned());
        MonthView {
            month,
            currency: self.config.currency.clone(),
            today,
            unassigned,
            assigned_to_future,
            cash: state.cash,
            categories,
            cards,
            warnings: state.warnings,
            flags,
            shortfalls: state.shortfalls,
        }
    }
}

/// `line + delta`, for line shifts. Lines never go below zero.
pub(crate) fn shift(line: u32, delta: i64) -> u32 {
    (i64::from(line) + delta).max(0) as u32
}
