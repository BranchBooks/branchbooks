//! The budget's state for one month, and how events change it. The model is `reference/sim4.py`.

use super::*;

// ---------------------------------------------------------------------------------------------
// Core state, as in reference/sim4.py

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CategoryState {
    /// A0: opening Available, negative only for the cash overspend a category that rolls over
    /// carries in (rule 6a).
    pub carried_in: Decimal,
    pub assigned: Decimal,
    pub moved: Decimal,
    /// c: cash spending this month, net of cash refunds.
    pub cash: Decimal,
    /// k per card: card spending this month, net of card refunds.
    pub card: IndexMap<String, Decimal>,
    /// Card overspending carried in by a category with rollover overspending (rule 6a), per card.
    /// A claim on the card like this month's spending, but not part of Activity.
    pub carried_card: IndexMap<String, Decimal>,
    /// Cash overspend corrected this month.
    pub cured: Decimal,
    /// Date of the last spending in this category this month.
    pub last_spend: Option<Date>,
    /// Allocated this month by the category's target (T3, T5): virtual, never in the ledger.
    pub target_allocated: Decimal,
    /// What the target needed this month and Unassigned could not fund (T5).
    pub shortfall: Decimal,
}

impl CategoryState {
    pub fn card_total(&self) -> Decimal {
        self.card.values().sum()
    }

    /// What the category owes each card: this month's spending plus carried card overspending.
    fn card_claims(&self) -> IndexMap<&str, Decimal> {
        let mut out: IndexMap<&str, Decimal> = IndexMap::new();
        for (card, v) in self.card.iter().chain(&self.carried_card) {
            *out.entry(card.as_str()).or_insert(ZERO) += *v;
        }
        out
    }

    pub fn available(&self) -> Decimal {
        self.carried_in + self.assigned + self.target_allocated + self.moved
            - self.cash
            - self.card_total()
            - self.carried_card.values().sum::<Decimal>()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Split {
    pub cash: Decimal,
    pub credit: Decimal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BudgetState {
    pub month: Month,
    pub cash: Decimal,
    pub owed: IndexMap<String, Decimal>,
    pub pay_carried: IndexMap<String, Decimal>,
    pub payments: IndexMap<String, Decimal>,
    /// Assigned straight to a card's payment envelope this month (decision 7).
    pub pay_assigned: IndexMap<String, Decimal>,
    pub cats: IndexMap<String, CategoryState>,
    pub warnings: Vec<Diagnostic>,
    /// Envelope entries skipped this month because they would overspend a category.
    pub blocked: Vec<Diagnostic>,
    /// The target in force per category (T1). Carries from month to month.
    pub targets: IndexMap<String, Target>,
    /// Categories with rollover overspending on (rule 6a). Carries from month to month.
    pub rollover_overspending: IndexSet<String>,
    /// Monthly `carry-shortfall` targets: what earlier months failed to fund.
    pub target_debt: IndexMap<String, Decimal>,
    /// Targets this month could not fully fund.
    pub shortfalls: Vec<Shortfall>,
    /// Whether this month's targets are funded; done once, at the end of the month.
    pub funded: bool,
}

/// A target that a month could not fully fund: the data behind shortfall alerts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shortfall {
    pub category: String,
    pub month: Month,
    pub amount: Decimal,
    pub message: String,
}

fn add_to(map: &mut IndexMap<String, Decimal>, key: &str, v: Decimal) {
    match map.get_mut(key) {
        Some(slot) => *slot += v,
        None => {
            map.insert(key.to_owned(), v);
        }
    }
}

pub(super) fn sorted<'a>(keys: impl Iterator<Item = &'a String>) -> Vec<&'a String> {
    let mut v: Vec<&String> = keys.collect();
    v.sort();
    v
}

impl BudgetState {
    pub fn new(month: Month) -> Self {
        BudgetState {
            month,
            cash: ZERO,
            owed: IndexMap::new(),
            pay_carried: IndexMap::new(),
            payments: IndexMap::new(),
            pay_assigned: IndexMap::new(),
            cats: IndexMap::new(),
            warnings: Vec::new(),
            blocked: Vec::new(),
            targets: IndexMap::new(),
            rollover_overspending: IndexSet::new(),
            target_debt: IndexMap::new(),
            shortfalls: Vec::new(),
            funded: false,
        }
    }

    pub fn cat(&mut self, name: &str) -> &mut CategoryState {
        if !self.cats.contains_key(name) {
            self.cats.insert(name.to_owned(), CategoryState::default());
        }
        self.cats.get_mut(name).expect("just inserted")
    }

    pub fn avail(&self, name: &str) -> Decimal {
        self.cats.get(name).map_or(ZERO, CategoryState::available)
    }

    /// Splits an overspend into cash and credit portions by month totals (rule 5). The credit
    /// portion is capped by the month's card spending (decision 2 in docs/DESIGN.md).
    pub fn split(&self, name: &str) -> Split {
        self.cats.get(name).map_or_else(Split::default, split_of)
    }

    /// Payment envelope Available per card (sim4 `pay`).
    pub fn pay(&self) -> IndexMap<String, Decimal> {
        let mut out = IndexMap::new();
        for (card, v) in &self.pay_carried {
            add_to(&mut out, card, *v);
        }
        for (card, v) in &self.pay_assigned {
            add_to(&mut out, card, *v);
        }
        for (card, v) in &self.payments {
            add_to(&mut out, card, -*v);
        }
        for c in self.cats.values() {
            let by_card = credit_by_card(c, split_of(c).credit);
            for (card, k) in c.card_claims() {
                add_to(
                    &mut out,
                    card,
                    k - by_card.get(card).copied().unwrap_or(ZERO),
                );
            }
        }
        for card in self.owed.keys() {
            out.entry(card.clone()).or_insert(ZERO);
        }
        out
    }

    /// Unassigned (rule 2). `future` is money assigned to months after this one.
    pub fn unassigned(&self, future: Decimal) -> Decimal {
        let env: Decimal = self
            .cats
            .values()
            .map(|c| c.available() + split_of(c).credit)
            .sum();
        let pay: Decimal = self.pay().values().sum();
        self.cash - env - pay - future
    }

    // operations -------------------------------------------------------------------------------

    fn correct(&mut self, name: &str, x: Decimal, ev: Option<&EventBase>, what: &str) {
        self.cat(name);
        if x <= ZERO {
            return;
        }
        let s = self.split(name);
        if (s.cash + s.credit).is_zero() {
            return;
        }
        let c = self.cat(name);
        c.cured += x.min(s.cash); // a correction covers the cash portion first
        let last_spend = c.last_spend;
        // Rule 15: warn only when dated after the last spending this month.
        if let (Some(ev), Some(last)) = (ev, last_spend)
            && ev.date > last
        {
            let message = format!(
                "{} {what} into {name} corrects an overspend from spending on {last}",
                ev.date
            );
            self.warnings.push(Diagnostic::new(
                &ev.file,
                ev.line,
                "late-correction",
                message,
            ));
        }
    }

    fn blocked_at(&mut self, ev: Option<&EventBase>, message: String) {
        let date = ev.map_or_else(|| self.month.to_string(), |e| e.date.to_string());
        let message = format!("{date} {message}");
        let file: Arc<str> = ev.map_or_else(|| Arc::from(""), |e| e.file.clone());
        self.blocked.push(Diagnostic::new(
            &file,
            ev.map_or(0, |e| e.line),
            "blocked",
            message,
        ));
    }

    /// Moves and negative assignments may not overspend a category: one that would leave the
    /// category it takes from below zero is skipped and reported, and changes nothing.
    fn blocks(
        &mut self,
        name: &str,
        decrease: Decimal,
        ev: Option<&EventBase>,
        what: &str,
    ) -> bool {
        if decrease <= ZERO {
            return false;
        }
        let after = self.avail(name) - decrease;
        if after >= ZERO {
            return false;
        }
        self.blocked_at(
            ev,
            format!(
                "{what} skipped: it would leave {name} at {}",
                fixed(after, 2)
            ),
        );
        true
    }

    pub fn assign(&mut self, name: &str, x: Decimal, ev: Option<&EventBase>) {
        if self.blocks(name, -x, ev, "envelope-assign") {
            return;
        }
        self.correct(name, x, ev, "envelope-assign");
        self.cat(name).assigned += x;
    }

    /// Sets money aside in a card's payment envelope (decision 7), which Unassigned pays for.
    /// A negative assignment takes it back, but never below zero, like a negative category
    /// assignment.
    pub fn assign_card(&mut self, card: &str, x: Decimal, ev: Option<&EventBase>) {
        if x < ZERO {
            let after = self.pay().get(card).copied().unwrap_or(ZERO) + x;
            if after < ZERO {
                let message = format!(
                    "envelope-assign skipped: it would leave {card}'s payment envelope at {}",
                    fixed(after, 2)
                );
                self.blocked_at(ev, message);
                return;
            }
        }
        add_to(&mut self.pay_assigned, card, x);
    }

    pub fn move_(&mut self, from: &str, to: &str, x: Decimal, ev: Option<&EventBase>) {
        if x < ZERO {
            return self.move_(to, from, -x, ev);
        }
        if self.blocks(from, x, ev, "envelope-move") {
            return;
        }
        self.cat(from).moved -= x;
        self.correct(to, x, ev, "envelope-move");
        self.cat(to).moved += x;
    }

    pub fn apply_tx(&mut self, e: &TxEffect, date: Date) {
        self.cash += e.cash;
        for (card, d) in &e.card_delta {
            add_to(&mut self.owed, card, -*d);
        }
        for s in &e.spends {
            let c = self.cat(&s.category);
            match &s.card {
                None => c.cash += s.amount,
                Some(card) => add_to(&mut c.card, card, s.amount),
            }
            if s.amount > ZERO {
                c.last_spend = Some(date);
            }
        }
        for (card, v) in &e.payments {
            add_to(&mut self.payments, card, *v);
        }
    }

    pub fn apply(&mut self, e: &BudgetEvent) {
        let ev = Some(&e.base);
        match &e.kind {
            EventKind::Tx(t) => self.apply_tx(t, e.base.date),
            EventKind::Assign {
                category,
                amount,
                card: true,
            } => self.assign_card(category, *amount, ev),
            EventKind::Assign {
                category,
                amount,
                card: false,
            } => self.assign(category, *amount, ev),
            EventKind::Move { from, to, amount } => self.move_(from, to, *amount, ev),
            EventKind::Target { category, target } => self.set_target(category, target),
            EventKind::Overspending { category, rollover } => {
                if *rollover {
                    self.rollover_overspending.insert(category.clone());
                } else {
                    self.rollover_overspending.shift_remove(category);
                }
            }
        }
    }

    /// Puts a target in force for its category (T1); an amount of zero ends the target.
    pub fn set_target(&mut self, category: &str, target: &Target) {
        self.target_debt.shift_remove(category);
        if target.amount.is_zero() {
            self.targets.shift_remove(category);
        } else {
            self.targets.insert(category.to_owned(), target.clone());
        }
    }

    /// What a category's target asks for this month, before funding (T3). "Put in" is the money
    /// the category has been given, before this month's spending: carried in, assigned, moved.
    pub fn target_need(&self, category: &str, t: &Target) -> Decimal {
        let empty = CategoryState::default();
        let c = self.cats.get(category).unwrap_or(&empty);
        let put_in = c.carried_in + c.assigned + c.moved;
        match (t.recurrence, t.rollover) {
            // Refill up to the amount.
            (Recurrence::Monthly, Rollover::Reset) => (t.amount - put_in).max(ZERO),
            // Set the amount aside every month, plus what earlier months failed to fund.
            (Recurrence::Monthly, Rollover::CarryShortfall) => {
                let debt = self.target_debt.get(category).copied().unwrap_or(ZERO);
                (t.amount + debt - c.assigned - c.moved).max(ZERO)
            }
            // Spread what is left at the month's start over the months to the due month; the last
            // one takes the rest. This month's assignments and moves count toward the share.
            (Recurrence::Once, _) => {
                let left = self.month.months_until(t.due_month(self.month));
                let remaining = t.amount - c.carried_in;
                if left <= 0 || remaining <= ZERO {
                    return ZERO;
                }
                let share = if left == 1 {
                    remaining
                } else {
                    (remaining / Decimal::from(left))
                        .round_dp_with_strategy(t.scale, RoundingStrategy::ToZero)
                };
                (share - c.assigned - c.moved).max(ZERO)
            }
        }
    }

    /// Funds this month's targets from the money on hand this month (T5): after the month's
    /// explicit assignments, by due month and then category name. What it cannot cover is the
    /// target's shortfall. Assignments to later months are not counted: funding a month depends
    /// only on that month and earlier ones, which the month snapshots rely on. Runs once per
    /// month.
    pub fn fund_targets(&mut self) {
        if self.funded {
            return;
        }
        self.funded = true;
        let month = self.month;
        let mut order: Vec<(String, Target)> = self
            .targets
            .iter()
            .map(|(c, t)| (c.clone(), t.clone()))
            .collect();
        order.sort_by(|(a, ta), (b, tb)| {
            ta.due_month(month)
                .cmp(&tb.due_month(month))
                .then_with(|| a.cmp(b))
        });
        for (category, t) in order {
            let need = self.target_need(&category, &t);
            let funded = need.min(self.unassigned(ZERO).max(ZERO));
            if funded > ZERO {
                // Like an assignment, it covers a cash overspend first (rule 5).
                let s = self.split(&category);
                let c = self.cat(&category);
                c.cured += funded.min(s.cash);
                c.target_allocated += funded;
            }
            let short = need - funded;
            self.cat(&category).shortfall = short;
            if t.rollover == Rollover::CarryShortfall
                && t.recurrence == Recurrence::Monthly
                && short > ZERO
            {
                self.target_debt.insert(category.clone(), short);
            } else {
                self.target_debt.shift_remove(&category);
            }
            if short > ZERO {
                let amount = fixed(short, 2);
                let message = match &t.due {
                    Some(due) if due.month() == month => {
                        format!("{category} misses its {} target by {amount}", due.month())
                    }
                    Some(due) => format!(
                        "{category} is {amount} short of its share for the {} target",
                        due.month()
                    ),
                    None => format!("{category} is {amount} short of its monthly target"),
                };
                self.shortfalls.push(Shortfall {
                    category,
                    month,
                    amount: short,
                    message,
                });
            }
        }
    }

    /// Month end (rules 6, 6a, 12). Positive Available carries. Overspending resets the category
    /// to zero and its credit portion moves into the payment envelopes, so the next month's
    /// Unassigned pays for all of it. With rollover overspending both portions carry instead.
    pub fn rollover(&mut self) {
        let mut pay_now = self.pay();
        for (name, c) in &mut self.cats {
            let av = c.available();
            let mut next = CategoryState::default();
            if av >= ZERO {
                next.carried_in = av;
            } else {
                let split = split_of(c);
                let by_card: Vec<(String, Decimal)> = credit_by_card(c, split.credit)
                    .into_iter()
                    .filter(|(_, v)| *v > ZERO)
                    .map(|(card, v)| (card.to_owned(), v))
                    .collect();
                if self.rollover_overspending.contains(name) {
                    next.carried_in = -split.cash;
                    next.carried_card = by_card.into_iter().collect();
                } else {
                    for (card, v) in by_card {
                        add_to(&mut pay_now, &card, v);
                    }
                }
            }
            *c = next;
        }
        self.pay_carried = pay_now;
        self.payments.clear();
        self.pay_assigned.clear();
        self.warnings.clear();
        self.blocked.clear();
        self.shortfalls.clear();
        self.funded = false;
        self.month = self.month.next();
    }
}

pub(super) fn split_of(c: &CategoryState) -> Split {
    let over = (-c.available()).max(ZERO);
    if over.is_zero() {
        return Split::default();
    }
    let claims = ((-c.carried_in).max(ZERO) + c.cash - c.cured).max(ZERO);
    let cash_first = over.min(claims);
    let card_spend: Decimal = c.card_claims().values().filter(|v| **v > ZERO).sum();
    let credit = (over - cash_first).min(card_spend);
    Split {
        cash: over - credit,
        credit,
    }
}

/// Credit portion of a category charged to each card, in card name order (decision 3).
pub(super) fn credit_by_card(c: &CategoryState, credit: Decimal) -> IndexMap<&str, Decimal> {
    let claims = c.card_claims();
    let mut cards: Vec<&str> = claims.keys().copied().collect();
    cards.sort_unstable();
    let mut out = IndexMap::new();
    let mut left = credit;
    for card in cards {
        if left <= ZERO {
            break;
        }
        let take = left.min(claims[card].max(ZERO));
        out.insert(card, take);
        left -= take;
    }
    out
}
