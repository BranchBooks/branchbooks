//! The rest of the Budget view: target shortfalls, the Target editor, card payments, the month's notes, moving money,
//! and the month's assignments, moves and targets.
// shortcut: closing a month shows no "changed since closed" yet, and the write path does not warn
// about an edit dated in a closed month; add both when closes are used in earnest.

use branchbooks_core::budget::{CardRow, CategoryRow, Target};
use branchbooks_core::types::{CustomValue, Diagnostic, Entry, MetaValue};
use branchbooks_core::write::render::{AssignInput, EntryInput, MoveInput};
use dioxus::prelude::*;
use rust_decimal::Decimal;

use crate::text::{fmt, month_label, norm_amount, parse_amount, short};
use branchbooks_core::date::Month;

/// A target rule in words: "1,400.00 a month", or "6,000.00 by June 2027".
pub fn target_rule(t: &Target) -> String {
    match &t.due {
        Some(due) => format!("{} by {}", fmt(t.amount), month_label(due.month())),
        None => format!("{} a month", fmt(t.amount)),
    }
}

#[component]
pub fn Shortfalls(items: Vec<(String, Decimal)>) -> Element {
    rsx! {
        if !items.is_empty() {
            section { class: "notice error", "aria-label": "Target shortfalls",
                strong {
                    if items.len() == 1 { "A target is short this month" } else { "{items.len()} targets are short this month" }
                }
                ul {
                    for (category, amount) in items {
                        li { key: "{category}", "{short(&category)} is {fmt(amount)} short." }
                    }
                }
            }
        }
    }
}

/// Sets, changes or ends a category's target. "Every month" refills it to the amount each month;
/// "by" saves the amount by the end of that month. Rollover keeps the defaults (T2).
/// `onsave`: the amount and the due date (`None`: monthly); 0.00 ends the target.
#[component]
pub fn TargetForm(
    row: CategoryRow,
    onsave: EventHandler<(String, Option<String>)>,
    oncancel: EventHandler<()>,
    mut notice: Signal<Option<(bool, String)>>,
) -> Element {
    let t = row.target.clone();
    let mut amount = use_signal(|| {
        t.as_ref()
            .map(|t| format!("{:.2}", t.amount))
            .unwrap_or_default()
    });
    let mut by = use_signal(|| {
        t.as_ref()
            .and_then(|t| t.due)
            .map(|d| d.month().to_string())
    });
    let name = short(&row.category).to_owned();
    rsx! {
        form { class: "inline-form target-form", "aria-label": "Target for {name}",
            onsubmit: move |e: FormEvent| {
                e.prevent_default();
                let value = match parse_amount(&amount.peek()) {
                    Ok(value) => value,
                    Err(m) => return notice.set(Some((false, m))),
                };
                if value <= Decimal::ZERO {
                    let m = "A target needs an amount above zero. To remove one, use End target.";
                    return notice.set(Some((false, m.to_owned())));
                }
                let due = by.peek().as_deref().and_then(|m| m.parse::<Month>().ok()).map(|m| m.first_day().to_string());
                onsave.call((format!("{value:.2}"), due));
            },
            label { "Amount"
                input { class: "amount", value: "{amount}", inputmode: "decimal", required: true,
                    oninput: move |e| amount.set(e.value()),
                    onmounted: move |e| async move { let _ = e.set_focus(true).await; },
                }
            }
            label {
                select { "aria-label": "How often", value: if by().is_some() { "by" } else { "monthly" },
                    onchange: move |e| by.set((e.value() == "by").then(String::new)),
                    option { value: "monthly", "every month" }
                    option { value: "by", "by" }
                }
            }
            if let Some(m) = by() {
                label { "Month"
                    input { r#type: "month", value: "{m}", required: true, oninput: move |e| by.set(Some(e.value())) }
                }
            }
            button { r#type: "submit", "Save" }
            if t.is_some() {
                button { r#type: "button", class: "danger", onclick: move |_| onsave.call(("0.00".into(), None)), "End target" }
            }
            button { r#type: "button", class: "link", onclick: move |_| oncancel.call(()), "Cancel" }
        }
    }
}

/// Card payment envelopes. Set aside is editable like Assigned: a new total writes an
/// envelope-assign to the card for the difference. "Cover" sets aside what is underfunded.
#[component]
pub fn CardPayments(
    cards: Vec<CardRow>,
    onsetaside: EventHandler<(String, String)>,
    mut notice: Signal<Option<(bool, String)>>,
) -> Element {
    let mut editing = use_signal(|| None::<String>);
    let mut value = use_signal(String::new);
    rsx! {
        if !cards.is_empty() {
            section { "aria-label": "Credit card payments",
                h2 { "Credit card payments" }
                table {
                    thead { tr { th { "Card" } th { class: "num", "Set aside" } th { class: "num", "Owed" } th { class: "num", "Underfunded" } } }
                    tbody {
                        for c in cards {
                            tr { key: "{c.card}", "data-card": "{c.card}",
                                td { {short(&c.card)} }
                                td { class: "num",
                                    if editing() == Some(c.card.clone()) {
                                        form {
                                            onsubmit: {
                                                let (card, available) = (c.card.clone(), c.available);
                                                move |e: FormEvent| {
                                                    e.prevent_default();
                                                    let total = match parse_amount(&value.peek()) {
                                                        Ok(total) => total,
                                                        Err(m) => return notice.set(Some((false, m))),
                                                    };
                                                    let delta = total - available;
                                                    if !delta.is_zero() {
                                                        onsetaside.call((card.clone(), format!("{delta:.2}")));
                                                    }
                                                    editing.set(None);
                                                }
                                            },
                                            input { class: "amount", "aria-label": "Set aside for {short(&c.card)}", inputmode: "decimal",
                                                value: "{value}", oninput: move |e| value.set(e.value()),
                                                onkeydown: move |e| if e.key() == Key::Escape { editing.set(None) },
                                                onmounted: move |e| async move { let _ = e.set_focus(true).await; },
                                            }
                                        }
                                    } else {
                                        button { r#type: "button", class: "cell", "aria-label": "Edit set aside for {short(&c.card)}",
                                            onclick: {
                                                let (card, available) = (c.card.clone(), c.available);
                                                move |_| {
                                                    value.set(format!("{available:.2}"));
                                                    editing.set(Some(card.clone()));
                                                }
                                            },
                                            {fmt(c.available)}
                                        }
                                    }
                                }
                                td { class: "num", {fmt(c.owed)} }
                                td { class: if c.underfunded > Decimal::ZERO { "num warn" } else { "num" },
                                    {fmt(c.underfunded)}
                                    if c.underfunded > Decimal::ZERO {
                                        " "
                                        button { r#type: "button", class: "link", "aria-label": "Cover {short(&c.card)}",
                                            onclick: {
                                                let (card, under) = (c.card.clone(), c.underfunded);
                                                move |_| onsetaside.call((card.clone(), format!("{under:.2}")))
                                            },
                                            "Cover"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                p { class: "muted small", "Click Set aside to change it: the difference comes from Unassigned, or goes back to it." }
            }
        }
    }
}

/// The month's flags (uncategorized transfers, unfunded categories, blocked entries) and rule 15
/// warnings.
#[component]
pub fn Notes(items: Vec<Diagnostic>) -> Element {
    rsx! {
        if !items.is_empty() {
            section { class: "problems", "aria-label": "Notes for this month",
                h2 { "Notes for this month" }
                ul {
                    for d in items {
                        li { "{d.file}:{d.line}: {d.message}" }
                    }
                }
            }
        }
    }
}

#[component]
pub fn MoveForm(
    categories: Vec<String>,
    onmove: EventHandler<(String, String, String)>,
) -> Element {
    let mut from = use_signal(|| categories.first().cloned().unwrap_or_default());
    let mut to = use_signal(|| categories.get(1).cloned().unwrap_or_default());
    let mut amount = use_signal(String::new);
    rsx! {
        section {
            form { class: "inline-form", "aria-label": "Move money",
                onsubmit: move |e: FormEvent| {
                    e.prevent_default();
                    onmove.call((from.peek().clone(), to.peek().clone(), norm_amount(&amount.peek())));
                    amount.set(String::new());
                },
                h2 { "Move money" }
                label { "From"
                    select { value: "{from}", onchange: move |e| from.set(e.value()),
                        for c in categories.iter() { option { key: "{c}", value: "{c}", selected: *c == from(), {short(c)} } }
                    }
                }
                label { "To"
                    select { value: "{to}", onchange: move |e| to.set(e.value()),
                        for c in categories.iter() { option { key: "{c}", value: "{c}", selected: *c == to(), {short(c)} } }
                    }
                }
                label { "Amount"
                    input { class: "amount", value: "{amount}", inputmode: "decimal", required: true,
                        oninput: move |e| amount.set(e.value()),
                    }
                }
                button { r#type: "submit", "Move" }
            }
        }
    }
}

/// An envelope entry in words, or `None` for any other entry: "Assign 50.00 to Food:Dining".
pub fn envelope_text(e: &Entry) -> Option<String> {
    let c = e.custom()?;
    let account = |i: usize| match c.values.get(i) {
        Some(CustomValue::Account(a)) => short(a).to_owned(),
        _ => String::new(),
    };
    let amount = |i: usize| match c.values.get(i) {
        Some(CustomValue::Amount(a)) => a.number,
        _ => Decimal::ZERO,
    };
    Some(match c.name.as_str() {
        "envelope-assign" => format!("Assign {} to {}", fmt(amount(1)), account(0)),
        "envelope-move" => format!(
            "Move {} from {} to {}",
            fmt(amount(2)),
            account(0),
            account(1)
        ),
        "target" if amount(1).is_zero() => format!("Target for {}: ended", account(0)),
        "target" => match e.meta.get("due") {
            Some(MetaValue::Date(due)) => {
                format!("Target for {}: {} by {due}", account(0), fmt(amount(1)))
            }
            Some(MetaValue::String(due)) => {
                format!("Target for {}: {} by {due}", account(0), fmt(amount(1)))
            }
            _ => format!("Target for {}: {} a month", account(0), fmt(amount(1))),
        },
        _ => return None,
    })
}

/// The month's assignments, moves and targets, newest first. Assignments and moves have Edit;
/// a target is changed in the Target column. Each has Delete.
#[component]
pub fn MonthEntries(
    entries: Vec<(Entry, String)>,
    categories: Vec<String>,
    onedit: EventHandler<(Entry, EntryInput)>,
    ondelete: EventHandler<Entry>,
) -> Element {
    let mut confirming = use_signal(|| None::<String>);
    let mut editing = use_signal(|| None::<String>);
    let key = |e: &Entry| format!("{}:{}", e.file, e.line);
    let editable = |e: &Entry| {
        e.custom()
            .is_some_and(|c| c.name == "envelope-assign" || c.name == "envelope-move")
    };
    rsx! {
        section { "aria-label": "Budget entries",
            h2 { "Assignments, moves and targets" }
            if entries.is_empty() {
                p { class: "muted", "No assignments, moves or targets this month." }
            }
            ul { class: "register",
                for (e, text) in entries {
                    if editing() == Some(key(&e)) {
                        li { key: "{key(&e)}", class: "editing",
                            EnvelopeEditor { entry: e.clone(), categories: categories.clone(),
                                oncancel: move |()| editing.set(None),
                                onsave: {
                                    let e = e.clone();
                                    move |input: EntryInput| {
                                        editing.set(None);
                                        onedit.call((e.clone(), input));
                                    }
                                },
                            }
                        }
                    } else {
                        li { key: "{key(&e)}", "data-testid": "envelope-entry",
                            div { class: "txn-head",
                                span { class: "date", "{e.date}" }
                                span { class: "desc", "{text}" }
                                span { class: "actions",
                                    if confirming() == Some(key(&e)) {
                                        span { class: "muted small", "Delete this entry?" }
                                        button { r#type: "button", class: "danger",
                                            onclick: {
                                                let e = e.clone();
                                                move |_| {
                                                    confirming.set(None);
                                                    ondelete.call(e.clone());
                                                }
                                            },
                                            "Delete"
                                        }
                                        button { r#type: "button", class: "link", onclick: move |_| confirming.set(None), "Keep" }
                                    } else {
                                        if editable(&e) {
                                            button { r#type: "button", class: "link", "aria-label": "Edit {e.date} {text}",
                                                onclick: {
                                                    let k = key(&e);
                                                    move |_| {
                                                        confirming.set(None);
                                                        editing.set(Some(k.clone()));
                                                    }
                                                },
                                                "Edit"
                                            }
                                        }
                                        button { r#type: "button", class: "link", "aria-label": "Delete {e.date} {text}",
                                            onclick: {
                                                let k = key(&e);
                                                move |_| {
                                                    editing.set(None);
                                                    confirming.set(Some(k.clone()));
                                                }
                                            },
                                            "Delete"
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

/// Edits an assignment (date, category, amount) or a move (date, from, to, amount).
#[component]
fn EnvelopeEditor(
    entry: Entry,
    categories: Vec<String>,
    onsave: EventHandler<EntryInput>,
    oncancel: EventHandler<()>,
) -> Element {
    let c = entry.custom().cloned().expect("an envelope entry");
    let is_move = c.name == "envelope-move";
    let account = |i: usize| match c.values.get(i) {
        Some(CustomValue::Account(a)) => a.clone(),
        _ => String::new(),
    };
    let amount_at = if is_move { 2 } else { 1 };
    let mut date = use_signal(|| entry.date.to_string());
    let mut first = use_signal(|| account(0));
    let mut second = use_signal(|| account(1));
    let mut amount = use_signal(|| match c.values.get(amount_at) {
        Some(CustomValue::Amount(a)) => a.number.to_string(),
        _ => String::new(),
    });
    // A category no longer budgeted (closed, say) still shows as the current choice.
    let options = move |current: String| {
        let mut list = categories.clone();
        if !current.is_empty() && !list.contains(&current) {
            list.insert(0, current);
        }
        list
    };
    let (first_options, second_options) = (options(first()), options(second()));
    rsx! {
        form { class: "editor", "aria-label": if is_move { "Edit move" } else { "Edit assignment" },
            onsubmit: move |e: FormEvent| {
                e.prevent_default();
                let (date, amount) = (date.peek().clone(), norm_amount(&amount.peek()));
                onsave.call(if is_move {
                    EntryInput::Move(MoveInput { date, from: first.peek().clone(), to: second.peek().clone(), amount, currency: None })
                } else {
                    EntryInput::Assign(AssignInput { date, category: first.peek().clone(), amount, currency: None })
                });
            },
            div { class: "editor-head",
                label { "Date"
                    input { r#type: "date", value: "{date}", required: true, oninput: move |e| date.set(e.value()) }
                }
                label { if is_move { "From" } else { "Category" }
                    select { value: "{first}", onchange: move |e| first.set(e.value()),
                        for o in first_options { option { key: "{o}", value: "{o}", selected: o == first(), {short(&o)} } }
                    }
                }
                if is_move {
                    label { "To"
                        select { value: "{second}", onchange: move |e| second.set(e.value()),
                            for o in second_options { option { key: "{o}", value: "{o}", selected: o == second(), {short(&o)} } }
                        }
                    }
                }
                label { "Amount"
                    input { class: "amount", value: "{amount}", inputmode: "decimal", required: true,
                        oninput: move |e| amount.set(e.value()),
                    }
                }
            }
            div { class: "editor-actions",
                span {}
                button { r#type: "button", class: "link", onclick: move |_| oncancel.call(()), "Cancel" }
                button { r#type: "submit", "Save" }
            }
        }
    }
}

/// Closing a month: a past month can be closed, which tags the book as it is now
/// (`close/<month>`, carried by sync), and reopened. `onchanged`: after either, so the app syncs.
#[component]
pub fn MonthClose(
    /// The book's id.
    id: String,
    month: Month,
    this_month: Month,
    notice: Signal<Option<(bool, String)>>,
    onchanged: EventHandler<()>,
) -> Element {
    let mut changed = use_signal(|| 0u32);
    let mut confirming = use_signal(|| false);
    let _ = changed();
    if month >= this_month {
        return rsx! {};
    }
    let closed = crate::books::closes(&id)
        .into_iter()
        .find(|c| c.month == month);
    let label = month_label(month);
    let mut act = {
        let (id, label) = (id.clone(), label.clone());
        move |close: bool| {
            let r = if close {
                crate::books::close_month(&id, month).map(|()| format!("Closed {label}."))
            } else {
                crate::books::reopen_month(&id, month).map(|()| format!("Reopened {label}."))
            };
            let ok = r.is_ok();
            notice.set(Some(match r {
                Ok(t) => (true, t),
                Err(e) => (false, e.to_string()),
            }));
            confirming.set(false);
            changed += 1;
            if ok {
                onchanged.call(());
            }
        }
    };
    let mut act2 = act.clone();
    rsx! {
        div { class: "month-close", "data-testid": "month-close",
            match closed {
                Some(c) => rsx! {
                    span { class: "badge", "closed" }
                    span { class: "muted small-inline", " {c.at.get(..10).unwrap_or(&c.at)} by {c.by} " }
                    button { r#type: "button", class: "link", onclick: move |_| act(false), "Reopen {label}" }
                },
                None if confirming() => rsx! {
                    span { class: "muted small-inline", "Close {label}? The book as it is now is kept as {label}'s record. " }
                    button { r#type: "button", onclick: move |_| act2(true), "Close" }
                    button { r#type: "button", class: "link", onclick: move |_| confirming.set(false), "Cancel" }
                },
                None => rsx! {
                    button { r#type: "button", class: "link", onclick: move |_| confirming.set(true), "Close {label}" }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use branchbooks_core::types::{Amount, Body, Custom, Meta};
    use std::str::FromStr;

    fn custom(name: &str, values: Vec<CustomValue>) -> Entry {
        Entry {
            date: "2026-11-02".parse().unwrap(),
            file: "main.beancount".into(),
            line: 1,
            end_line: 1,
            seq: 0,
            meta: Meta::default(),
            body: Body::Custom(Custom {
                name: name.into(),
                values,
            }),
        }
    }

    #[test]
    fn envelope_entries_read_as_words() {
        let usd = |n: &str| {
            CustomValue::Amount(Amount {
                number: Decimal::from_str(n).unwrap(),
                currency: "USD".into(),
                scale: 2,
            })
        };
        let acct = |a: &str| CustomValue::Account(a.into());
        let assign = custom(
            "envelope-assign",
            vec![acct("Expenses:Food:Dining"), usd("50")],
        );
        assert_eq!(
            envelope_text(&assign).unwrap(),
            "Assign 50.00 to Food:Dining"
        );
        let mv = custom(
            "envelope-move",
            vec![
                acct("Expenses:Housing:Rent"),
                acct("Expenses:Food:Dining"),
                usd("20"),
            ],
        );
        assert_eq!(
            envelope_text(&mv).unwrap(),
            "Move 20.00 from Housing:Rent to Food:Dining"
        );
        let ended = custom("target", vec![acct("Expenses:Housing:Rent"), usd("0")]);
        assert_eq!(
            envelope_text(&ended).unwrap(),
            "Target for Housing:Rent: ended"
        );
        assert_eq!(envelope_text(&custom("other", vec![])), None);
    }
}
