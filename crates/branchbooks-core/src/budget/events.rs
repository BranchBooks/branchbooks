//! What the budget reacts to: transactions, assignments, moves and targets, from ledger entries.

use super::*;

// ---------------------------------------------------------------------------------------------
// Events

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spend {
    pub category: String,
    pub amount: Decimal,
    pub card: Option<String>,
}

/// What a transaction does to the budget.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TxEffect {
    pub cash: Decimal,
    pub card_delta: Vec<(String, Decimal)>,
    pub spends: Vec<Spend>,
    pub payments: Vec<(String, Decimal)>,
    pub flags: Vec<Diagnostic>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recurrence {
    /// Every month (T2's default).
    Monthly,
    /// Once, by the due month.
    Once,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rollover {
    /// What a month fails to fund is still owed later (the default for one-time targets).
    CarryShortfall,
    /// Each month starts fresh (the default for monthly targets).
    Reset,
}

impl Recurrence {
    pub fn as_str(self) -> &'static str {
        match self {
            Recurrence::Monthly => "monthly",
            Recurrence::Once => "once",
        }
    }
}

impl Rollover {
    pub fn as_str(self) -> &'static str {
        match self {
            Rollover::CarryShortfall => "carry-shortfall",
            Rollover::Reset => "reset",
        }
    }
}

/// A target rule: what a category should receive (T1 to T7 in docs/BUDGET-RULES.md). Allocations it implies
/// are computed when a month is funded and never written to the ledger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub amount: Decimal,
    /// Digits after the point in the amount as written: allocations are rounded down to it.
    pub scale: u32,
    /// The due date of a one-time target.
    pub due: Option<Date>,
    pub recurrence: Recurrence,
    pub rollover: Rollover,
    /// The date of the entry that set it.
    pub since: Date,
}

impl Target {
    /// The month a one-time target is due, or `month` for a monthly one (T5's funding order).
    pub(super) fn due_month(&self, month: Month) -> Month {
        self.due.map_or(month, Date::month)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventKind {
    Tx(TxEffect),
    /// An assignment to a category, or (`card`) to a card's payment envelope (decision 7).
    Assign {
        category: String,
        amount: Decimal,
        card: bool,
    },
    Move {
        from: String,
        to: String,
        amount: Decimal,
    },
    /// A target rule for a category from this date on (T1); an amount of zero ends it.
    Target {
        category: String,
        target: Target,
    },
}

/// Where an event came from: its entry's id, date, tie-break order and location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventBase {
    /// Identifies the event for `remove`; the Book uses the entry's id.
    pub id: u64,
    pub date: Date,
    pub seq: u64,
    pub file: Arc<str>,
    pub line: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BudgetEvent {
    pub base: EventBase,
    pub kind: EventKind,
}

impl BudgetEvent {
    fn kind_order(&self) -> u8 {
        match self.kind {
            EventKind::Tx(_) => 0,
            // Targets only take effect when the month is funded, so their order within a day
            // changes nothing; they sort with assignments.
            EventKind::Assign { .. } | EventKind::Target { .. } => 1,
            EventKind::Move { .. } => 2,
        }
    }

    pub fn date(&self) -> Date {
        self.base.date
    }
}

impl EventBase {
    fn of(e: &Entry, id: u64) -> Self {
        EventBase {
            id,
            date: e.date,
            seq: e.seq,
            file: e.file.clone(),
            line: e.line,
        }
    }
}

pub fn compare_events(a: &BudgetEvent, b: &BudgetEvent) -> Ordering {
    a.base
        .date
        .cmp(&b.base.date)
        .then(a.kind_order().cmp(&b.kind_order()))
        .then(a.base.seq.cmp(&b.base.seq))
}

/// Turns a booked ledger transaction into a budget event, or `None` when it does not touch the
/// budget.
pub fn tx_event(e: &Entry, cfg: &BudgetConfig, id: u64) -> Option<BudgetEvent> {
    let t = e.txn()?;
    let mut cash = ZERO;
    let mut cards: IndexMap<&str, Decimal> = IndexMap::new();
    let mut category_postings: Vec<(String, Decimal)> = Vec::new();
    let mut off_out = ZERO;
    let mut flags = Vec::new();
    let mut flag =
        |code: &str, message: String| flags.push(Diagnostic::new(&e.file, e.line, code, message));

    for p in &t.postings {
        let Some(units) = &p.units else { continue };
        if units.currency != cfg.currency {
            continue; // rule 13
        }
        let n = units.number;
        match cfg.role(&p.account) {
            Some(AccountRole::Cash) => cash += n,
            Some(AccountRole::Card) => *cards.entry(p.account.as_str()).or_insert(ZERO) += n,
            Some(AccountRole::Category) => category_postings.push((p.account.clone(), n)),
            Some(AccountRole::OffBudget) => off_out += n,
            _ => {}
        }
    }
    let cards_touched: Vec<(String, Decimal)> = cards
        .into_iter()
        .filter(|(_, v)| !v.is_zero())
        .map(|(c, v)| (c.to_owned(), v))
        .collect();
    let on_budget_touched = !cash.is_zero() || !cards_touched.is_empty();
    if !on_budget_touched && category_postings.is_empty() {
        return None;
    }

    // Rule 8: money leaving the budget for an off-budget account needs a budgeted category. Only
    // the off-budget amount that on-budget money paid for counts: what left cash and cards, less
    // what the same transaction spent in categories. Money that reaches an off-budget account from
    // income (a 401k contribution in a paycheck) is not a transfer.
    let card_sum: Decimal = cards_touched.iter().map(|(_, v)| *v).sum();
    let spent: Decimal = category_postings.iter().map(|(_, n)| *n).sum();
    let paid_off = off_out.min((-(cash + card_sum) - spent).max(ZERO));
    if paid_off > ZERO {
        let meta = e
            .meta
            .get("category")
            .or_else(|| t.postings.iter().find_map(|p| p.meta.get("category")));
        let name = match meta {
            Some(MetaValue::String(s) | MetaValue::Account(s)) => Some(s.as_str()),
            _ => None,
        };
        match name {
            Some(n) if cfg.is_category(n) => category_postings.push((n.to_owned(), paid_off)),
            Some(n) => flag(
                "uncategorized",
                format!(
                    "category {n} is not a budgeted category; transfer to an off-budget account is uncategorized"
                ),
            ),
            None => flag(
                "uncategorized",
                "transfer to an off-budget account has no category: metadata".to_owned(),
            ),
        }
    }

    let mut spends = Vec::new();
    let mut payments = Vec::new();
    if on_budget_touched {
        let funded = fund_spends(&category_postings, cash, &cards_touched);
        spends = funded.spends;
        payments = card_payments(funded.card_inflow, funded.card_outflow);
    } else {
        flag(
            "unfunded",
            "budgeted category posting with no on-budget account; ignored by the budget".to_owned(),
        );
    }

    Some(BudgetEvent {
        base: EventBase::of(e, id),
        kind: EventKind::Tx(TxEffect {
            cash,
            card_delta: cards_touched,
            spends,
            payments,
            flags,
        }),
    })
}

/// How a transaction's category postings drew on its on-budget accounts.
struct Funded {
    spends: Vec<Spend>,
    /// Money arriving on each card that no refund used.
    card_inflow: Vec<(String, Decimal)>,
    /// Money charged to each card that no spending used.
    card_outflow: Vec<(String, Decimal)>,
}

/// Money arriving on a card is a payment against its payment envelope. What came from another card
/// in the same transaction (a balance transfer) moves that much into the charged card's envelope, as
/// a negative payment, so Unassigned does not change (docs/DESIGN.md, decision 8).
fn card_payments(
    inflow: Vec<(String, Decimal)>,
    outflow: Vec<(String, Decimal)>,
) -> Vec<(String, Decimal)> {
    let mut left: Decimal = inflow.iter().map(|(_, v)| *v).sum();
    let mut payments = inflow;
    for (card, charged) in outflow {
        let moved = left.min(charged);
        if moved > ZERO {
            left -= moved;
            payments.push((card, -moved));
        }
    }
    payments.retain(|(_, v)| !v.is_zero());
    payments
}

/// Attributes category postings to the on-budget accounts that funded them: cash first, then
/// cards in account-name order. Spending draws on money leaving those accounts; refunds and other
/// inflows draw on money arriving. Anything left over (for example, a category posting balanced by
/// income in the same transaction) counts as cash.
fn fund_spends(
    postings: &[(String, Decimal)],
    cash: Decimal,
    cards: &[(String, Decimal)],
) -> Funded {
    let mut out = Vec::new();
    let mut sorted: Vec<&(String, Decimal)> = cards.iter().collect();
    sorted.sort_by(|(a, _), (b, _)| a.cmp(b));
    // pools[0] is cash; outflow pools serve spending, inflow pools serve refunds.
    let pools = |sign: Decimal| -> Vec<(Option<String>, Decimal)> {
        std::iter::once((None, (cash * sign).max(ZERO)))
            .chain(
                sorted
                    .iter()
                    .map(|(c, v)| (Some(c.clone()), (*v * sign).max(ZERO))),
            )
            .collect()
    };
    let mut outflow = pools(Decimal::NEGATIVE_ONE);
    let mut inflow = pools(Decimal::ONE);
    for (category, amount) in postings {
        let spending = *amount > ZERO;
        let pools = if spending { &mut outflow } else { &mut inflow };
        let signed = |v: Decimal| if spending { v } else { -v };
        let mut left = amount.abs();
        for (card, pool) in pools.iter_mut() {
            if left.is_zero() {
                break;
            }
            let take = left.min(*pool);
            if take.is_zero() {
                continue;
            }
            *pool -= take;
            left -= take;
            out.push(Spend {
                category: category.clone(),
                amount: signed(take),
                card: card.clone(),
            });
        }
        if left > ZERO {
            out.push(Spend {
                category: category.clone(),
                amount: signed(left),
                card: None,
            });
        }
    }
    let cards_only = |pools: Vec<(Option<String>, Decimal)>| {
        pools
            .into_iter()
            .filter_map(|(card, left)| Some((card?, left)))
            .collect()
    };
    Funded {
        spends: out,
        card_inflow: cards_only(inflow),
        card_outflow: cards_only(outflow),
    }
}

/// The budget event of an `envelope-assign` or `envelope-move`, and flags for one that is
/// malformed or names something that is not a budgeted category.
pub fn custom_event(
    e: &Entry,
    cfg: &BudgetConfig,
    id: u64,
) -> (Option<BudgetEvent>, Vec<Diagnostic>) {
    let Some(c) = e.custom() else {
        return (None, Vec::new());
    };
    let bad = |message: String| {
        (
            None,
            vec![Diagnostic::new(&e.file, e.line, "envelope", message)],
        )
    };
    let event = |kind| {
        (
            Some(BudgetEvent {
                base: EventBase::of(e, id),
                kind,
            }),
            Vec::new(),
        )
    };
    let amount_of = |v: Option<&CustomValue>| match v {
        Some(CustomValue::Amount(a)) if a.currency == cfg.currency => Some(a.number),
        _ => None,
    };
    match c.name.as_str() {
        "envelope-assign" => {
            let (Some(CustomValue::Account(acct)), 2) = (c.values.first(), c.values.len()) else {
                return bad("envelope-assign takes an account and an amount".to_owned());
            };
            // A card takes assignments too: money set aside to pay what it already owes (decision 7).
            let card = cfg.role(acct) == Some(AccountRole::Card);
            if !cfg.is_category(acct) && !card {
                return bad(format!("{acct} is not a budgeted category or card"));
            }
            let Some(amount) = amount_of(c.values.get(1)) else {
                return bad(format!(
                    "envelope-assign amount must be in {}",
                    cfg.currency
                ));
            };
            event(EventKind::Assign {
                category: acct.clone(),
                amount,
                card,
            })
        }
        "envelope-move" => {
            let (Some(CustomValue::Account(from)), Some(CustomValue::Account(to)), 3) =
                (c.values.first(), c.values.get(1), c.values.len())
            else {
                return bad("envelope-move takes two accounts and an amount".to_owned());
            };
            if let Some(a) = [from, to].into_iter().find(|a| !cfg.is_category(a)) {
                return bad(format!("{a} is not a budgeted category"));
            }
            let Some(amount) = amount_of(c.values.get(2)) else {
                return bad(format!("envelope-move amount must be in {}", cfg.currency));
            };
            event(EventKind::Move {
                from: from.clone(),
                to: to.clone(),
                amount,
            })
        }
        "target" => match target_of(e, c, cfg) {
            Ok((category, target)) => event(EventKind::Target { category, target }),
            Err(message) => bad(message),
        },
        _ => (None, Vec::new()),
    }
}

/// The category and rule of a `custom "target"` entry (T2, T7), or why it is not one.
fn target_of(e: &Entry, c: &Custom, cfg: &BudgetConfig) -> Result<(String, Target), String> {
    let (Some(CustomValue::Account(category)), 2) = (c.values.first(), c.values.len()) else {
        return Err("target takes a category and an amount".to_owned());
    };
    if !cfg.is_category(category) {
        return Err(format!("{category} is not a budgeted category"));
    }
    let Some(CustomValue::Amount(amount)) = c
        .values
        .get(1)
        .filter(|v| matches!(v, CustomValue::Amount(a) if a.currency == cfg.currency))
    else {
        return Err(format!("target amount must be in {}", cfg.currency));
    };
    if amount.number < ZERO {
        return Err("target amount can't be negative".to_owned());
    }
    let text = |key: &str| match e.meta.get(key) {
        None => Ok(None),
        Some(MetaValue::String(s)) => Ok(Some(s.as_str())),
        Some(_) => Err(format!("target {key} must be a string")),
    };
    let due = match e.meta.get("due") {
        None => None,
        Some(MetaValue::Date(d)) => Some(*d),
        Some(_) => return Err("target due must be a date".to_owned()),
    };
    let recurrence = match (text("recurrence")?, &due) {
        (None | Some("once"), Some(_)) => Recurrence::Once,
        (None | Some("monthly"), None) => Recurrence::Monthly,
        (Some("once"), None) => return Err("a one-time target needs a due date".to_owned()),
        (Some("monthly"), Some(_)) => return Err("a monthly target takes no due date".to_owned()),
        (Some(_), _) => return Err("target recurrence must be \"monthly\" or \"once\"".to_owned()),
    };
    let rollover = match (text("rollover")?, recurrence) {
        (None, Recurrence::Once) | (Some("carry-shortfall"), _) => Rollover::CarryShortfall,
        (None | Some("reset"), Recurrence::Monthly) => Rollover::Reset,
        (Some("reset"), Recurrence::Once) => {
            return Err("rollover \"reset\" needs a monthly target".to_owned());
        }
        (Some(_), _) => {
            return Err("target rollover must be \"carry-shortfall\" or \"reset\"".to_owned());
        }
    };
    let target = Target {
        amount: amount.number,
        scale: amount.scale,
        due,
        recurrence,
        rollover,
        since: e.date,
    };
    Ok((category.clone(), target))
}
