//! Which accounts are cash, cards and categories, from `budget:` metadata on open directives.

use super::*;

// ---------------------------------------------------------------------------------------------
// Account classification (flags from `budget:` metadata on open directives; see docs/BUDGET-RULES.md)

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AccountRole {
    Cash,
    Card,
    OffBudget,
    Category,
    Excluded,
    Income,
    Equity,
}

impl AccountRole {
    /// The role's name.
    pub fn name(self) -> &'static str {
        match self {
            AccountRole::Cash => "cash",
            AccountRole::Card => "card",
            AccountRole::OffBudget => "off-budget",
            AccountRole::Category => "category",
            AccountRole::Excluded => "excluded",
            AccountRole::Income => "income",
            AccountRole::Equity => "equity",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct BudgetConfig {
    pub currency: String,
    /// Every account's role, in account-name order.
    pub roles: IndexMap<String, AccountRole>,
    pub categories: Vec<String>,
    pub cards: Vec<String>,
    pub diagnostics: Vec<Diagnostic>,
}

impl BudgetConfig {
    pub fn role(&self, account: &str) -> Option<AccountRole> {
        self.roles.get(account).copied()
    }

    pub(super) fn is_category(&self, account: &str) -> bool {
        self.role(account) == Some(AccountRole::Category)
    }
}

pub fn classify_accounts(accounts: &Accounts, options: &Options) -> BudgetConfig {
    let currency = options
        .get("operating_currency")
        .and_then(|v| v.first())
        .cloned()
        .unwrap_or_else(|| "USD".to_owned());
    let mut roles = IndexMap::new();
    let mut diagnostics = Vec::new();

    let mut sorted: Vec<_> = accounts.iter().collect();
    sorted.sort_by_key(|(a, _)| *a);
    for (name, info) in sorted {
        let mut diag = |message: String| {
            diagnostics.push(Diagnostic::new(
                &info.open.file,
                info.open.line,
                "budget-flag",
                message,
            ));
        };
        let flag = match info.open.meta.get("budget") {
            Some(MetaValue::String(s)) => Some(s.as_str()),
            Some(_) => Some("<invalid>"),
            None => None,
        };
        let root = &name[..name.find(':').unwrap_or(name.len())];
        let currencies = info.currencies();
        let other_currency = !currencies.is_empty() && !currencies.contains(&currency);
        let role = match root {
            "Assets" | "Liabilities" => {
                if flag == Some("on") && !other_currency {
                    if root == "Assets" {
                        AccountRole::Cash
                    } else {
                        AccountRole::Card
                    }
                } else {
                    match flag {
                        None => diag(format!("{name} has no budget flag; treated as off-budget")),
                        Some(f) if f != "on" && f != "off" => {
                            diag(format!("{name}: budget flag must be \"on\" or \"off\""));
                        }
                        _ => {}
                    }
                    AccountRole::OffBudget
                }
            }
            "Expenses" => {
                if flag == Some("excluded") {
                    AccountRole::Excluded
                } else {
                    if flag.is_some() {
                        diag(format!(
                            "{name}: the only budget flag for expense accounts is \"excluded\""
                        ));
                    }
                    AccountRole::Category
                }
            }
            "Income" => {
                if flag.is_some_and(|f| f != "excluded") {
                    diag(format!(
                        "{name}: the only budget flag for income accounts is \"excluded\""
                    ));
                }
                AccountRole::Income
            }
            _ => AccountRole::Equity,
        };
        roles.insert(name.clone(), role);
    }
    let pick = |r: AccountRole| {
        roles
            .iter()
            .filter(|(_, v)| **v == r)
            .map(|(k, _)| k.clone())
            .collect()
    };
    let categories = pick(AccountRole::Category);
    let cards = pick(AccountRole::Card);
    BudgetConfig {
        currency,
        roles,
        categories,
        cards,
        diagnostics,
    }
}
