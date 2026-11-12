//! Accounts: a form to open an account or category, then every one grouped by type with its balance;
//! its name opens its transactions; Edit (date, currencies, budget flag), Reconcile (R1 to R3),
//! Close (on a date), Reopen and Delete.

use std::str::FromStr;

use branchbooks_core::date::Date;
use branchbooks_core::types::{Entry, MetaValue};
use branchbooks_core::write::render::{
    BudgetFlag, CloseInput, EntryInput, MetaInputValue, OpenInput, PostingInput, TransactionInput,
    is_account,
};
use branchbooks_core::write::workspace::{Operation, ReconcileInput};
use dioxus::prelude::*;
use rust_decimal::Decimal;

use crate::budget::apply;
use crate::journal::{Shared, meta, run, use_today_date};
use crate::store::Backend;
use crate::text::{fmt, ledger_part, norm_amount, parse_amount, short, words};

/// Account types in the order shown, with their headings. Liabilities, income and equity are
/// credits: shown positive, as amounts owed or earned.
const ROOTS: [(&str, &str, bool); 5] = [
    ("Assets", "Assets", false),
    ("Liabilities", "Liabilities", true),
    ("Income", "Income", true),
    ("Expenses", "Expense categories", false),
    ("Equity", "Equity", true),
];

/// One row: the account, its open and close entries, and its balance in the book's currency.
#[derive(Clone, PartialEq)]
struct Row {
    account: String,
    open: Entry,
    close: Option<Entry>,
    balance: Decimal,
}

/// One line of the account tree, as `GnuCash` shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    /// The full name: Expenses:Food.
    pub path: String,
    /// The last part, for people: Food.
    pub label: String,
    /// 0 for a part right under the type (Expenses:Food), 1 below that, and so on.
    pub depth: usize,
    /// The balance with every sub-account's, as `GnuCash`'s Total.
    pub total: Decimal,
    /// It has sub-accounts, so it can be collapsed.
    pub parent: bool,
    /// It is an account (opened); else only a folder for the ones below it.
    pub account: bool,
}

/// The accounts of one type as a tree: every level of their names, parents before children,
/// siblings by name, with totals rolled up.
pub fn tree(accounts: &[(&str, Decimal)]) -> Vec<Node> {
    let mut paths: std::collections::BTreeSet<Vec<&str>> = std::collections::BTreeSet::new();
    for (a, _) in accounts {
        let parts: Vec<&str> = a.split(':').collect();
        for i in 2..=parts.len() {
            paths.insert(parts[..i].to_vec());
        }
    }
    paths
        .iter()
        .map(|parts| {
            let path = parts.join(":");
            let under = format!("{path}:");
            Node {
                label: words(parts[parts.len() - 1]),
                depth: parts.len() - 2,
                total: accounts
                    .iter()
                    .filter(|(a, _)| *a == path || a.starts_with(&under))
                    .map(|(_, b)| *b)
                    .sum(),
                parent: accounts.iter().any(|(a, _)| a.starts_with(&under)),
                account: accounts.iter().any(|(a, _)| *a == path),
                path,
            }
        })
        .collect()
}

/// What a row has open: "close" (a date), "delete" (a question), "edit" or "reconcile".
type Mode = Option<(String, &'static str)>;

#[component]
pub fn Accounts(
    backend: Backend,
    core: Shared,
    version: Signal<Option<String>>,
    notice: Signal<Option<(bool, String)>>,
    today: Date,
    /// Opens the Journal filtered to an account, to see and edit its transactions.
    onshow: EventHandler<String>,
) -> Element {
    let mut mode = use_signal(|| None as Mode);
    let mut show_closed = use_signal(|| false);
    // "all", "accounts" (what you have and owe) or "categories" (income and expenses).
    let mut view = use_signal(|| "all");
    // Types whose whole group is folded to its heading. Equity is mostly opening balances.
    let mut folded = use_signal(|| vec!["Equity"]);
    let mut adding = use_signal(|| false);
    // Parents whose sub-accounts are hidden, as GnuCash's tree collapses.
    let collapsed = use_signal(Vec::<String>::new);
    let _ = version();
    let mut rows: Vec<Row> = {
        let mut c = core.0.borrow_mut();
        let book = c.book();
        let currency = book.currency().to_owned();
        let balances = book.balances().clone();
        book.account_info()
            .iter()
            .map(|(a, i)| Row {
                account: a.clone(),
                open: i.open.clone(),
                close: i.close.clone(),
                balance: branchbooks_core::ledger::balance_of(&balances, a, &currency),
            })
            .collect()
    };
    rows.sort_by(|a, b| a.account.cmp(&b.account));
    let closed = rows.iter().filter(|r| r.close.is_some()).count();
    // Saves `op`, then closes the row's form.
    let save = {
        let (backend, core) = (backend.clone(), core.clone());
        move |op: Operation, done: String| {
            let (backend, core, mut mode) = (backend.clone(), core.clone(), mode);
            if run(backend, core, op, &done, version, notice) {
                mode.set(None);
            }
        }
    };
    let at = {
        let core = core.clone();
        move |e: &Entry| core.0.borrow().ref_of(e)
    };
    let mode_of = |a: &str| mode().filter(|(m, _)| m == a).map(|(_, k)| k);
    let categories = |root: &str| root == "Income" || root == "Expenses";
    // Two columns on a wide window: what you have and owe, then the expense categories.
    let columns: Vec<Vec<(&'static str, &'static str, bool)>> = [false, true]
        .into_iter()
        .map(|right| {
            ROOTS
                .into_iter()
                .filter(|r| (r.0 == "Expenses") == right)
                .filter(|r| match view() {
                    "accounts" => !categories(r.0),
                    "categories" => categories(r.0),
                    _ => true,
                })
                .collect::<Vec<_>>()
        })
        .filter(|c| !c.is_empty())
        .collect();
    rsx! {
        div { class: "accounts-page",
            div { class: "accounts-toolbar",
                h2 { "Accounts and categories" }
                div { class: "segmented", role: "group", "aria-label": "Show",
                    for (id, text) in [("all", "All"), ("accounts", "Accounts"), ("categories", "Categories")] {
                        button { key: "{id}", r#type: "button", "aria-pressed": view() == id,
                            onclick: move |_| view.set(id), {text}
                        }
                    }
                }
                if closed > 0 {
                    button { r#type: "button", class: "link", onclick: move |_| show_closed.toggle(),
                        if show_closed() { "Hide closed" } else { "Show closed ({closed})" }
                    }
                }
                button { r#type: "button", class: "add-button", "aria-expanded": adding(),
                    onclick: move |_| adding.toggle(), "Add account"
                }
            }
            if adding() {
                AddAccount { backend: backend.clone(), core: core.clone(), version, notice, today: today,
                    onclose: move |()| adding.set(false),
                }
            }
            div { class: "account-cols",
            for column in columns {
            div { key: "{column[0].0}", class: "account-col",
                for (root, heading, credit) in column {
                    {
                        let group: Vec<&Row> = rows
                            .iter()
                            .filter(|r| r.account.split(':').next() == Some(root))
                            .filter(|r| show_closed() || r.close.is_none())
                            .collect();
                        let sign = if credit { Decimal::NEGATIVE_ONE } else { Decimal::ONE };
                        let total: Decimal = group.iter().map(|r| r.balance).sum::<Decimal>() * sign;
                        let shut = folded.read().contains(&root);
                        rsx! {
                            if !group.is_empty() {
                                section { key: "{root}", class: "group", "aria-label": heading,
                                    button { r#type: "button", class: "group-head", "aria-expanded": !shut,
                                        onclick: move |_| {
                                            let mut f = folded.write();
                                            match f.iter().position(|r| *r == root) {
                                                Some(i) => {
                                                    f.remove(i);
                                                }
                                                None => f.push(root),
                                            }
                                        },
                                        span { span { class: "chevron", "aria-hidden": "true", if shut { "▸" } else { "▾" } } "{heading}" }
                                        span { class: "num", {fmt(total)} }
                                    }
                                    if !shut {
                                    table { class: "accounts",
                                    tbody {
                                    for node in tree(&group.iter().map(|r| (r.account.as_str(), r.balance)).collect::<Vec<_>>())
                                        .into_iter()
                                        .filter(|n| !collapsed.read().iter().any(|c| n.path.starts_with(&format!("{c}:"))))
                                    {
                                        if !node.account {
                                            tr { key: "{node.path}", class: "folder",
                                                td { title: "{node.path}",
                                                    span { style: "padding-left: {node.depth as f32 * 1.25}em" }
                                                    Toggle { node: node.clone(), collapsed }
                                                    "{node.label}"
                                                }
                                                td { class: "num", {fmt(node.total * sign)} }
                                                td {}
                                            }
                                        } else if let Some(r) = group.iter().find(|r| r.account == node.path).copied() {
                                        if mode_of(&r.account) == Some("edit") {
                                            tr { key: "{r.account}", class: "editing",
                                                td { colspan: 3,
                                                    OpenEditor { open: r.open.clone(), close: r.close.clone(),
                                                        oncancel: move |()| mode.set(None),
                                                        onsave: {
                                                            let (backend, core, a) = (backend.clone(), core.clone(), r.account.clone());
                                                            move |edit: AccountEdit| {
                                                                let (backend, core, a) = (backend.clone(), core.clone(), a.clone());
                                                                if save_account(backend, core, &a, edit, version, notice) {
                                                                    mode.set(None);
                                                                }
                                                            }
                                                        },
                                                    }
                                                }
                                            }
                                        } else if mode_of(&r.account) == Some("reconcile") {
                                            tr { key: "{r.account}", class: "editing",
                                                td { colspan: 3,
                                                    ReconcileForm { core: core.clone(), version, account: r.account.clone(),
                                                        today: today, sign,
                                                        oncancel: move |()| mode.set(None),
                                                        onsave: {
                                                            let save = save.clone();
                                                            move |(op, done): (Operation, String)| save(op, done)
                                                        },
                                                    }
                                                }
                                            }
                                        } else {
                                            tr { key: "{r.account}", "data-testid": "account",
                                                class: if r.close.is_some() { "closed" } else if node.parent { "parent" } else { "" },
                                                td { title: "{r.account}",
                                                    span { style: "padding-left: {node.depth as f32 * 1.25}em" }
                                                    Toggle { node: node.clone(), collapsed }
                                                    button { r#type: "button", class: "cell name", "aria-label": "Transactions in {r.account}",
                                                        onclick: {
                                                            let a = r.account.clone();
                                                            move |_| onshow.call(a.clone())
                                                        },
                                                        "{node.label}"
                                                    }
                                                    if let Some(c) = &r.close { " " span { class: "badge", "closed {c.date}" } }
                                                    if let Some(tag) = budget_tag(root, r.open.meta.str("budget")) {
                                                        " " span { class: "badge", {tag} }
                                                    }
                                                }
                                                td { class: "num", title: if node.parent { "With its sub-accounts" } else { "" }, {fmt(node.total * sign)} }
                                                td { class: "menu-cell",
                                                details { class: "row-menu",
                                                    summary { "aria-label": "More for {r.account}", "⋯" }
                                                    div { class: "menu",
                                                    span { class: "muted small-inline", "Opened {r.open.date}" }
                                                    button { r#type: "button", class: "link", "aria-label": "Transactions in {r.account} (menu)",
                                                        onclick: {
                                                            let a = r.account.clone();
                                                            move |_| onshow.call(a.clone())
                                                        },
                                                        "Transactions"
                                                    }
                                                    if let Some(input) = placeholder_of(&r.open).filter(|_| node.parent && r.close.is_none()) {
                                                        button { r#type: "button", class: "link", "aria-label": "Make {r.account} a placeholder",
                                                            onclick: {
                                                                let (save, at, open) = (save.clone(), at.clone(), r.open.clone());
                                                                move |_| {
                                                                    let done = format!("{} is now a placeholder.", input.account);
                                                                    // Worked out on the click: it reads the whole file.
                                                                    save(Operation::Edit(at(&open), EntryInput::Open(input.clone())), done);
                                                                }
                                                            },
                                                            "Make placeholder"
                                                        }
                                                    }
                                                    Actions { account: r.account.clone(), today: today,
                                                        closed: r.close.is_some(),
                                                        // Statements come for asset and liability accounts, not for groups of them.
                                                        reconcilable: (root == "Assets" || root == "Liabilities")
                                                            && r.open.meta.str("budget") != Some("placeholder"),
                                                        // Closing a group would not close what is in it.
                                                        closable: r.open.meta.str("budget") != Some("placeholder"),
                                                        mode: mode_of(&r.account),
                                                        onmode: {
                                                            let a = r.account.clone();
                                                            move |k: Option<&'static str>| mode.set(k.map(|k| (a.clone(), k)))
                                                        },
                                                        onclose: {
                                                            let (save, a) = (save.clone(), r.account.clone());
                                                            move |date: String| {
                                                                let input = CloseInput { date, account: a.clone() };
                                                                save(Operation::Add(EntryInput::Close(input)), format!("Closed {a}."));
                                                            }
                                                        },
                                                        onreopen: {
                                                            let (save, a, at, close) = (save.clone(), r.account.clone(), at.clone(), r.close.clone());
                                                            move |()| if let Some(c) = &close {
                                                                save(Operation::Delete(at(c)), format!("Reopened {a}."));
                                                            }
                                                        },
                                                        ondelete: {
                                                            let (save, a, at, open) = (save.clone(), r.account.clone(), at.clone(), r.open.clone());
                                                            move |()| save(Operation::Delete(at(&open)), format!("Deleted {a}."))
                                                        },
                                                    }
                                                    }
                                                }
                                                }
                                            }
                                        }
                                        }
                                    }
                                    }
                                    }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            }
            }
            p { class: "muted small",
                "Deleting an account removes its open entry; it is refused while transactions or envelope entries use it. "
                "Close it instead to keep its history."
            }
        }
    }
}

/// A badge for a budget flag worth noticing; none for the usual ones (on budget, a category).
/// An asset or liability with no flag is off budget too (DESIGN.md, decision 5).
fn budget_tag(root: &str, flag: Option<&str>) -> Option<&'static str> {
    match flag {
        Some("off") => Some("off budget"),
        Some("excluded") => Some("excluded"),
        Some("placeholder") => Some("placeholder"),
        None if root == "Assets" || root == "Liabilities" => Some("off budget"),
        _ => None,
    }
}

/// The `open` entry as a placeholder, keeping everything else about it; `None` when it already is one.
/// The write path refuses it for an account that holds transactions, and says why.
fn placeholder_of(open: &Entry) -> Option<OpenInput> {
    if open.meta.str("budget") == Some("placeholder") {
        return None;
    }
    let o = open.open()?;
    Some(OpenInput {
        date: open.date.to_string(),
        account: o.account.clone(),
        currencies: o.currencies.clone(),
        budget: Some(BudgetFlag::Placeholder),
        booking: o.booking.clone(),
        meta: meta(&open.meta)
            .into_iter()
            .filter(|(k, _)| k != "budget")
            .collect(),
    })
}

/// A parent's ▾ or ▸, which hides or shows its sub-accounts; a space the same width otherwise.
#[component]
fn Toggle(node: Node, collapsed: Signal<Vec<String>>) -> Element {
    if !node.parent {
        return rsx! { span { class: "tree-toggle", "aria-hidden": "true" } };
    }
    let shut = collapsed.read().contains(&node.path);
    rsx! {
        button { r#type: "button", class: "tree-toggle", "aria-expanded": !shut,
            "aria-label": if shut { "Show what is in {node.label}" } else { "Hide what is in {node.label}" },
            onclick: move |_| {
                let mut c = collapsed.write();
                match c.iter().position(|p| *p == node.path) {
                    Some(i) => {
                        c.remove(i);
                    }
                    None => c.push(node.path.clone()),
                }
            },
            if shut { "▸" } else { "▾" }
        }
    }
}

/// A row's buttons: Edit, Reconcile, Close (then a date), Reopen, and Delete (then a question).
#[component]
fn Actions(
    account: String,
    today: Date,
    closed: bool,
    reconcilable: bool,
    closable: bool,
    mode: Option<&'static str>,
    onmode: EventHandler<Option<&'static str>>,
    onclose: EventHandler<String>,
    onreopen: EventHandler<()>,
    ondelete: EventHandler<()>,
) -> Element {
    let mut date = use_today_date(today);
    match mode {
        Some("close") => rsx! {
            form { class: "inline", "aria-label": "Close {account}",
                onsubmit: move |e: FormEvent| {
                    e.prevent_default();
                    onclose.call(date.peek().clone());
                },
                input { r#type: "date", "aria-label": "Close date", value: "{date}", required: true,
                    oninput: move |e| date.set(e.value()),
                }
                button { r#type: "submit", "Close" }
                button { r#type: "button", class: "link", onclick: move |_| onmode.call(None), "Cancel" }
            }
        },
        Some("delete") => rsx! {
            span { class: "muted small", "Delete {short(&account)}?" }
            button { r#type: "button", class: "danger", onclick: move |_| ondelete.call(()), "Delete" }
            button { r#type: "button", class: "link", onclick: move |_| onmode.call(None), "Keep" }
        },
        _ => rsx! {
            button { r#type: "button", class: "link", "aria-label": "Edit {account}",
                onclick: move |_| onmode.call(Some("edit")), "Edit"
            }
            if closed {
                button { r#type: "button", class: "link", "aria-label": "Reopen {account}",
                    onclick: move |_| onreopen.call(()), "Reopen"
                }
            } else {
                if reconcilable {
                    button { r#type: "button", class: "link", "aria-label": "Reconcile {account}",
                        onclick: move |_| onmode.call(Some("reconcile")), "Reconcile"
                    }
                }
                if closable {
                    button { r#type: "button", class: "link", "aria-label": "Close {account}",
                        onclick: move |_| onmode.call(Some("close")), "Close"
                    }
                }
                // A closed account is reopened first: deleting its open would leave the close behind.
                button { r#type: "button", class: "link", "aria-label": "Delete {account}",
                    onclick: move |_| onmode.call(Some("delete")), "Delete"
                }
            }
        },
    }
}

/// What the account editor changed; each part is `None` when it was left as it was.
#[derive(Clone, PartialEq)]
pub struct AccountEdit {
    rename: Option<String>,
    /// The open, under the account's new name if it was renamed.
    open: Option<OpenInput>,
    close_date: Option<String>,
}

/// Says that `name` changed under the edit, such as by a sync.
fn gone(mut notice: Signal<Option<(bool, String)>>, name: &str) {
    notice.set(Some((
        false,
        format!("{name} changed while editing it. Try again."),
    )));
}

/// Saves an account edit as up to three commits, in order: the rename (every line that names
/// the account), the open's fields, then the close date. Stops at the first refusal.
fn save_account(
    backend: Backend,
    core: Shared,
    account: &str,
    edit: AccountEdit,
    version: Signal<Option<String>>,
    notice: Signal<Option<(bool, String)>>,
) -> bool {
    let mut name = account.to_owned();
    if let Some(to) = edit.rename {
        let op = Operation::RenameAccount {
            from: name.clone(),
            to: to.clone(),
        };
        let done = format!("Renamed {name} to {to}.");
        if !run(backend.clone(), core.clone(), op, &done, version, notice) {
            return false;
        }
        name = to;
    }
    // The entries as they are now: the rename rewrote their lines.
    let (open, close) = {
        let mut c = core.0.borrow_mut();
        let Some(info) = c.book().account_info().get(&name).cloned() else {
            drop(c);
            gone(notice, &name);
            return false;
        };
        (c.ref_of(&info.open), info.close.map(|e| c.ref_of(&e)))
    };
    if let Some(input) = edit.open {
        let op = Operation::Edit(open, EntryInput::Open(input));
        if !run(
            backend.clone(),
            core.clone(),
            op,
            &format!("Saved {name}."),
            version,
            notice,
        ) {
            return false;
        }
    }
    if let (Some(date), Some(_)) = (edit.close_date, &close) {
        // Read again: the open's edit may have moved the close's line.
        let close = {
            let mut c = core.0.borrow_mut();
            let Some(e) = c
                .book()
                .account_info()
                .get(&name)
                .and_then(|i| i.close.clone())
            else {
                drop(c);
                gone(notice, &name);
                return false;
            };
            c.ref_of(&e)
        };
        let input = CloseInput {
            date,
            account: name.clone(),
        };
        let op = Operation::Edit(close, EntryInput::Close(input));
        return run(
            backend,
            core,
            op,
            &format!("Saved {name}'s close date."),
            version,
            notice,
        );
    }
    true
}

/// Edits an account: its name (renamed everywhere), the open's date, currencies and budget flag,
/// and, when it is closed, the close date. Its booking method and other metadata are kept.
#[component]
fn OpenEditor(
    open: Entry,
    close: Option<Entry>,
    onsave: EventHandler<AccountEdit>,
    oncancel: EventHandler<()>,
) -> Element {
    let o = open.open().cloned().expect("an open entry");
    let flag = open.meta.str("budget").unwrap_or("").to_owned();
    let mut name = use_signal(|| o.account.clone());
    let mut date = use_signal(|| open.date.to_string());
    let mut currencies = use_signal(|| o.currencies.join(", "));
    let mut budget = use_signal(|| flag.clone());
    let liability = o.account.starts_with("Liabilities:");
    let was_loan = open.meta.str("kind") == Some("loan");
    let mut loan = use_signal(|| was_loan);
    let mut closed_on = use_signal(|| {
        close
            .as_ref()
            .map(|c| c.date.to_string())
            .unwrap_or_default()
    });
    rsx! {
        form { class: "editor", "aria-label": "Edit {o.account}",
            onsubmit: move |e: FormEvent| {
                e.prevent_default();
                let new_name = name.peek().trim().to_owned();
                let list: Vec<String> = currencies
                    .peek()
                    .split(',')
                    .map(|c| c.trim().to_owned())
                    .filter(|c| !c.is_empty())
                    .collect();
                let fields_changed = *date.peek() != open.date.to_string() || list != o.currencies || *budget.peek() != flag
                    || *loan.peek() != was_loan;
                let mut kept = meta(&open.meta);
                let on = *budget.peek() == "on";
                kept.retain(|(k, _)| {
                    k != "budget" && (!liability || k != "kind") && (on || k != "budget-from")
                });
                if liability {
                    kept.extend(kind_meta(if *loan.peek() { "loan" } else { "card" }));
                    // Switched on from today: earlier months keep their numbers (decision 7b).
                    if on && flag != "on" {
                        kept.push((
                            "budget-from".to_owned(),
                            MetaInputValue::Value(MetaValue::Date(crate::store::local_today())),
                        ));
                    }
                }
                let close_date = close
                    .as_ref()
                    .filter(|c| *closed_on.peek() != c.date.to_string())
                    .map(|_| closed_on.peek().clone());
                onsave.call(AccountEdit {
                    rename: (new_name != o.account).then(|| new_name.clone()),
                    open: fields_changed.then(|| OpenInput {
                        date: date.peek().clone(),
                        account: new_name.clone(),
                        currencies: list,
                        budget: BudgetFlag::parse(&budget.peek()),
                        booking: o.booking.clone(),
                        meta: kept,
                    }),
                    close_date,
                });
            },
            p { strong { "Edit {o.account}" } }
            div { class: "editor-head",
                label { "Name"
                    input { value: "{name}", required: true, oninput: move |e| name.set(e.value()) }
                }
                label { "Opened"
                    input { r#type: "date", value: "{date}", required: true, oninput: move |e| date.set(e.value()) }
                }
                label { "Currencies"
                    input { value: "{currencies}", placeholder: "USD", oninput: move |e| currencies.set(e.value()) }
                }
                label { "Budget"
                    select { value: "{budget}", onchange: move |e| budget.set(e.value()),
                        option { value: "", "category (expenses) / no flag" }
                        option { value: "on", "on budget (cash, card or loan)" }
                        option { value: "off", "off budget (tracking)" }
                        option { value: "excluded", "excluded expense" }
                        option { value: "placeholder", "placeholder (groups its sub-accounts)" }
                    }
                }
                if liability {
                    label { "This is a"
                        select { value: if loan() { "loan" } else { "card" }, onchange: move |e| loan.set(e.value() == "loan"),
                            option { value: "card", selected: !loan(), "Credit card" }
                            option { value: "loan", selected: loan(), "Loan or mortgage" }
                        }
                    }
                }
                if close.is_some() {
                    label { "Closed"
                        input { r#type: "date", value: "{closed_on}", required: true, oninput: move |e| closed_on.set(e.value()) }
                    }
                }
            }
            p { class: "muted small",
                "A new name changes it everywhere the ledger names it, sub-accounts too; it keeps its type (Assets stays Assets)."
            }
            div { class: "editor-actions",
                span {}
                button { r#type: "button", class: "link", onclick: move |_| oncancel.call(()), "Cancel" }
                button { r#type: "submit", "Save" }
            }
        }
    }
}

/// A statement balance as typed, in the ledger's sign: `sign` -1 (a liability, typed as owed) flips
/// it, since what is owed is negative in the ledger.
fn ledger_balance(typed: &str, sign: Decimal) -> String {
    let n = norm_amount(typed);
    if sign.is_sign_positive() {
        n
    } else {
        n.strip_prefix('-')
            .map_or_else(|| format!("-{n}"), str::to_owned)
    }
}

/// Reconciles an account with a bank statement (R1 to R3): its date and ending balance, then the
/// pending transactions it shows, ticked; once the difference is zero, one commit clears them and
/// writes a balance assertion dated the day after. `sign` is how the table shows the account:
/// -1 for liabilities, so a card's balance owed is typed positive, as the statement prints it.
#[component]
fn ReconcileForm(
    core: Shared,
    version: Signal<Option<String>>,
    account: String,
    today: Date,
    sign: Decimal,
    onsave: EventHandler<(Operation, String)>,
    oncancel: EventHandler<()>,
) -> Element {
    let _ = version();
    let mut date = use_today_date(today);
    let mut balance = use_signal(String::new);
    let mut ticked = use_signal(Vec::<String>::new);
    let d = date();
    let parsed = branchbooks_core::write::render::parse_date(&d).ok();
    let (cleared, pending) = if let Some(day) = parsed {
        let mut c = core.0.borrow_mut();
        let currency = c.book().currency().to_owned();
        let r = c.book().reconcile(&account, day, &currency);
        let pending: Vec<(Entry, Decimal)> =
            r.pending.iter().map(|(e, a)| ((*e).clone(), *a)).collect();
        (r.cleared, pending)
    } else {
        (Decimal::ZERO, Vec::new())
    };
    let key = |e: &Entry| format!("{}:{}", e.file, e.line);
    let is_ticked = |e: &Entry| ticked.read().contains(&key(e));
    let cleared = cleared
        + pending
            .iter()
            .filter(|(e, _)| is_ticked(e))
            .map(|(_, a)| *a)
            .sum::<Decimal>();
    let statement = Decimal::from_str(&norm_amount(&balance())).ok();
    let difference = statement.map(|s| s - cleared * sign);
    let left = pending.iter().filter(|(e, _)| !is_ticked(e)).count();
    let ok = difference == Some(Decimal::ZERO) && left == 0;
    let status = match difference {
        None => "Enter the ending balance on the statement.".to_owned(),
        Some(diff) if !diff.is_zero() => format!(
            "Off by {}.{}",
            fmt(diff),
            if pending.is_empty() {
                ""
            } else {
                " Tick the pending transactions the statement shows."
            }
        ),
        Some(_) if left > 0 => format!(
            "{left} pending transaction(s) not on the statement. Date them after {d} first: a balance assertion counts every transaction up to its date."
        ),
        Some(_) => "Matches the statement.".to_owned(),
    };
    let clear: Vec<_> = pending
        .iter()
        .filter(|(e, _)| is_ticked(e))
        .map(|(e, _)| core.0.borrow().ref_of(e))
        .collect();
    rsx! {
        form { class: "editor", "aria-label": "Reconcile {account}",
            onsubmit: move |e: FormEvent| {
                e.prevent_default();
                if !ok {
                    return;
                }
                let n = ledger_balance(&balance.peek(), sign);
                let input = ReconcileInput {
                    account: account.clone(),
                    date: date.peek().clone(),
                    balance: n,
                    currency: None,
                    clear: clear.clone(),
                };
                let done = format!("Reconciled {} to the {} statement.", short(&account), date.peek());
                onsave.call((Operation::Reconcile(input), done));
            },
            p { strong { "Reconcile {account}" } }
            div { class: "editor-head",
                label { "Statement date"
                    input { r#type: "date", value: "{date}", required: true, oninput: move |e| date.set(e.value()) }
                }
                label { "Ending balance"
                    input { class: "amount", value: "{balance}", inputmode: "decimal", required: true,
                        oninput: move |e| balance.set(e.value()),
                    }
                }
            }
            p { class: "muted small", "data-testid": "cleared", "Cleared balance on {d}: {fmt(cleared * sign)}" }
            if !pending.is_empty() {
                fieldset {
                    legend { class: "small", "Pending up to {d}" }
                    ul { class: "register",
                        for (e, amount) in pending.iter() {
                            li { key: "{key(e)}",
                                label {
                                    input { r#type: "checkbox", checked: is_ticked(e),
                                        onchange: {
                                            let k = key(e);
                                            move |ev: FormEvent| {
                                                let mut t = ticked.write();
                                                t.retain(|x| *x != k);
                                                if ev.checked() {
                                                    t.push(k.clone());
                                                }
                                            }
                                        },
                                    }
                                    " "
                                    span { class: "date", "{e.date}" }
                                    " {e.txn().map_or(\"\", |t| t.payee.as_deref().unwrap_or(&t.narration))} "
                                    span { class: "num", {fmt(*amount * sign)} }
                                }
                            }
                        }
                    }
                }
            }
            p { role: "status", class: if ok { "" } else { "muted" }, "{status}" }
            div { class: "editor-actions",
                button { r#type: "submit", disabled: !ok, "Reconcile" }
                button { r#type: "button", class: "link", onclick: move |_| oncancel.call(()), "Cancel" }
            }
        }
    }
}

/// What someone is adding, and where it goes in the ledger.
#[derive(Clone, Copy)]
struct Kind {
    id: &'static str,
    label: &'static str,
    root: &'static str,
    flag: Option<BudgetFlag>,
    /// What the budget does with it, in words.
    effect: &'static str,
    /// It holds money, so it can start with a balance.
    holds_money: bool,
}

/// The kinds offered. "other" picks the root and the budget flag by hand.
const KINDS: [Kind; 7] = [
    Kind {
        id: "cash",
        label: "Checking or cash",
        root: "Assets",
        flag: Some(BudgetFlag::On),
        effect: "on budget: its money is yours to assign",
        holds_money: true,
    },
    Kind {
        id: "card",
        label: "Credit card",
        root: "Liabilities",
        flag: Some(BudgetFlag::On),
        effect: "on budget: purchases set money aside for the payment",
        holds_money: true,
    },
    Kind {
        id: "track",
        label: "Savings or investments",
        root: "Assets",
        flag: Some(BudgetFlag::Off),
        effect: "off budget: tracked, not budgeted",
        holds_money: true,
    },
    Kind {
        id: "loan",
        label: "Loan or mortgage",
        root: "Liabilities",
        flag: Some(BudgetFlag::On),
        effect: "on budget: set aside its payment each month",
        holds_money: true,
    },
    Kind {
        id: "category",
        label: "Spending category",
        root: "Expenses",
        flag: None,
        effect: "a budget category",
        holds_money: false,
    },
    Kind {
        id: "income",
        label: "Income",
        root: "Income",
        flag: None,
        effect: "income: money that arrives is yours to assign",
        holds_money: false,
    },
    Kind {
        id: "other",
        label: "More…",
        root: "Expenses",
        flag: None,
        effect: "",
        holds_money: false,
    },
];

/// The liabilities marked `kind: "loan"`, so the budget shows them as loans. A liability with no
/// `kind:` is a credit card.
pub fn loans(book: &mut branchbooks_core::book::Book) -> Vec<String> {
    book.account_info()
        .iter()
        .filter(|(_, i)| i.open.meta.str("kind") == Some("loan"))
        .map(|(a, _)| a.clone())
        .collect()
}

/// `kind:` metadata for a liability's open: "loan" or "card", which the budget groups by.
fn kind_meta(kind: &str) -> Vec<(String, MetaInputValue)> {
    vec![("kind".to_owned(), MetaInputValue::Text(kind.to_owned()))]
}

/// Where opening balances come from, as `GnuCash` names it.
const OPENING: &str = "Equity:Opening-Balances";

/// The full ledger name for a kind's `root`, an optional parent typed as "Food" or "Food:Dining",
/// and a name typed in words; or what is wrong with it.
fn new_account_name(root: &str, parent: &str, name: &str) -> Result<String, &'static str> {
    let name = ledger_part(name);
    if name.is_empty() {
        return Err("Type a name.");
    }
    let mut parts = vec![root.to_owned()];
    parts.extend(parent.split(':').map(ledger_part).filter(|p| !p.is_empty()));
    parts.push(name);
    let full = parts.join(":");
    if is_account(&full) {
        Ok(full)
    } else {
        Err("Use letters, digits, spaces and hyphens.")
    }
}

/// Adds an account or category: a kind, a name in words, an optional parent, the date it opened, and
/// for an account its balance today, written against `OPENING`. Each is its own commit, in order.
#[component]
fn AddAccount(
    backend: Backend,
    core: Shared,
    version: Signal<Option<String>>,
    notice: Signal<Option<(bool, String)>>,
    today: Date,
    onclose: EventHandler<()>,
) -> Element {
    let mut kind = use_signal(|| "cash");
    let mut name = use_signal(String::new);
    let mut parent = use_signal(String::new);
    let mut balance = use_signal(String::new);
    let mut other_root = use_signal(|| "Expenses".to_owned());
    let mut other_flag = use_signal(String::new);
    let mut date = use_today_date(today);
    let mut error = use_signal(|| None::<String>);
    let Kind {
        id: kind_id,
        label,
        root,
        flag,
        effect,
        holds_money,
        ..
    } = KINDS
        .into_iter()
        .find(|k| k.id == kind())
        .unwrap_or(KINDS[0]);
    let other = kind() == "other";
    let root = if other { other_root() } else { root.to_owned() };
    let flag = if other {
        BudgetFlag::parse(&other_flag())
    } else {
        flag
    };
    let full = new_account_name(&root, &parent(), &name());
    let _ = version();
    let (existing, parents, currency) = {
        let mut c = core.0.borrow_mut();
        let book = c.book();
        let info = book.account_info();
        let under = format!("{root}:");
        // Every level of the names under this root, in words: "Food", "Food:Dining".
        let parents: std::collections::BTreeSet<String> = info
            .keys()
            .filter_map(|a| a.strip_prefix(&under))
            .flat_map(|rest| {
                let parts: Vec<&str> = rest.split(':').collect();
                (1..=parts.len())
                    .map(|i| {
                        parts[..i]
                            .iter()
                            .map(|p| words(p))
                            .collect::<Vec<_>>()
                            .join(":")
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        let existing = full.as_ref().is_ok_and(|f| info.contains_key(f.as_str()));
        (existing, parents, book.currency().to_owned())
    };
    let amount = balance().trim().to_owned();
    let amount_ok = amount.is_empty() || parse_amount(&amount).is_ok();
    let problem = match &full {
        Err(e) => Some((*e).to_owned()),
        Ok(f) if existing => Some(format!("{} already exists.", short(f))),
        Ok(_) if holds_money && !amount_ok => {
            Some(format!("{amount:?} is not an amount, such as 1,500.00."))
        }
        Ok(_) => None,
    };
    let add = {
        let (backend, core) = (backend.clone(), core.clone());
        let (full, root) = (full.clone(), root.clone());
        move |e: FormEvent| {
            e.prevent_default();
            let Ok(account) = full.clone() else { return };
            let opened_on = date.peek().clone();
            let open = OpenInput {
                date: opened_on.clone(),
                account: account.clone(),
                budget: flag,
                meta: if ["loan", "card"].contains(&kind_id) {
                    kind_meta(kind_id)
                } else {
                    Vec::new()
                },
                ..Default::default()
            };
            let typed = balance.peek().trim().to_owned();
            let sign = if root == "Liabilities" {
                Decimal::NEGATIVE_ONE
            } else {
                Decimal::ONE
            };
            let opening = (holds_money
                && !typed.is_empty()
                && parse_amount(&typed).is_ok_and(|d| !d.is_zero()))
            .then(|| ledger_balance(&typed, sign));
            match add_account(&backend, &core, open, opening, &currency) {
                Ok(done) => {
                    notice.clone().set(Some((true, done)));
                    error.set(None);
                    name.set(String::new());
                    balance.set(String::new());
                    onclose.call(());
                }
                Err(e) => error.set(Some(e)),
            }
            let v = core.0.borrow().version.clone();
            version.clone().set(Some(v));
        }
    };
    rsx! {
        form { class: "add-account", "aria-label": "Add an account or category", onsubmit: add,
            div { class: "kinds", role: "radiogroup", "aria-label": "Kind",
                for Kind { id, label, .. } in KINDS {
                    label { key: "{id}",
                        input { r#type: "radio", name: "kind", value: id, checked: kind() == id,
                            onchange: move |_| kind.set(id),
                        }
                        {label}
                    }
                }
            }
            div { class: "fields",
                if other {
                    label { "Type"
                        select { value: "{other_root}", onchange: move |e| other_root.set(e.value()),
                            for (r, _, _) in ROOTS { option { key: "{r}", value: r, {r} } }
                        }
                    }
                    label { "Budget"
                        select { value: "{other_flag}", onchange: move |e| other_flag.set(e.value()),
                            option { value: "", "category / no flag" }
                            option { value: "on", "on budget" }
                            option { value: "off", "off budget" }
                            option { value: "excluded", "excluded expense" }
                            option { value: "placeholder", "placeholder (groups its sub-accounts)" }
                        }
                    }
                }
                label { "Name"
                    input { value: "{name}", required: true, autocomplete: "off",
                        placeholder: if root == "Liabilities" { "Chase Visa" } else if root == "Expenses" { "Dining" } else { "Checking" },
                        oninput: move |e| name.set(e.value()),
                    }
                }
                label { "Inside (optional)"
                    input { value: "{parent}", list: "account-parents", placeholder: "none", autocomplete: "off",
                        oninput: move |e| parent.set(e.value()),
                    }
                }
                label { "Opened"
                    input { r#type: "date", value: "{date}", required: true, oninput: move |e| date.set(e.value()) }
                }
                if holds_money {
                    label { if root == "Liabilities" { "Owed today" } else { "Balance today" }
                        input { value: "{balance}", inputmode: "decimal", placeholder: "0.00", autocomplete: "off",
                            oninput: move |e| balance.set(e.value()),
                        }
                    }
                }
            }
            datalist { id: "account-parents",
                for p in parents { option { key: "{p}", value: "{p}" } }
            }
            p { class: "preview", "aria-live": "polite",
                match (&full, &problem) {
                    (Ok(f), None) => rsx! {
                        span { "Saved as " code { "{f}" } }
                        span { class: "muted",
                            if other { {flag.map_or("no budget flag", BudgetFlag::as_str)} } else { {effect} }
                        }
                    },
                    (_, Some(p)) if !name().trim().is_empty() => rsx! { span { class: "field-error", "{p}" } },
                    _ => rsx! { span { class: "muted", "Adding: {label}" } },
                }
            }
            if let Some(e) = error() {
                p { class: "field-error", role: "alert", "{e}" }
            }
            div { class: "editor-actions",
                button { r#type: "submit", disabled: problem.is_some(), "Add" }
                button { r#type: "button", class: "link", onclick: move |_| onclose.call(()), "Cancel" }
            }
        }
    }
}

/// Opens `open`'s account and, given `opening` (ledger signed), writes its balance against
/// `OPENING`, opening that too the first time. What was done, or why it stopped; an account
/// already opened stays when its balance is refused.
fn add_account(
    backend: &Backend,
    core: &Shared,
    open: OpenInput,
    opening: Option<String>,
    currency: &str,
) -> Result<String, String> {
    let account = open.account.clone();
    let date = open.date.clone();
    let save = |op| apply(backend, &core.0, op).map_err(|e| e.to_string());
    save(Operation::Add(EntryInput::Open(open)))?;
    let Some(amount) = opening else {
        return Ok(format!("Added {}.", short(&account)));
    };
    let stop = |e: String| format!("Added {}, but not its balance: {e}", short(&account));
    let has_opening = core
        .0
        .borrow_mut()
        .book()
        .account_info()
        .contains_key(OPENING);
    if !has_opening {
        let equity = OpenInput {
            date: date.clone(),
            account: OPENING.to_owned(),
            ..Default::default()
        };
        save(Operation::Add(EntryInput::Open(equity))).map_err(stop)?;
    }
    let posting = |account: &str, amount: Option<String>| PostingInput {
        account: account.to_owned(),
        currency: amount.as_ref().map(|_| currency.to_owned()),
        amount,
        ..Default::default()
    };
    let txn = TransactionInput {
        date,
        payee: Some("Opening balance".to_owned()),
        narration: words(account.rsplit(':').next().unwrap_or(&account)),
        postings: vec![posting(&account, Some(amount)), posting(OPENING, None)],
        ..Default::default()
    };
    save(Operation::Add(EntryInput::Txn(txn))).map_err(stop)?;
    Ok(format!("Added {} with its balance.", short(&account)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_name_is_typed_in_words() {
        assert_eq!(
            new_account_name("Liabilities", "", "chase visa"),
            Ok("Liabilities:Chase-Visa".into())
        );
        assert_eq!(
            new_account_name("Expenses", " food : dining ", "Take out"),
            Ok("Expenses:Food:Dining:Take-Out".into())
        );
        assert_eq!(new_account_name("Assets", "", "  "), Err("Type a name."));
        assert_eq!(
            new_account_name("Assets", "", "Bank & Co"),
            Err("Use letters, digits, spaces and hyphens.")
        );
    }

    #[test]
    fn a_card_opens_with_what_it_owes() {
        use branchbooks_core::write::store::{CommitInfo, FileChange};
        use branchbooks_core::write::workspace::WorkspaceCore;
        let dir =
            std::env::temp_dir().join(format!("branchbooks-add-account-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        branchbooks_git::GitStore::init(&dir, None).unwrap();
        let backend = Backend {
            dir: dir.clone(),
            author: branchbooks_git::Author {
                name: "test".into(),
                email: "test@example.invalid".into(),
            },
            scenario: None,
        };
        let ledger = "option \"operating_currency\" \"USD\"\n2026-01-01 open Assets:Checking USD\n  budget: \"on\"\n";
        let seed = FileChange {
            path: "main.beancount".into(),
            text: Some(ledger.into()),
        };
        let info = CommitInfo {
            op_id: "seed".into(),
            op_type: "import".into(),
            summary: "Seed".into(),
        };
        backend.commit("", &[seed], &info).unwrap();
        let snap = backend.snapshot().unwrap();
        let mut core = WorkspaceCore::new(None);
        core.reset(&snap.version, snap.files);
        let core = Shared(std::rc::Rc::new(std::cell::RefCell::new(core)));
        let open = OpenInput {
            date: "2026-11-01".into(),
            account: "Liabilities:Chase-Visa".into(),
            budget: Some(BudgetFlag::On),
            ..Default::default()
        };
        let owed = ledger_balance("500", Decimal::NEGATIVE_ONE);
        assert_eq!(
            add_account(&backend, &core, open, Some(owed), "USD"),
            Ok("Added Chase-Visa with its balance.".into())
        );
        let mut c = core.0.borrow_mut();
        let book = c.book();
        let balances = book.balances().clone();
        assert_eq!(
            branchbooks_core::ledger::balance_of(&balances, "Liabilities:Chase-Visa", "USD"),
            Decimal::from(-500)
        );
        assert_eq!(
            branchbooks_core::ledger::balance_of(&balances, OPENING, "USD"),
            Decimal::from(500)
        );
        assert!(book.errors().is_empty(), "{:?}", book.errors());
        drop(c);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_statement_balance_takes_the_ledgers_sign() {
        assert_eq!(ledger_balance("1,234.5", Decimal::ONE), "1234.50");
        // A card statement's balance owed is typed positive; the ledger has it negative.
        assert_eq!(ledger_balance("45", Decimal::NEGATIVE_ONE), "-45.00");
        assert_eq!(ledger_balance("-10", Decimal::NEGATIVE_ONE), "10.00");
    }
}

#[cfg(test)]
mod tree_tests {
    use super::*;

    #[test]
    fn accounts_make_a_tree_with_totals_like_gnucash() {
        let d = Decimal::from;
        let t = tree(&[
            ("Expenses:Food:Dining", d(10)),
            ("Expenses:Food", d(5)),
            ("Expenses:Food:Groceries", d(20)),
            ("Expenses:Food-Extra", d(1)),
            ("Expenses:Home:Repair-And-Maintenance", d(7)),
        ]);
        let seen: Vec<(&str, usize, Decimal, bool, bool)> = t
            .iter()
            .map(|n| (n.label.as_str(), n.depth, n.total, n.parent, n.account))
            .collect();
        assert_eq!(
            seen,
            [
                ("Food", 0, d(35), true, true),
                ("Dining", 1, d(10), false, true),
                ("Groceries", 1, d(20), false, true),
                ("Food Extra", 0, d(1), false, true),
                ("Home", 0, d(7), true, false),
                ("Repair And Maintenance", 1, d(7), false, true),
            ]
        );
    }
}
