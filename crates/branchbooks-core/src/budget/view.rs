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
    /// Rollover overspending is on at the month's end (rule 6a).
    pub rollover_overspending: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiabilityRow {
    pub account: String,
    /// Payment envelope Available: money set aside for this card.
    pub available: Decimal,
    pub owed: Decimal,
    /// Credit overspending on this card still in categories, by category (rule 7): to cover
    /// there, so it is not counted in `underfunded`.
    pub in_categories: Vec<(String, Decimal)>,
    /// Paid in full (no target): what it owes beyond its envelope and `in_categories`, debt from
    /// before the budget. Paid monthly (a target): what this month's payment could not be funded.
    pub underfunded: Decimal,
    /// The monthly payment target in force this month, if any.
    pub target: Option<Target>,
    /// Allocated by the target this month (virtual).
    pub target_allocated: Decimal,
    /// What the envelope and `in_categories` hold beyond what the card owes (rule 7a): money set
    /// aside twice, which can go back to Unassigned.
    pub overfunded: Decimal,
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
    pub liabilities: Vec<LiabilityRow>,
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

    pub fn liability(&self, name: &str) -> Option<&LiabilityRow> {
        self.liabilities.iter().find(|c| c.account == name)
    }
}
