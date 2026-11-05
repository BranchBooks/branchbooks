//! One month's budget as the views read it.

use super::*;

// ---------------------------------------------------------------------------------------------
// Month view

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CategoryRow {
    pub category: String,
    pub carried_in: Decimal,
    pub assigned: Decimal,
    /// Allocated by the category's target this month (virtual).
    pub target_allocated: Decimal,
    pub moved: Decimal,
    pub activity: Decimal,
    pub available: Decimal,
    pub cash_overspent: Decimal,
    pub credit_overspent: Decimal,
    /// The target in force this month, if any.
    pub target: Option<Target>,
    /// What the target needed and Unassigned could not fund.
    pub shortfall: Decimal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardRow {
    pub card: String,
    /// Payment envelope Available: money set aside for this card.
    pub available: Decimal,
    pub owed: Decimal,
    pub underfunded: Decimal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonthView {
    pub month: Month,
    pub currency: String,
    /// The date Unassigned was computed for.
    pub today: Date,
    /// As of today's month, not the viewed month.
    pub unassigned: Decimal,
    /// Assigned to months after today's month.
    pub assigned_to_future: Decimal,
    pub cash: Decimal,
    pub categories: Vec<CategoryRow>,
    pub cards: Vec<CardRow>,
    /// Rule 15 warnings: never change the numbers.
    pub warnings: Vec<Diagnostic>,
    /// Flags in this month: uncategorized off-budget transfers, unfunded categories, blocked
    /// envelope entries.
    pub flags: Vec<Diagnostic>,
    /// Targets this month could not fully fund: shortfall alerts.
    pub shortfalls: Vec<Shortfall>,
}

impl MonthView {
    pub fn category(&self, name: &str) -> Option<&CategoryRow> {
        self.categories.iter().find(|c| c.category == name)
    }

    pub fn card(&self, name: &str) -> Option<&CardRow> {
        self.cards.iter().find(|c| c.card == name)
    }
}
