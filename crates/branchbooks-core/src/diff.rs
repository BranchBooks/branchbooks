//! Budget diff between two Books, such as trunk and a scenario branch, in two layers: the rules
//! (targets in force, by category) and their effect (each category's numbers and Unassigned
//! per month). Categories and months are the keys, so entries need no ids.

use indexmap::IndexSet;

use crate::book::Book;
use crate::budget::{CategoryRow, MonthView, Target};
use crate::date::Month;
use crate::money::Decimal;

/// A category's target changed over a run of months. `None` is "no target".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleChange {
    pub category: String,
    /// First and last month of the run, inclusive.
    pub from: Month,
    pub to: Month,
    pub before: Option<Target>,
    pub after: Option<Target>,
}

/// The numbers a change can move for one category in one month.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Effect {
    pub assigned: Decimal,
    pub target_allocated: Decimal,
    pub available: Decimal,
    pub shortfall: Decimal,
}

impl Effect {
    fn of(row: Option<&CategoryRow>) -> Self {
        row.map_or_else(Effect::default, |r| Effect {
            assigned: r.assigned,
            target_allocated: r.target_allocated,
            available: r.available,
            shortfall: r.shortfall,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CategoryChange {
    pub month: Month,
    pub category: String,
    pub before: Effect,
    pub after: Effect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnassignedChange {
    pub month: Month,
    pub before: Decimal,
    pub after: Decimal,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BudgetDiff {
    /// The intent: targets that differ, in category order, then by month.
    pub rules: Vec<RuleChange>,
    /// The effect: per month, then category, the numbers that differ.
    pub categories: Vec<CategoryChange>,
    /// Unassigned as of each month, where it differs.
    pub unassigned: Vec<UnassignedChange>,
}

/// The same rule, whatever date set it or how its amount was written.
fn same_rule(a: &Option<Target>, b: &Option<Target>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.amount == b.amount
                && a.due == b.due
                && a.recurrence == b.recurrence
                && a.rollover == b.rollover
        }
        _ => false,
    }
}

fn target_of<'a>(v: &'a MonthView, category: &str) -> &'a Option<Target> {
    const NONE: &Option<Target> = &None;
    v.category(category).map_or(NONE, |r| &r.target)
}

/// How `b` differs from `a` in the months `from` to `to` (inclusive). Each month's
/// Unassigned is as of that month.
pub fn diff_books(a: &mut Book, b: &mut Book, from: Month, to: Month) -> BudgetDiff {
    let mut months = Vec::new();
    let mut m = from;
    while m <= to {
        months.push(m);
        m = m.next();
    }
    let views: Vec<(MonthView, MonthView)> = months
        .iter()
        .map(|&m| (a.budget(m, None), b.budget(m, None)))
        .collect();

    let mut out = BudgetDiff::default();
    let mut names: Vec<&String> = views
        .iter()
        .flat_map(|(va, vb)| {
            va.categories
                .iter()
                .chain(&vb.categories)
                .map(|r| &r.category)
        })
        .collect::<IndexSet<_>>()
        .into_iter()
        .collect();
    names.sort();

    for name in &names {
        let mut run: Option<RuleChange> = None;
        for (&month, (va, vb)) in months.iter().zip(&views) {
            let (ta, tb) = (target_of(va, name), target_of(vb, name));
            let continues = run
                .as_ref()
                .is_some_and(|r| same_rule(&r.before, ta) && same_rule(&r.after, tb));
            if continues {
                run.as_mut().expect("checked").to = month;
                continue;
            }
            out.rules.extend(run.take());
            if !same_rule(ta, tb) {
                run = Some(RuleChange {
                    category: (*name).clone(),
                    from: month,
                    to: month,
                    before: ta.clone(),
                    after: tb.clone(),
                });
            }
        }
        out.rules.extend(run);
    }

    for (&month, (va, vb)) in months.iter().zip(&views) {
        for name in &names {
            let (before, after) = (Effect::of(va.category(name)), Effect::of(vb.category(name)));
            if before != after {
                out.categories.push(CategoryChange {
                    month,
                    category: (*name).clone(),
                    before,
                    after,
                });
            }
        }
        if va.unassigned != vb.unassigned {
            out.unassigned.push(UnassignedChange {
                month,
                before: va.unassigned,
                after: vb.unassigned,
            });
        }
    }
    out
}
