//! Which accounts are cash, cards and categories, from `budget:` metadata on open directives.

use super::*;

// ---------------------------------------------------------------------------------------------
// Account classification (flags from `budget:` metadata on open directives; see docs/BUDGET-RULES.md)

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AccountRole {
    Cash,
    Liability,
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
            AccountRole::Liability => "liability",
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
    pub liabilities: Vec<String>,
    /// A liability switched on budget later (`budget-from:` on its open): before this date it is
    /// off-budget, and what it owed then is debt from before the budget (decision 7b).
    pub on_from: std::collections::HashMap<String, Date>,
    /// Accounts that only group their sub-accounts. Each keeps a role, so anything posted to one by
    /// hand still counts, and is flagged.
    pub placeholders: std::collections::HashSet<String>,
    pub diagnostics: Vec<Diagnostic>,
}

impl BudgetConfig {
    pub fn role(&self, account: &str) -> Option<AccountRole> {
        self.roles.get(account).copied()
    }

    /// The role on `date`: a liability is off-budget before its `budget-from:` date.
    pub(super) fn role_on(&self, account: &str, date: Date) -> Option<AccountRole> {
        match self.on_from.get(account) {
            Some(from) if date < *from => Some(AccountRole::OffBudget),
            _ => self.role(account),
        }
    }

    pub fn is_placeholder(&self, account: &str) -> bool {
        self.placeholders.contains(account)
    }

    pub(super) fn is_category(&self, account: &str) -> bool {
        self.role(account) == Some(AccountRole::Category) && !self.is_placeholder(account)
    }
}

pub fn classify_accounts(accounts: &Accounts, options: &Options) -> BudgetConfig {
    let currency = options
        .get("operating_currency")
        .and_then(|v| v.first())
        .cloned()
        .unwrap_or_else(|| "USD".to_owned());
    let mut roles = IndexMap::new();
    let mut placeholders = std::collections::HashSet::new();
    let mut on_from = std::collections::HashMap::new();
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
        let placeholder = flag == Some("placeholder");
        if placeholder {
            placeholders.insert(name.clone());
        }
        let root = &name[..name.find(':').unwrap_or(name.len())];
        let currencies = info.currencies();
        let other_currency = !currencies.is_empty() && !currencies.contains(&currency);
        let role = match root {
            "Assets" | "Liabilities" => {
                if flag == Some("on") && !other_currency {
                    if root == "Assets" {
                        AccountRole::Cash
                    } else {
                        if let Some(MetaValue::Date(d)) = info.open.meta.get("budget-from") {
                            on_from.insert(name.clone(), *d);
                        }
                        AccountRole::Liability
                    }
                } else {
                    match flag {
                        None => diag(format!("{name} has no budget flag; treated as off-budget")),
                        Some(f) if f != "on" && f != "off" && !placeholder => {
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
                    if flag.is_some() && !placeholder {
                        diag(format!(
                            "{name}: the only budget flag for expense accounts is \"excluded\""
                        ));
                    }
                    AccountRole::Category
                }
            }
            "Income" => {
                if flag.is_some_and(|f| f != "excluded") && !placeholder {
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
    let mut categories: Vec<String> = pick(AccountRole::Category);
    categories.retain(|c| !placeholders.contains(c));
    let liabilities = pick(AccountRole::Liability);
    BudgetConfig {
        currency,
        roles,
        categories,
        liabilities,
        on_from,
        placeholders,
        diagnostics,
    }
}
