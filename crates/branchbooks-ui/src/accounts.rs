//! Accounts: a form to open an account or category, then every one grouped by type with its balance;
//! its name opens its transactions; Edit (date, currencies, budget flag), Reconcile (R1 to R3),
//! Close (on a date), Reopen and Delete.

use std::str::FromStr;

use branchbooks_core::date::Date;
use branchbooks_core::types::Entry;
use branchbooks_core::write::render::{BudgetFlag, CloseInput, EntryInput, OpenInput};
use branchbooks_core::write::workspace::{Operation, ReconcileInput};
use dioxus::prelude::*;
use rust_decimal::Decimal;

use crate::journal::{Shared, meta, run};
use crate::store::Backend;
use crate::text::{fmt, norm_amount, short, words};

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
    rsx! {
        OpenAccount { today: today, onopen: {
            let save = save.clone();
            move |input: OpenInput| {
                let done = format!("Opened {}.", input.account);
                save(Operation::Add(EntryInput::Open(input)), done);
            }
        }}
        section { "aria-label": "Accounts",
            div { class: "section-head",
                h2 { "Accounts and categories" }
                if closed > 0 {
                    button { r#type: "button", class: "link", onclick: move |_| show_closed.toggle(),
                        if show_closed() { "Hide closed" } else { "Show closed ({closed})" }
                    }
                }
            }
            table { class: "accounts",
                thead { tr { th { "Name" } th { class: "num", "Balance" } th { "Budget" } th { "Opened" } th {} } }
                for (root, heading, credit) in ROOTS {
                    {
                        let group: Vec<&Row> = rows
                            .iter()
                            .filter(|r| r.account.split(':').next() == Some(root))
                            .filter(|r| show_closed() || r.close.is_none())
                            .collect();
                        let sign = if credit { Decimal::NEGATIVE_ONE } else { Decimal::ONE };
                        let total: Decimal = group.iter().map(|r| r.balance).sum::<Decimal>() * sign;
                        rsx! {
                            if !group.is_empty() {
                                tbody { key: "{root}", class: "group",
                                    tr { class: "group-head",
                                        th { scope: "rowgroup", "{heading}" }
                                        td { class: "num", {fmt(total)} }
                                        td { colspan: 3 }
                                    }
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
                                                td { colspan: 3 }
                                            }
                                        } else if let Some(r) = group.iter().find(|r| r.account == node.path).copied() {
                                        if mode_of(&r.account) == Some("edit") {
                                            tr { key: "{r.account}", class: "editing",
                                                td { colspan: 5,
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
                                                td { colspan: 5,
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
                                            tr { key: "{r.account}", "data-testid": "account", class: if r.close.is_some() { "closed" } else { "" },
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
                                                }
                                                td { class: "num", title: if node.parent { "With its sub-accounts" } else { "" }, {fmt(node.total * sign)} }
                                                td {
                                                    match r.open.meta.str("budget") {
                                                        Some("on") => "on budget",
                                                        Some("off") => "off budget",
                                                        Some("excluded") => "excluded",
                                                        _ if root == "Expenses" => "category",
                                                        _ => "",
                                                    }
                                                }
                                                td { class: "date", "{r.open.date}" }
                                                td { class: "actions",
                                                    Actions { account: r.account.clone(), today: today,
                                                        closed: r.close.is_some(),
                                                        // Statements come for asset and liability accounts.
                                                        reconcilable: root == "Assets" || root == "Liabilities",
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
                                                            let (save, a) = (save.clone(), r.account.clone());
                                                            let close = r.close.as_ref().map(&at);
                                                            move |()| if let Some(c) = close.clone() {
                                                                save(Operation::Delete(c), format!("Reopened {a}."));
                                                            }
                                                        },
                                                        ondelete: {
                                                            let (save, a, open) = (save.clone(), r.account.clone(), at(&r.open));
                                                            move |()| save(Operation::Delete(open.clone()), format!("Deleted {a}."))
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
            p { class: "muted small",
                "Deleting an account removes its open entry; it is refused while transactions or envelope entries use it. "
                "Close it instead to keep its history."
            }
        }
    }
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
    mode: Option<&'static str>,
    onmode: EventHandler<Option<&'static str>>,
    onclose: EventHandler<String>,
    onreopen: EventHandler<()>,
    ondelete: EventHandler<()>,
) -> Element {
    let mut date = use_signal(|| today.to_string());
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
                button { r#type: "button", class: "link", "aria-label": "Close {account}",
                    onclick: move |_| onmode.call(Some("close")), "Close"
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
                let fields_changed = *date.peek() != open.date.to_string() || list != o.currencies || *budget.peek() != flag;
                let mut kept = meta(&open.meta);
                kept.retain(|(k, _)| k != "budget");
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
                        option { value: "on", "on budget (cash or card)" }
                        option { value: "off", "off budget (tracking)" }
                        option { value: "excluded", "excluded expense" }
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
    let mut date = use_signal(|| today.to_string());
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

/// Opens an account or a category: its name, budget flag and date.
#[component]
fn OpenAccount(today: Date, onopen: EventHandler<OpenInput>) -> Element {
    let mut account = use_signal(String::new);
    let mut budget = use_signal(String::new);
    let mut date = use_signal(|| today.to_string());
    rsx! {
        section {
            form { class: "inline-form", "aria-label": "Open account",
                onsubmit: move |e: FormEvent| {
                    e.prevent_default();
                    onopen.call(OpenInput {
                        date: date.peek().clone(),
                        account: account.peek().trim().to_owned(),
                        budget: BudgetFlag::parse(&budget.peek()),
                        ..Default::default()
                    });
                    account.set(String::new());
                },
                h2 { "Open an account or category" }
                label { "Name"
                    input { value: "{account}", placeholder: "Expenses:Food:Dining", required: true,
                        oninput: move |e| account.set(e.value()),
                    }
                }
                label { "Budget"
                    select { value: "{budget}", onchange: move |e| budget.set(e.value()),
                        option { value: "", "category (expenses) / no flag" }
                        option { value: "on", "on budget (cash or card)" }
                        option { value: "off", "off budget (tracking)" }
                        option { value: "excluded", "excluded expense" }
                    }
                }
                label { "Opened"
                    input { r#type: "date", value: "{date}", required: true, oninput: move |e| date.set(e.value()) }
                }
                button { r#type: "submit", "Open" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
