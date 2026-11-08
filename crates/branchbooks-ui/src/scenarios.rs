//! Scenarios: what-if copies of the book as git
//! branches. Start one from the book as it is, open it (every view then works on it, and the book
//! is untouched), compare it with the book (the target rules that differ, then what they change),
//! merge it into the book as one commit, or set it aside. Sync carries them between devices.
// shortcut: no comparison chart, target sliders or automatic cover yet; add them here.

use branchbooks_core::diff::BudgetDiff;
use dioxus::prelude::*;

use crate::books;
use crate::envelopes::target_rule;
use crate::text::{fmt, month_label, short};
use branchbooks_core::date::Month;

/// The next 12 months from `month`, for a comparison.
fn year_from(month: Month) -> Month {
    (0..11).fold(month, |m, _| m.next())
}

#[component]
pub fn Scenarios(
    /// The book's id.
    id: String,
    /// The scenario open, if any.
    open: Option<String>,
    month: Month,
    notice: Signal<Option<(bool, String)>>,
    onscenario: EventHandler<Option<String>>,
) -> Element {
    // Bumped after a change, to read the list again.
    let mut changed = use_signal(|| 0u32);
    let mut name = use_signal(String::new);
    let mut comparing = use_signal(|| None::<String>);
    let mut confirming = use_signal(|| None::<(String, &'static str)>);
    let to = year_from(month);
    // Kept, so that typing a name does not read git and load both ledgers again on each key.
    let list = use_memo(use_reactive((&id,), move |(id,)| {
        let _ = changed();
        books::scenarios(&id).map_err(|e| e.to_string())
    }))();
    let diff = use_memo(use_reactive((&id, &month), move |(id, month)| {
        let _ = changed();
        comparing().map(|n| {
            let found = books::compare(&id, &n, month, year_from(month));
            (n, found.map_err(|e| e.to_string()))
        })
    }))();
    rsx! {
        section { "aria-label": "Start a scenario",
            form { class: "inline-form", "aria-label": "Start a scenario",
                onsubmit: {
                    let id = id.clone();
                    move |e: FormEvent| {
                        e.prevent_default();
                        let n = name.peek().trim().to_owned();
                        match books::start_scenario(&id, &n) {
                            Ok(()) => {
                                name.set(String::new());
                                changed += 1;
                                notice.set(Some((true, format!("Started {n}: you are in it now."))));
                                onscenario.call(Some(n));
                            }
                            Err(e) => notice.set(Some((false, e.to_string()))),
                        }
                    }
                },
                h2 { "Start a scenario" }
                label { "Name"
                    input { value: "{name}", placeholder: "rent-2027", required: true, oninput: move |e| name.set(e.value()) }
                }
                button { r#type: "submit", "Start" }
            }
            p { class: "muted small",
                "A scenario is a copy of the book to try changes in: targets, assignments, transactions. "
                "The book stays as it is until you merge the scenario into it."
            }
        }
        section { "aria-label": "Scenarios",
            h2 { "Scenarios" }
            match list {
                Err(e) => rsx! { p { class: "notice error", "{e}" } },
                Ok(list) if list.is_empty() => rsx! { p { class: "muted", "No scenarios yet." } },
                Ok(list) => rsx! {
                    ul { class: "register",
                        for s in list {
                            li { key: "{s.name}", "data-testid": "scenario",
                                div { class: "txn-head",
                                    span { class: "desc",
                                        strong { "{s.name}" }
                                        if open.as_deref() == Some(s.name.as_str()) { " " span { class: "badge", "open" } }
                                    }
                                    span { class: "actions",
                                        if let Some((who, what)) = confirming() && who == s.name {
                                            span { class: "muted small", if what == "merge" { "Merge {s.name} into the book?" } else { "Set {s.name} aside?" } }
                                            button { r#type: "button", class: if what == "merge" { "" } else { "danger" },
                                                onclick: {
                                                    let (id, n, open) = (id.clone(), s.name.clone(), open.clone());
                                                    move |_| {
                                                        confirming.set(None);
                                                        let r = if what == "merge" {
                                                            books::merge_scenario(&id, &n)
                                                        } else {
                                                            books::archive_scenario(&id, &n).map(|()| format!("Set {n} aside."))
                                                        };
                                                        let ok = r.is_ok();
                                                        notice.set(Some(match r { Ok(t) => (true, t), Err(e) => (false, e.to_string()) }));
                                                        changed += 1;
                                                        if ok && open.as_deref() == Some(n.as_str()) {
                                                            onscenario.call(None);
                                                        }
                                                    }
                                                },
                                                if what == "merge" { "Merge" } else { "Set aside" }
                                            }
                                            button { r#type: "button", class: "link", onclick: move |_| confirming.set(None), "Keep" }
                                        } else {
                                            if open.as_deref() != Some(s.name.as_str()) {
                                                button { r#type: "button", class: "link",
                                                    onclick: { let n = s.name.clone(); move |_| onscenario.call(Some(n.clone())) },
                                                    "Open"
                                                }
                                            }
                                            button { r#type: "button", class: "link",
                                                onclick: {
                                                    let n = s.name.clone();
                                                    move |_| {
                                                        let same = comparing.peek().as_deref() == Some(n.as_str());
                                                        comparing.set((!same).then(|| n.clone()));
                                                    }
                                                },
                                                "Compare"
                                            }
                                            button { r#type: "button", class: "link",
                                                onclick: { let n = s.name.clone(); move |_| confirming.set(Some((n.clone(), "merge"))) },
                                                "Merge"
                                            }
                                            button { r#type: "button", class: "link",
                                                onclick: { let n = s.name.clone(); move |_| confirming.set(Some((n.clone(), "archive"))) },
                                                "Set aside"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
            }
        }
        if let Some((n, d)) = diff {
            section { "aria-label": "Comparison",
                h2 { "{n} against the book " span { class: "muted small-inline", "{month_label(month)} to {month_label(to)}" } }
                match d {
                    Err(e) => rsx! { p { class: "notice error", "{e}" } },
                    Ok(d) => rsx! { Comparison { diff: d } },
                }
            }
        }
    }
}

/// A comparison: the target rules that differ, then Unassigned and the categories they move.
#[component]
fn Comparison(diff: BudgetDiff) -> Element {
    let rule = |t: &Option<branchbooks_core::budget::Target>| {
        t.as_ref().map_or("no target".to_owned(), target_rule)
    };
    if diff.rules.is_empty() && diff.categories.is_empty() && diff.unassigned.is_empty() {
        return rsx! { p { class: "muted", "No difference in these months." } };
    }
    rsx! {
        if !diff.rules.is_empty() {
            h3 { "Targets" }
            ul {
                for r in diff.rules.iter() {
                    li { "{short(&r.category)}: {rule(&r.before)} → {rule(&r.after)} ({month_label(r.from)} to {month_label(r.to)})" }
                }
            }
        }
        if !diff.unassigned.is_empty() {
            h3 { "Unassigned" }
            table { class: "report",
                thead { tr { th { "Month" } th { class: "num", "Book" } th { class: "num", "Scenario" } } }
                tbody {
                    for r in diff.unassigned.iter() {
                        tr { td { {month_label(r.month)} } td { class: "num", {fmt(r.before)} } td { class: "num", {fmt(r.after)} } }
                    }
                }
            }
        }
        if !diff.categories.is_empty() {
            h3 { "Available, by category" }
            table { class: "report",
                thead { tr { th { "Month" } th { "Category" } th { class: "num", "Book" } th { class: "num", "Scenario" } } }
                tbody {
                    for c in diff.categories.iter().take(40) {
                        tr {
                            td { {month_label(c.month)} }
                            td { {short(&c.category)} }
                            td { class: "num", {fmt(c.before.available)} }
                            td { class: "num", {fmt(c.after.available)} }
                        }
                    }
                }
            }
            if diff.categories.len() > 40 {
                p { class: "muted small", "and {diff.categories.len() - 40} more" }
            }
        }
    }
}
