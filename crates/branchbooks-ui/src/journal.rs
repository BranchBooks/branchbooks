//! The Journal: a row to add a transaction, then
//! the month's transactions, newest first, each with Edit (date, payee, description and every
//! posting) and Delete.

use std::cell::RefCell;
use std::rc::Rc;

use branchbooks_core::date::{Date, Month};
use branchbooks_core::types::{Amount, Body, Entry, Meta, Posting, Transaction};
use branchbooks_core::write::render::{EntryInput, MetaInputValue, PostingInput, TransactionInput};
use branchbooks_core::write::workspace::{Operation, WorkspaceCore};
use dioxus::prelude::*;
use rust_decimal::Decimal;

use crate::budget::apply;
use crate::store::Backend;
use crate::text::{fmt, norm_amount, short};

/// The book being shown, shared by the views: equal when it is the same one.
#[derive(Clone)]
pub struct Shared(pub Rc<RefCell<WorkspaceCore>>);

impl PartialEq for Shared {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

/// Commits `op` and says how it went in `notice`; true when it was saved.
pub(crate) fn run(
    backend: Backend,
    core: Shared,
    op: Operation,
    done: &str,
    mut version: Signal<Option<String>>,
    mut notice: Signal<Option<(bool, String)>>,
) -> bool {
    let saved = match apply(&backend, &core.0, op) {
        Ok(w) if w.is_empty() => {
            notice.set(Some((true, done.to_owned())));
            true
        }
        Ok(w) => {
            let w: Vec<&str> = w.iter().map(|w| w.message.as_str()).collect();
            notice.set(Some((false, format!("Saved, but: {}", w.join("; ")))));
            true
        }
        Err(e) => {
            notice.set(Some((false, e.to_string())));
            false
        }
    };
    let v = core.0.borrow().version.clone();
    version.set(Some(v));
    saved
}

#[component]
pub fn Journal(
    backend: Backend,
    core: Shared,
    version: Signal<Option<String>>,
    notice: Signal<Option<(bool, String)>>,
    month: Month,
    today: Date,
    /// Only the transactions that touch this account (from Accounts), with "Show all".
    filter: Option<String>,
    onclear: EventHandler<()>,
    /// The add row's last account, kept by the screen.
    last_account: Signal<String>,
) -> Element {
    let mut editing = use_signal(|| None::<String>);
    let mut confirming = use_signal(|| None::<String>);
    // Read, so a new version redraws the list.
    let _ = version();
    let rows = core.0.borrow_mut().transaction_rows(
        Some(month.first_day()),
        Some(month.last_day()),
        filter.as_deref(),
    );
    // The filtered account's change this month, as its booked postings add up.
    let total: Option<Decimal> = filter.as_ref().map(|a| {
        rows.iter()
            .filter_map(|(_, b)| b.txn())
            .flat_map(|t| &t.postings)
            .filter(|p| p.account == *a)
            .filter_map(|p| p.units.as_ref().map(|u| u.number))
            .sum()
    });
    let key = |e: &Entry| format!("{}:{}", e.file, e.line);
    rsx! {
        AddTransaction { backend: backend.clone(), core: core.clone(), version, notice, today: today, last_account }
        section { id: "transactions", "aria-label": "Transactions",
            div { class: "section-head",
                h2 {
                    "Transactions"
                    if let Some(a) = &filter { " in {short(a)}" }
                }
                if filter.is_some() {
                    button { r#type: "button", class: "link", onclick: move |_| onclear.call(()), "Show all" }
                }
            }
            if let Some(t) = total {
                p { class: "muted small", "data-testid": "filter-total", "Change this month: {fmt(t)}" }
            }
            if rows.is_empty() {
                p { class: "muted", "No transactions this month." }
            }
            ul { class: "register",
                for (entry, booked) in rows {
                    if editing() == Some(key(&entry)) {
                        li { key: "{key(&entry)}", class: "editing",
                            TransactionEditor { core: core.clone(), entry: entry.clone(),
                                oncancel: move |()| editing.set(None),
                                onsave: {
                                    let (backend, core, entry) = (backend.clone(), core.clone(), entry.clone());
                                    move |input: EntryInput| {
                                        let (backend, core) = (backend.clone(), core.clone());
                                        // Worked out on the click: it reads the whole file.
                                        let at = core.0.borrow().ref_of(&entry);
                                        let op = Operation::Edit(at, input);
                                        if run(backend, core, op, "Transaction saved.", version, notice) {
                                            editing.set(None);
                                        }
                                    }
                                },
                            }
                        }
                    } else {
                        Line { key: "{key(&entry)}", entry: entry.clone(), booked, today: today, hit: filter.clone(),
                            confirming: confirming() == Some(key(&entry)),
                            onedit: {
                                let k = key(&entry);
                                move |()| {
                                    confirming.set(None);
                                    editing.set(Some(k.clone()));
                                }
                            },
                            onask: {
                                let k = key(&entry);
                                move |ask: bool| {
                                    editing.set(None);
                                    confirming.set(ask.then(|| k.clone()));
                                }
                            },
                            ondelete: {
                                let (backend, core, entry) = (backend.clone(), core.clone(), entry.clone());
                                move |()| {
                                    let (backend, core) = (backend.clone(), core.clone());
                                    let at = core.0.borrow().ref_of(&entry);
                                    let op = Operation::Delete(at);
                                    if run(backend, core, op, "Transaction deleted.", version, notice) {
                                        confirming.set(None);
                                    }
                                }
                            },
                        }
                    }
                }
            }
        }
    }
}

/// One transaction: date, payee and description, its postings as booked, Edit and Delete.
#[component]
fn Line(
    entry: Entry,
    booked: Entry,
    /// The filtered account: its postings are highlighted.
    hit: Option<String>,
    today: Date,
    confirming: bool,
    onedit: EventHandler<()>,
    onask: EventHandler<bool>,
    ondelete: EventHandler<()>,
) -> Element {
    let t = entry.txn().expect("a transaction row");
    let label = format!(
        "{} {}",
        entry.date,
        t.payee.as_deref().unwrap_or(&t.narration)
    );
    let postings = booked.txn().map(|b| b.postings.clone()).unwrap_or_default();
    rsx! {
        li { "data-testid": "txn",
            div { class: "txn-head",
                span { class: "date", "{entry.date}" }
                span { class: "desc",
                    if let Some(p) = &t.payee {
                        strong { "{p}" }
                        if !t.narration.is_empty() && t.narration != *p { " · {t.narration}" }
                    } else {
                        strong { "{t.narration}" }
                    }
                }
                if entry.date > today { span { class: "badge", title: "Dated after today", "upcoming" } }
                if t.flag == "!" { span { class: "badge", title: "Flagged !", "flagged" } }
                span { class: "actions",
                    if confirming {
                        span { class: "muted small", "Delete this transaction?" }
                        button { r#type: "button", class: "danger", onclick: move |_| ondelete.call(()), "Delete" }
                        button { r#type: "button", class: "link", onclick: move |_| onask.call(false), "Keep" }
                    } else {
                        button { r#type: "button", class: "link", "aria-label": "Edit {label}",
                            onclick: move |_| onedit.call(()), "Edit"
                        }
                        button { r#type: "button", class: "link", "aria-label": "Delete {label}",
                            onclick: move |_| onask.call(true), "Delete"
                        }
                    }
                }
            }
            ul { class: "postings",
                for p in postings {
                    li { class: if hit.as_deref() == Some(p.account.as_str()) { "hit" } else { "" },
                        span { {short(&p.account)} }
                        span { class: "num", {p.units.as_ref().map(|u| fmt(u.number)).unwrap_or_default()} }
                    }
                }
            }
        }
    }
}

/// Edits any transaction: date, payee, description and every posting. One amount may be left
/// blank for the ledger to fill in. Flag, tags, links and metadata (such as a transfer's
/// `category:`) are kept as they were.
#[component]
fn TransactionEditor(
    core: Shared,
    entry: Entry,
    onsave: EventHandler<EntryInput>,
    oncancel: EventHandler<()>,
) -> Element {
    let t = entry.txn().expect("a transaction").clone();
    // Not placeholders, unless this transaction already posts to one, so it can still be shown.
    let accounts: Vec<String> = core
        .0
        .borrow_mut()
        .book()
        .account_info()
        .iter()
        .filter(|(a, i)| {
            i.open.meta.str("budget") != Some("placeholder")
                || t.postings.iter().any(|p| p.account == **a)
        })
        .map(|(a, _)| a.clone())
        .collect();
    let mut date = use_signal(|| entry.date.to_string());
    let mut payee = use_signal(|| t.payee.clone().unwrap_or_default());
    let mut narration = use_signal(|| t.narration.clone());
    // (account, amount, the posting it came from, for its currency and metadata)
    let mut rows = use_signal(|| {
        t.postings
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let amount = p
                    .units
                    .as_ref()
                    .map(|u| u.number.to_string())
                    .unwrap_or_default();
                (p.account.clone(), amount, Some(i))
            })
            .collect::<Vec<_>>()
    });
    let count = rows.read().len();
    rsx! {
        form { class: "editor", "aria-label": "Edit transaction",
            onsubmit: move |e: FormEvent| {
                e.prevent_default();
                let payee = payee.peek().trim().to_owned();
                let narration = narration.peek().trim().to_owned();
                let postings = rows
                    .peek()
                    .iter()
                    .filter(|(account, _, _)| !account.trim().is_empty())
                    .map(|(account, amount, from)| {
                        let from = from.map(|i| &t.postings[i]);
                        let amount = Some(amount.trim()).filter(|a| !a.is_empty()).map(norm_amount);
                        PostingInput {
                            account: account.trim().to_owned(),
                            currency: amount
                                .as_ref()
                                .and(from.and_then(|p| p.units.as_ref()).map(|u| u.currency.clone())),
                            amount,
                            meta: from.map(|p| meta(&p.meta)).unwrap_or_default(),
                        }
                    })
                    .collect();
                onsave.call(EntryInput::Txn(TransactionInput {
                    date: date.peek().clone(),
                    flag: Some(if t.flag == "!" { "!" } else { "*" }.to_owned()),
                    narration: if narration.is_empty() { payee.clone() } else { narration },
                    payee: Some(payee).filter(|p| !p.is_empty()),
                    tags: t.tags.clone(),
                    links: t.links.clone(),
                    meta: meta(&entry.meta),
                    postings,
                }));
            },
            div { class: "editor-head",
                label { "Date"
                    input { r#type: "date", value: "{date}", required: true, oninput: move |e| date.set(e.value()) }
                }
                label { "Payee"
                    input { value: "{payee}", oninput: move |e| payee.set(e.value()) }
                }
                label { "Description"
                    input { value: "{narration}", oninput: move |e| narration.set(e.value()) }
                }
            }
            datalist { id: "all-accounts",
                for a in accounts { option { key: "{a}", value: "{a}" } }
            }
            table { class: "posting-rows",
                thead { tr { th { "Account" } th { class: "num", "Amount" } th {} } }
                tbody {
                    for (i, (account, amount, _)) in rows().into_iter().enumerate() {
                        tr { key: "{i}",
                            td {
                                input { list: "all-accounts", "aria-label": "Posting {i + 1} account", value: "{account}",
                                    oninput: move |e| rows.write()[i].0 = e.value(),
                                }
                            }
                            td { class: "num",
                                input { class: "amount", "aria-label": "Posting {i + 1} amount", inputmode: "decimal",
                                    placeholder: "auto", value: "{amount}",
                                    oninput: move |e| rows.write()[i].1 = e.value(),
                                }
                            }
                            td {
                                if count > 2 {
                                    button { r#type: "button", class: "link", "aria-label": "Remove posting {i + 1}",
                                        onclick: move |_| { rows.write().remove(i); }, "Remove"
                                    }
                                }
                            }
                        }
                    }
                }
            }
            p { class: "muted small", "Amounts must add up to zero. Leave one blank to have it filled in." }
            div { class: "editor-actions",
                button { r#type: "button", class: "link",
                    onclick: move |_| rows.write().push((String::new(), String::new(), None)), "Add posting"
                }
                span {}
                button { r#type: "button", class: "link", onclick: move |_| oncancel.call(()), "Cancel" }
                button { r#type: "submit", "Save" }
            }
        }
    }
}

/// Metadata as it was, to write back unchanged.
pub(crate) fn meta(m: &branchbooks_core::types::Meta) -> Vec<(String, MetaInputValue)> {
    m.iter()
        .map(|(k, v)| (k.to_owned(), MetaInputValue::Value(v.clone())))
        .collect()
}

/// The accounts the add row offers: categories first, then income, then other accounts (a
/// transfer); and the accounts money comes from, on-budget first. Closed accounts and placeholders
/// are left out.
fn choices(core: &mut WorkspaceCore) -> (Vec<String>, Vec<String>) {
    let info = core.book().account_info();
    let on = |a: &str| info[a].open.meta.str("budget") == Some("on");
    let mut open: Vec<&str> = info
        .iter()
        .filter(|(_, i)| i.close.is_none() && i.open.meta.str("budget") != Some("placeholder"))
        .map(|(a, _)| a.as_str())
        .collect();
    open.sort_unstable();
    let rank = |a: &str| match a.split(':').next() {
        Some("Expenses") => 0,
        Some("Income") => 1,
        _ => 2,
    };
    let mut targets: Vec<&str> = open.clone();
    targets.sort_by_key(|a| rank(a));
    let mut funding: Vec<&str> = open
        .into_iter()
        .filter(|a| a.starts_with("Assets:") || a.starts_with("Liabilities:"))
        .collect();
    funding.sort_by_key(|a| !on(a));
    let own = |v: Vec<&str>| v.into_iter().map(str::to_owned).collect();
    (own(targets), own(funding))
}

/// The quick entry row: date, payee, category, account, memo, and an outflow or an inflow. Split
/// opens the full editor with what is filled in so far, for a transaction with several postings.
#[component]
fn AddTransaction(
    backend: Backend,
    core: Shared,
    version: Signal<Option<String>>,
    notice: Signal<Option<(bool, String)>>,
    today: Date,
    mut last_account: Signal<String>,
) -> Element {
    let _ = version();
    let (targets, funding) = choices(&mut core.0.borrow_mut());
    let payees = core.0.borrow_mut().payees();
    let mut date = use_signal(|| today.to_string());
    let mut payee = use_signal(String::new);
    let mut category = use_signal(String::new);
    let mut account = use_signal(|| {
        let last = last_account.peek().clone();
        if funding.contains(&last) {
            last
        } else {
            funding.first().cloned().unwrap_or_default()
        }
    });
    let mut memo = use_signal(String::new);
    let mut outflow = use_signal(String::new);
    let mut inflow = use_signal(String::new);
    let mut problem = use_signal(String::new);
    let mut splitting = use_signal(|| None::<Entry>);
    let off_budget = {
        let core = core.clone();
        move |a: &str| {
            let mut c = core.0.borrow_mut();
            c.book().account_info()[a].open.meta.str("budget") != Some("on")
        }
    };
    if let Some(entry) = splitting() {
        return rsx! {
            section { "aria-label": "New transaction",
                TransactionEditor { core: core.clone(), entry,
                    oncancel: move |()| splitting.set(None),
                    onsave: move |input: EntryInput| {
                        let (backend, core) = (backend.clone(), core.clone());
                        let op = Operation::Add(input);
                        if run(backend, core, op, "Transaction added.", version, notice) {
                            last_account.set(account.peek().clone());
                            splitting.set(None);
                            for mut s in [payee, category, memo, outflow, inflow] {
                                s.set(String::new());
                            }
                        }
                    },
                }
            }
        };
    }
    rsx! {
        section { "aria-label": "New transaction",
            form { class: "quick-add", "aria-label": "Add transaction",
                onsubmit: {
                    let (targets, funding, backend, core) = (targets.clone(), funding.clone(), backend.clone(), core.clone());
                    move |e: FormEvent| {
                        e.prevent_default();
                        let entry = match quick_entry(
                            &targets, &funding, &date.peek(), &payee.peek(), &category.peek(),
                            &account.peek(), &memo.peek(), &outflow.peek(), &inflow.peek(),
                        ) {
                            Ok(entry) => entry,
                            Err(p) => return problem.set(p.to_owned()),
                        };
                        problem.set(String::new());
                        let (backend, core) = (backend.clone(), core.clone());
                        let op = Operation::Add(entry);
                        if run(backend, core, op, "Transaction added.", version, notice) {
                            last_account.set(account.peek().clone());
                            for mut s in [payee, category, memo, outflow, inflow] {
                                s.set(String::new());
                            }
                        }
                    }
                },
                datalist { id: "payees",
                    for p in payees { option { key: "{p}", value: "{p}" } }
                }
                datalist { id: "targets",
                    for a in targets.iter() { option { key: "{a}", value: "{a}" } }
                }
                label { class: "f-date", "Date"
                    input { r#type: "date", value: "{date}", required: true, oninput: move |e| date.set(e.value()) }
                }
                label { class: "f-payee", "Payee"
                    input { list: "payees", value: "{payee}", autocomplete: "off", oninput: move |e| payee.set(e.value()) }
                }
                label { class: "f-category", "Category"
                    input { list: "targets", value: "{category}", required: true, autocomplete: "off",
                        placeholder: "Type to search", oninput: move |e| category.set(e.value()),
                    }
                }
                label { class: "f-account", "Account"
                    select { value: "{account}", onchange: move |e| account.set(e.value()),
                        for a in funding.iter() {
                            option { key: "{a}", value: "{a}", selected: *a == account(),
                                {short(a)}
                                if off_budget(a) { " (off budget)" }
                            }
                        }
                    }
                }
                label { class: "f-memo", "Memo"
                    input { value: "{memo}", placeholder: "optional", oninput: move |e| memo.set(e.value()) }
                }
                label { class: "f-out", "Outflow"
                    input { class: "amount", value: "{outflow}", inputmode: "decimal", oninput: move |e| outflow.set(e.value()) }
                }
                label { class: "f-in", "Inflow"
                    input { class: "amount", value: "{inflow}", inputmode: "decimal", oninput: move |e| inflow.set(e.value()) }
                }
                button { r#type: "submit", class: "f-add", "Add" }
                button { r#type: "button", class: "link", title: "Several categories or accounts in one transaction",
                    onclick: {
                        let core = core.clone();
                        move |_| {
                            let currency = core.0.borrow_mut().book().currency().to_owned();
                            splitting.set(Some(split_entry(
                                date.peek().parse().unwrap_or(today), &payee.peek(), &memo.peek(), &category.peek(),
                                &account.peek(), &outflow.peek(), &inflow.peek(), &currency,
                            )));
                        }
                    },
                    "Split"
                }
            }
            if !problem().is_empty() {
                p { class: "field-error", role: "alert", "{problem}" }
            }
        }
    }
}

/// The quick row's transaction, or what is missing from it.
#[allow(clippy::too_many_arguments)]
fn quick_entry(
    targets: &[String],
    funding: &[String],
    date: &str,
    payee: &str,
    category: &str,
    account: &str,
    memo: &str,
    outflow: &str,
    inflow: &str,
) -> Result<EntryInput, &'static str> {
    if !targets.iter().any(|t| t == category) {
        return Err("Pick a category or account from the list.");
    }
    if !funding.iter().any(|f| f == account) {
        return Err("Pick the account the money came from or went to.");
    }
    let out = outflow.trim().trim_start_matches('-');
    let inn = inflow.trim().trim_start_matches('-');
    if out.is_empty() == inn.is_empty() {
        return Err("Enter an outflow or an inflow.");
    }
    let n = norm_amount(if out.is_empty() { inn } else { out });
    let (p, m) = (payee.trim(), memo.trim());
    // Beancount's payee and narration: both only when there is a memo, else one string.
    let narration = [m, p, short(category)]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or_default();
    Ok(EntryInput::Txn(TransactionInput {
        date: date.to_owned(),
        payee: (!p.is_empty() && !m.is_empty()).then(|| p.to_owned()),
        narration: narration.to_owned(),
        postings: vec![
            PostingInput {
                account: category.to_owned(),
                amount: Some(if out.is_empty() { format!("-{n}") } else { n }),
                ..Default::default()
            },
            PostingInput {
                account: account.to_owned(),
                ..Default::default()
            },
        ],
        ..Default::default()
    }))
}

/// A new transaction for the editor, from the quick row so far: the category with its amount,
/// the account to fill in, and an empty posting for the next part of the split.
#[allow(clippy::too_many_arguments)]
fn split_entry(
    date: Date,
    payee: &str,
    memo: &str,
    category: &str,
    account: &str,
    outflow: &str,
    inflow: &str,
    currency: &str,
) -> Entry {
    let posting = |account: &str, units: Option<Amount>| Posting {
        account: account.to_owned(),
        units,
        flag: None,
        meta: Meta::default(),
        line: 0,
    };
    let amount = [(outflow, ""), (inflow, "-")]
        .into_iter()
        .find_map(|(a, sign)| {
            let a = a.trim().trim_start_matches('-');
            Decimal::from_str_exact(&format!("{sign}{}", norm_amount(a))).ok()
        })
        .map(|number| Amount {
            number,
            currency: currency.to_owned(),
            scale: number.scale(),
        });
    let (payee, memo) = (payee.trim(), memo.trim());
    Entry {
        date,
        file: "".into(),
        line: 0,
        end_line: 0,
        seq: 0,
        meta: Meta::default(),
        body: Body::Txn(Transaction {
            flag: "*".to_owned(),
            payee: Some(payee.to_owned()).filter(|p| !p.is_empty()),
            narration: memo.to_owned(),
            tags: Vec::new(),
            links: Vec::new(),
            postings: vec![
                posting(category, amount),
                posting("", None),
                posting(account, None),
            ],
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_quick_row_writes_an_outflow_or_an_inflow_against_the_account() {
        let targets = ["Expenses:Food:Dining".to_owned(), "Income:Other".to_owned()];
        let funding = ["Assets:Checking".to_owned()];
        let entry = |payee, category, memo, out, inn| {
            quick_entry(
                &targets,
                &funding,
                "2026-11-01",
                payee,
                category,
                "Assets:Checking",
                memo,
                out,
                inn,
            )
        };
        let Ok(EntryInput::Txn(t)) = entry("Cafe", "Expenses:Food:Dining", "", "12.5", "") else {
            panic!("refused")
        };
        assert_eq!((t.payee, t.narration.as_str()), (None, "Cafe"));
        assert_eq!(t.postings[0].amount.as_deref(), Some("12.50"));
        assert_eq!(t.postings[1].amount, None);
        let Ok(EntryInput::Txn(t)) = entry("Work", "Income:Other", "Refund", "", "-30") else {
            panic!("refused")
        };
        assert_eq!(
            (t.payee.as_deref(), t.narration.as_str()),
            (Some("Work"), "Refund")
        );
        assert_eq!(t.postings[0].amount.as_deref(), Some("-30.00"));
        assert!(entry("", "Expenses:Nope", "", "1", "").is_err());
        assert!(entry("", "Expenses:Food:Dining", "", "1", "2").is_err());
        assert!(entry("", "Expenses:Food:Dining", "", "", "").is_err());
    }

    #[test]
    fn a_split_starts_from_the_quick_row() {
        let e = split_entry(
            "2026-11-01".parse().unwrap(),
            "Shop",
            "",
            "Expenses:Food",
            "Assets:Checking",
            "",
            "12.5",
            "USD",
        );
        let t = e.txn().unwrap();
        assert_eq!(t.payee.as_deref(), Some("Shop"));
        let accounts: Vec<&str> = t.postings.iter().map(|p| p.account.as_str()).collect();
        assert_eq!(accounts, ["Expenses:Food", "", "Assets:Checking"]);
        assert_eq!(
            t.postings[0].units.as_ref().unwrap().number.to_string(),
            "-12.50"
        );
        assert!(t.postings[2].units.is_none());
        let blank = split_entry(
            "2026-11-01".parse().unwrap(),
            "",
            "",
            "",
            "Assets:Checking",
            "",
            "",
            "USD",
        );
        assert!(blank.txn().unwrap().postings[0].units.is_none());
    }
}
