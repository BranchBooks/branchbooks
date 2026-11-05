//! The Budget view: the month, Unassigned, and the envelopes grouped by parent category, with
//! Assigned editable. The rest of the view is in `envelopes`.

use std::cell::RefCell;
use std::rc::Rc;

use branchbooks_core::budget::CategoryRow;
use branchbooks_core::date::Date;
use branchbooks_core::types::{Diagnostic, Entry};
use branchbooks_core::write::render::{
    AssignInput, EntryInput, MoveInput, OverspendingInput, TargetInput,
};
use branchbooks_core::write::store::CommitInfo;
use branchbooks_core::write::workspace::{
    ApplyError, Operation, OperationError, OperationErrorCode, WorkspaceCore, retry_on_conflict,
};
use dioxus::prelude::*;
use rust_decimal::Decimal;

use crate::accounts::Accounts;
use crate::envelopes::{
    CardPayments, MonthClose, MonthEntries, MoveForm, Notes, OverspendingSwitch, Shortfalls,
    TargetForm, envelope_text, target_rule,
};
use crate::files::Files;
use crate::journal::{Journal, Shared, run};
use crate::overview::Overview;
use crate::reports::Reports;
use crate::scenarios::Scenarios;
use crate::store::{Backend, local_today, new_id};
use crate::text::{envelope_date, fmt, is_far_date, month_label, parse_amount, short, words};

struct Group {
    name: String,
    rows: Vec<(CategoryRow, String)>,
    assigned: Decimal,
    target: Decimal,
    activity: Decimal,
    available: Decimal,
}

const GENERAL: &str = "General";

/// The views in the sidebar: id, name, icon path.
const PAGES: [(&str, &str, &str); 7] = [
    ("overview", "Overview", "M3 12l9-8 9 8M5 10v10h14V10"),
    ("journal", "Journal", "M5 4h14v16H5zM9 8h6M9 12h6M9 16h4"),
    ("accounts", "Accounts", "M4 6h16v12H4zM4 10h16M8 15h3"),
    ("budget", "Budget", "M3 7h18v12H3zM3 7l3-3h12l3 3M8 12h8"),
    ("reports", "Reports", "M4 20V10M10 20V4M16 20v-7M22 20H2"),
    ("files", "Files", "M6 3h9l5 5v13H6zM14 3v6h6"),
    (
        "scenarios",
        "Scenarios",
        "M6 3v12M18 9a3 3 0 1 0 0-.1M6 21a3 3 0 1 0 0-.1M18 12c0 4-6 3-12 6",
    ),
];

/// The sidebar's groups, each a heading (none for the first) and the views under it.
const NAV: [(&str, &[&str]); 4] = [
    ("", &["overview"]),
    ("Ledger", &["journal", "accounts", "budget", "reports"]),
    ("Import", &["files"]),
    ("Advanced", &["scenarios"]),
];

/// Categories grouped by the part after "Expenses:": Expenses:Insurance:Auto-Insurance is "Auto
/// Insurance" in Insurance. A category with no parent of its own goes to its group if one exists
/// (Expenses:Insurance itself, as "Insurance (general)"), else to General.
fn group_categories(rows: &[CategoryRow]) -> Vec<Group> {
    let parts = |r: &CategoryRow| -> Vec<String> {
        r.category.split(':').skip(1).map(str::to_owned).collect()
    };
    let parents: Vec<String> = rows
        .iter()
        .map(parts)
        .filter(|p| p.len() > 1)
        .map(|p| p[0].clone())
        .collect();
    let mut groups: Vec<Group> = Vec::new();
    for row in rows {
        let p = parts(row);
        let first = p.first().cloned().unwrap_or_default();
        let name = if p.len() > 1 || parents.contains(&first) {
            first.clone()
        } else {
            GENERAL.to_owned()
        };
        let label = if p.len() > 1 {
            p[1..]
                .iter()
                .map(|s| words(s))
                .collect::<Vec<_>>()
                .join(" · ")
        } else if name == GENERAL {
            words(&first)
        } else {
            format!("{} (general)", words(&first))
        };
        let at = if let Some(i) = groups.iter().position(|g| g.name == name) {
            i
        } else {
            groups.push(Group {
                name,
                rows: Vec::new(),
                assigned: Decimal::ZERO,
                target: Decimal::ZERO,
                activity: Decimal::ZERO,
                available: Decimal::ZERO,
            });
            groups.len() - 1
        };
        let g = &mut groups[at];
        g.assigned += row.assigned;
        g.target += row.target_allocated;
        g.activity += row.activity;
        g.available += row.available;
        g.rows.push((row.clone(), label));
    }
    groups.sort_by(|a, b| match (a.name == GENERAL, b.name == GENERAL) {
        (true, false) => std::cmp::Ordering::Greater,
        (false, true) => std::cmp::Ordering::Less,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    groups
}

/// Plans an operation, commits it to the store and keeps the result; a conflict (another device
/// saved first) reloads the book and tries again, twice. Returns the warnings that do not block it.
pub(crate) fn apply(
    backend: &Backend,
    core: &RefCell<WorkspaceCore>,
    op: Operation,
) -> Result<Vec<Diagnostic>, ApplyError> {
    check_date(&op, local_today())?;
    let op_id = new_id();
    retry_on_conflict(2, |again| {
        if again {
            let snap = backend.snapshot()?;
            core.borrow_mut().reset(&snap.version, snap.files);
        }
        let (planned, base) = {
            let mut c = core.borrow_mut();
            let planned = c.plan(&op)?;
            (planned, c.version.clone())
        };
        let info = CommitInfo {
            op_id: op_id.clone(),
            op_type: planned.op_type.clone(),
            summary: planned.summary.clone(),
        };
        let version = backend.commit(&base, &planned.changes, &info)?;
        Ok(core.borrow_mut().commit_planned(planned, &version))
    })
}

/// Refuses a date too far from `today` to be meant, such as 9026 for 2026: one such entry makes the
/// budget walk thousands of empty months on every later save.
fn check_date(op: &Operation, today: Date) -> Result<(), OperationError> {
    let date = match op {
        Operation::Add(e) | Operation::Edit(_, e) => e.date(),
        Operation::Reconcile(r) => r.date.as_str(),
        Operation::Delete(_) | Operation::RenameAccount { .. } => return Ok(()),
    };
    // A date that does not parse is the write path's to refuse.
    match date.parse() {
        Ok(d) if is_far_date(d, today) => Err(OperationError {
            code: OperationErrorCode::Invalid,
            message: format!("{date} is too far from today: check the year."),
            diagnostics: Vec::new(),
        }),
        _ => Ok(()),
    }
}

/// `a` when open, else `b`. (An `if` in a component prop is rsx!'s own syntax, which makes a String.)
fn if_open(open: bool, a: &'static str, b: &'static str) -> &'static str {
    if open { a } else { b }
}

#[component]
fn Chevron(dir: &'static str) -> Element {
    let d = match dir {
        "left" => "M10 3 5 8l5 5",
        "right" => "M6 3l5 5-5 5",
        _ => "M3 6l5 5 5-5",
    };
    rsx! {
        svg { class: "chevron-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d }
        }
    }
}

/// One book's Budget view, loaded from `backend`. `today` dates new entries.
/// `children`: the app's book controls, at the foot of the sidebar. `onsaved`: after each change
/// is committed (the app syncs then).
#[component]
pub fn Screen(
    backend: Backend,
    today: Date,
    children: Element,
    onsaved: Option<EventHandler<()>>,
    /// The view to open on (an id in PAGES); the Overview by default.
    page: Option<&'static str>,
    /// Opens a scenario (`Some`) or goes back to the book (`None`).
    onscenario: Option<EventHandler<Option<String>>>,
    /// Bumped to load the book again, after a sync brings changes in. The page, month and any form
    /// being typed in stay as they are.
    reloads: u32,
    /// Opens Settings, from the foot of the sidebar.
    onsettings: Option<EventHandler<()>>,
) -> Element {
    let core = use_hook(|| Rc::new(RefCell::new(WorkspaceCore::new(None))));
    // The version on screen: None while loading. Setting it redraws.
    let mut version = use_signal(|| None::<String>);
    let mut month = use_signal(|| today.month());
    let mut notice = use_signal(|| None::<(bool, String)>);
    let mut editing = use_signal(|| None::<String>);
    let value = use_signal(String::new);
    let mut collapsed = use_signal(Vec::<String>::new);
    let mut page = use_signal(|| page.unwrap_or("overview"));
    // The sidebar: collapsed to icons on a wide window, opened over the page on a narrow one. The
    // stylesheet honours each only at its width.
    let mut sidebar_collapsed = use_signal(|| false);
    let mut menu_open = use_signal(|| false);
    // The account or category the Journal is filtered to, from Accounts or the Budget's Activity.
    let mut filter = use_signal(|| None::<String>);
    // The category whose target editor is open.
    let mut target_for = use_signal(|| None::<String>);
    // The add row's last account, kept while the book is open.
    // shortcut: forgotten when the app closes; keep it in the book's git config if that bothers.
    let last_account = use_signal(String::new);
    // A new version after the first load is a commit made here: tell the app.
    let mut seen = use_signal(|| None::<String>);
    use_effect(move || {
        if let Some(v) = version() {
            if seen.peek().as_ref().is_some_and(|s| *s != v)
                && let Some(h) = onsaved
            {
                h.call(());
            }
            seen.set(Some(v));
        }
    });

    use_future({
        let (core, backend) = (core.clone(), backend.clone());
        move || {
            let (core, backend) = (core.clone(), backend.clone());
            async move {
                match backend.snapshot() {
                    Ok(snap) => {
                        let v = snap.version.clone();
                        core.borrow_mut().reset(&snap.version, snap.files);
                        version.set(Some(v));
                    }
                    Err(e) => notice.set(Some((false, e.to_string()))),
                }
            }
        }
    });

    let mut loaded = use_signal(|| reloads);
    use_effect(use_reactive((&reloads,), {
        let (core, backend) = (core.clone(), backend.clone());
        move |(reloads,)| {
            if *loaded.peek() == reloads {
                return;
            }
            loaded.set(reloads);
            match backend.snapshot() {
                Ok(snap) => {
                    let v = snap.version.clone();
                    core.borrow_mut().reset(&snap.version, snap.files);
                    // Seen already: it came from a sync, so it is no commit to sync again.
                    seen.set(Some(v.clone()));
                    version.set(Some(v));
                }
                Err(e) => notice.set(Some((false, e.to_string()))),
            }
        }
    }));

    let assign = use_callback({
        let (core, backend, today) = (core.clone(), backend.clone(), today);
        move |(category, amount): (String, String)| {
            let (core, backend) = (core.clone(), backend.clone());
            let entry = EntryInput::Assign(AssignInput {
                date: envelope_date(*month.peek(), today).to_string(),
                category: category.clone(),
                amount: amount.clone(),
                currency: None,
            });
            match apply(&backend, &core, Operation::Add(entry)) {
                Ok(warnings) => {
                    editing.set(None);
                    let text = if warnings.is_empty() {
                        format!("Assigned {amount} to {}.", short(&category))
                    } else {
                        let w: Vec<&str> = warnings.iter().map(|w| w.message.as_str()).collect();
                        format!("Saved, but: {}", w.join("; "))
                    };
                    notice.set(Some((warnings.is_empty(), text)));
                }
                Err(e) => notice.set(Some((false, e.to_string()))),
            }
            let v = core.borrow().version.clone();
            version.set(Some(v));
        }
    });

    // Any other change from the Budget page (targets, moves, card payments, deleting an entry).
    let save = use_callback({
        let (core, backend) = (core.clone(), backend.clone());
        move |(op, done): (Operation, String)| {
            let (core, backend) = (Shared(core.clone()), backend.clone());
            if run(backend, core, op, &done, version, notice) {
                target_for.set(None);
            }
        }
    });

    let Some(_) = version() else {
        return rsx! {
            crate::Splash {
                if let Some((_, text)) = notice() { p { class: "notice error", "{text}" } }
            }
        };
    };
    let this_month = today.month();
    let current_month = this_month;
    let book_id = backend
        .dir()
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_owned();
    let shown = month();
    let view = core.borrow_mut().book().budget(shown, Some(today));
    let groups = group_categories(&view.categories);
    let errors = core.borrow_mut().book().errors();
    let env_date = envelope_date(shown, today).to_string();
    // Under T1 a past month's rules never change, so targets are set from this month on.
    let can_target = shown >= this_month;
    let categories: Vec<String> = view.categories.iter().map(|c| c.category.clone()).collect();
    let mut entries: Vec<(Entry, String)> = core
        .borrow()
        .entries()
        .map(|(_, e)| e)
        .filter(|e| e.date.month() == shown)
        .filter_map(|e| envelope_text(e).map(|t| (e.clone(), t)))
        .collect();
    entries.sort_by(|a, b| b.0.date.cmp(&a.0.date).then(b.0.seq.cmp(&a.0.seq)));

    rsx! {
        div { class: "app", "data-collapsed": sidebar_collapsed(), "data-menu": menu_open(),
            button { r#type: "button", class: "scrim", "aria-label": "Close the sidebar",
                onclick: move |_| menu_open.set(false),
            }
            // The app's frame.
            aside { class: "sidebar",
                div { class: "brand",
                    crate::Logo {}
                    span { class: "brand-name", "BranchBooks" }
                }
                nav { "aria-label": "Main",
                    for (heading, ids) in NAV {
                        div { key: "{heading}", class: "nav-group",
                            if !heading.is_empty() {
                                p { class: "nav-heading", "{heading}" }
                            }
                            ul {
                                for (id, label, d) in PAGES.into_iter().filter(|p| ids.contains(&p.0)) {
                                    li { key: "{id}",
                                        a { href: "#", title: label, "aria-current": if page() == id { "page" } else { "false" },
                                            onclick: move |e| {
                                                e.prevent_default();
                                                page.set(id);
                                                filter.set(None);
                                                // A narrow window's sidebar covers the page: get out of the way.
                                                menu_open.set(false);
                                            },
                                            svg { class: "icon", view_box: "0 0 24 24", "aria-hidden": "true",
                                                path { d }
                                            }
                                            span { class: "nav-label", {label} }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                div { class: "sidebar-foot",
                    if let Some(open) = onsettings {
                        button { r#type: "button", class: "settings-link", title: "Settings",
                            onclick: move |_| open.call(()),
                            svg { class: "icon", view_box: "0 0 24 24", "aria-hidden": "true",
                                path { d: "M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z" }
                            }
                            span { class: "nav-label", "Settings" }
                        }
                    }
                    span { class: "muted small-inline", "data-testid": "storage", "Saved on this device" }
                    span { class: "muted small-inline", "BranchBooks {env!(\"CARGO_PKG_VERSION\")}" }
                }
            }
            div { class: "main",
                header { class: "topbar",
                    button { r#type: "button", class: "icon", "aria-label": "Toggle the sidebar",
                        "aria-pressed": sidebar_collapsed(),
                        onclick: move |_| {
                            sidebar_collapsed.toggle();
                            menu_open.toggle();
                        },
                        svg { class: "chevron-icon", view_box: "0 0 16 16", "aria-hidden": "true",
                            path { d: "M2 4h12M2 8h12M2 12h12" }
                        }
                    }
                    div { class: "crumbs", h1 { {PAGES.iter().find(|p| p.0 == page()).map_or("", |p| p.1)} } }
                    if !["accounts", "files", "overview"].contains(&page()) {
                        nav { class: "months", "aria-label": "Month",
                            button { r#type: "button", class: "icon", "aria-label": "Previous month",
                                onclick: move |_| { let m = month.peek().prev(); month.set(m) },
                                Chevron { dir: "left" }
                            }
                            span { class: "month-label", {month_label(shown)} }
                            button { r#type: "button", class: "icon", "aria-label": "Next month",
                                onclick: move |_| { let m = month.peek().next(); month.set(m) },
                                Chevron { dir: "right" }
                            }
                            if shown != this_month {
                                button { r#type: "button", class: "link",
                                    onclick: move |_| month.set(this_month), "Today"
                                }
                            }
                        }
                    }
                    div { class: "topbar-actions", {children} }
                }
                if let Some(s) = backend.scenario() {
                    div { class: "notice scenario", role: "status", "data-testid": "scenario-banner",
                        p { "Scenario {s}: changes here stay out of the book until you merge it (Scenarios). " }
                        button { r#type: "button", class: "link",
                            onclick: move |_| if let Some(h) = onscenario { h.call(None) },
                            "Back to the book"
                        }
                    }
                }
                if !errors.is_empty() {
                    div { class: "notice error", role: "alert", "data-testid": "ledger-errors",
                        p { "The ledger has {errors.len()} error(s), shown with its first five:" }
                        ul {
                            for d in errors.iter().take(5) {
                                li { "{d.file}:{d.line}: {d.message}" }
                            }
                        }
                    }
                }
                if let Some((ok, text)) = notice() {
                    div { class: if ok { "notice sticky ok" } else { "notice sticky error" },
                        p { role: "status", "{text}" }
                        button { r#type: "button", class: "link", "aria-label": "Dismiss",
                            onclick: move |_| notice.set(None), "×"
                        }
                    }
                }
                main { class: "content", "data-view": page(),
                    if page() == "overview" {
                        Overview { core: Shared(core.clone()), version, today: today,
                            onopen: move |p: &'static str| {
                                filter.set(None);
                                page.set(p);
                            },
                        }
                    } else if page() == "budget" {
                        if backend.scenario().is_none() {
                            MonthClose { id: book_id.clone(), month: shown, this_month: current_month, notice,
                                onchanged: move |()| if let Some(h) = onsaved { h.call(()) },
                            }
                        }
                        section { class: "unassigned", "data-negative": view.unassigned < Decimal::ZERO,
                            div {
                                span { class: "label", "Unassigned" }
                                strong { "data-testid": "unassigned", {fmt(view.unassigned)} }
                            }
                            if view.assigned_to_future > Decimal::ZERO {
                                span { class: "unassigned-note", "{fmt(view.assigned_to_future)} assigned to future months" }
                            }
                        }
                        Shortfalls { items: view.shortfalls.iter().map(|s| (s.category.clone(), s.amount)).collect::<Vec<_>>() }
                        section { "aria-label": "Envelopes",
                            div { class: "section-head", h2 { "Envelopes" } }
                            table { class: "envelopes",
                                thead {
                                    tr {
                                        th { "Category" }
                                        th { class: "num", "Assigned" }
                                        th { class: "num", "Target" }
                                        th { class: "num", "Activity" }
                                        th { class: "num", "Available" }
                                    }
                                }
                                for g in groups {
                                    tbody { key: "{g.name}", class: "group",
                                        tr { class: "group-head",
                                            th { scope: "rowgroup",
                                                button { r#type: "button", class: "group-toggle",
                                                    "aria-expanded": !collapsed.read().contains(&g.name),
                                                    onclick: {
                                                        let name = g.name.clone();
                                                        move |_| {
                                                            let mut c = collapsed.write();
                                                            match c.iter().position(|n| *n == name) {
                                                                Some(i) => { c.remove(i); }
                                                                None => c.push(name.clone()),
                                                            }
                                                        }
                                                    },
                                                    Chevron { dir: if_open(!collapsed.read().contains(&g.name), "down", "right") }
                                                    " {words(&g.name)}"
                                                }
                                            }
                                            td { class: "num", {fmt(g.assigned)} }
                                            td { class: "num", {fmt(g.target)} }
                                            td { class: "num", {fmt(g.activity)} }
                                            td { class: "num", {fmt(g.available)} }
                                        }
                                        if !collapsed.read().contains(&g.name) {
                                            for (c, label, cat) in g.rows.into_iter().map(|(c, l)| { let k = c.category.clone(); (c, l, k) }) {
                                                CategoryLine { key: "{c.category}", row: c.clone(), label: label.clone(),
                                                    editing: editing() == Some(c.category.clone()),
                                                    value, notice, onedit: move |cat: String| editing.set(Some(cat)),
                                                    oncancel: move |()| editing.set(None),
                                                    onassign: move |amount: String| assign((cat.clone(), amount)),
                                                    onshow: move |cat: String| {
                                                        filter.set(Some(cat));
                                                        page.set("journal");
                                                    },
                                                    can_target,
                                                    ontarget: move |cat: String| target_for.set(Some(cat)),
                                                }
                                                if target_for() == Some(c.category.clone()) {
                                                    tr { key: "{c.category}-target", class: "editing",
                                                        td { colspan: 5,
                                                            TargetForm { row: c.clone(), notice,
                                                                oncancel: move |()| target_for.set(None),
                                                                onsave: {
                                                                    let (cat, date) = (c.category.clone(), env_date.clone());
                                                                    move |(amount, due): (String, Option<String>)| {
                                                                        let done = if amount == "0.00" {
                                                                            format!("Ended the target for {}.", short(&cat))
                                                                        } else {
                                                                            format!("Set the target for {}.", short(&cat))
                                                                        };
                                                                        let input = TargetInput {
                                                                            date: date.clone(),
                                                                            category: cat.clone(),
                                                                            amount,
                                                                            due,
                                                                            ..Default::default()
                                                                        };
                                                                        save((Operation::Add(EntryInput::Target(input)), done));
                                                                    }
                                                                },
                                                            }
                                                            OverspendingSwitch { row: c.clone(),
                                                                onsave: {
                                                                    let (cat, date) = (c.category.clone(), env_date.clone());
                                                                    move |rollover: bool| {
                                                                        let done = if rollover {
                                                                            format!("Overspending in {} now rolls over.", short(&cat))
                                                                        } else {
                                                                            format!("Overspending in {} now resets at month end.", short(&cat))
                                                                        };
                                                                        let input = OverspendingInput { date: date.clone(), category: cat.clone(), rollover };
                                                                        save((Operation::Add(EntryInput::Overspending(input)), done));
                                                                    }
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
                            p { class: "muted small",
                                "Red: overspent in cash. Amber: overspent on a card. Either comes out of next month's Unassigned, "
                                "or carries into next month on a category that rolls over. "
                                "Target: what the category's target put in this month, from Unassigned."
                            }
                        }
                        CardPayments { cards: view.cards.clone(), notice,
                            onsetaside: {
                                let date = env_date.clone();
                                move |(card, amount): (String, String)| {
                                    let done = format!("Set aside {amount} more for {}.", short(&card));
                                    let input = AssignInput { date: date.clone(), category: card, amount, currency: None };
                                    save((Operation::Add(EntryInput::Assign(input)), done));
                                }
                            },
                        }
                        Notes { items: view.flags.iter().chain(&view.warnings).cloned().collect::<Vec<_>>() }
                        MoveForm { categories: categories.clone(),
                            onmove: {
                                let date = env_date.clone();
                                move |(from, to, amount): (String, String, String)| {
                                    let done = format!("Moved {amount}.");
                                    let input = MoveInput { date: date.clone(), from, to, amount, currency: None };
                                    save((Operation::Add(EntryInput::Move(input)), done));
                                }
                            },
                        }
                        MonthEntries { entries: entries.clone(), categories: categories.clone(),
                            onedit: {
                                let core = core.clone();
                                move |(e, input): (Entry, EntryInput)| {
                                    let at = core.borrow().ref_of(&e);
                                    save((Operation::Edit(at, input), "Saved.".to_owned()));
                                }
                            },
                            ondelete: {
                                let core = core.clone();
                                move |e: Entry| {
                                    let at = core.borrow().ref_of(&e);
                                    save((Operation::Delete(at), "Deleted.".to_owned()));
                                }
                            },
                        }
                    } else if page() == "journal" {
                        Journal { backend: backend.clone(), core: Shared(core.clone()), version, notice,
                            month: shown, today: today, filter: filter(), last_account,
                            onclear: move |()| filter.set(None),
                        }
                    } else if page() == "scenarios" {
                        Scenarios { id: book_id.clone(), open: backend.scenario().map(str::to_owned), month: shown, notice,
                            onscenario: move |s: Option<String>| if let Some(h) = onscenario { h.call(s) },
                        }
                    } else if page() == "files" {
                        Files { backend: backend.clone(), core: Shared(core.clone()), version, notice }
                    } else if page() == "reports" {
                        Reports { core: Shared(core.clone()), version, month: shown }
                    } else {
                        Accounts { backend: backend.clone(), core: Shared(core.clone()), version, notice, today: today,
                            onshow: move |a: String| {
                                filter.set(Some(a));
                                page.set("journal");
                            },
                        }
                    }
                }
            }
        }
    }
}

/// One envelope's row; Assigned opens an editor that writes the difference as an assignment.
#[component]
fn CategoryLine(
    row: CategoryRow,
    label: String,
    editing: bool,
    value: Signal<String>,
    mut notice: Signal<Option<(bool, String)>>,
    onedit: EventHandler<String>,
    oncancel: EventHandler<()>,
    onassign: EventHandler<String>,
    onshow: EventHandler<String>,
    /// The Target cell opens the target editor (this month and later).
    can_target: bool,
    ontarget: EventHandler<String>,
) -> Element {
    let state = if row.cash_overspent > Decimal::ZERO {
        "cash"
    } else if row.credit_overspent > Decimal::ZERO {
        "credit"
    } else if row.available > Decimal::ZERO {
        "funded"
    } else {
        "zero"
    };
    let name = short(&row.category).to_owned();
    let assigned = row.assigned;
    rsx! {
        tr { class: "category", "data-category": "{row.category}",
            td { title: "{row.category}", "{label}"
                if row.rollover_overspending {
                    span { class: "badge", title: "Overspending carries into next month (rule 6a)", "rolls over" }
                }
            }
            td { class: "num",
                if editing {
                    form {
                        onsubmit: move |e: FormEvent| {
                            e.prevent_default();
                            // The cell shows the month's total; assigning writes the difference.
                            let total = match parse_amount(&value.peek()) {
                                Ok(total) => total,
                                Err(m) => return notice.set(Some((false, m))),
                            };
                            let delta = total - assigned;
                            if delta.is_zero() {
                                oncancel.call(());
                            } else {
                                onassign.call(format!("{:.2}", delta.round_dp(2)));
                            }
                        },
                        input { class: "amount", "aria-label": "Assigned to {name}", inputmode: "decimal",
                            value: "{value}",
                            oninput: move |e| value.set(e.value()),
                            onkeydown: move |e| if e.key() == Key::Escape { oncancel.call(()) },
                            onmounted: move |e| async move { let _ = e.set_focus(true).await; },
                        }
                    }
                } else {
                    button { r#type: "button", class: "cell", "aria-label": "Edit assigned for {name}",
                        onclick: {
                            let category = row.category.clone();
                            move |_| {
                                value.set(format!("{assigned:.2}"));
                                onedit.call(category.clone());
                            }
                        },
                        {fmt(row.assigned)}
                    }
                }
            }
            td { class: if row.shortfall > Decimal::ZERO { "num target warn" } else { "num target" },
                if can_target {
                    button { r#type: "button", class: "cell",
                        "aria-label": if row.target.is_some() { "Edit the target for {name}" } else { "Set a target for {name}" },
                        title: row.target.as_ref().map(target_rule).unwrap_or_default(),
                        onclick: {
                            let category = row.category.clone();
                            move |_| ontarget.call(category.clone())
                        },
                        if row.target.is_none() && row.target_allocated.is_zero() { "Set" } else { {fmt(row.target_allocated)} }
                    }
                } else {
                    {fmt(row.target_allocated)}
                }
            }
            td { class: "num",
                button { r#type: "button", class: "cell", "aria-label": "Transactions in {row.category}",
                    onclick: {
                        let category = row.category.clone();
                        move |_| onshow.call(category.clone())
                    },
                    {fmt(row.activity)}
                }
            }
            td { class: "num",
                span { class: "pill {state}", {fmt(row.available)} }
            }
        }
    }
}

#[cfg(test)]
mod save_tests {
    use std::path::PathBuf;

    use branchbooks_core::write::render::{AssignInput, EntryInput};
    use branchbooks_core::write::store::FileChange;
    use branchbooks_git::{Author, GitStore};

    use super::*;

    const LEDGER: &str = include_str!("../../../fixtures/examples-1-5.bean");

    fn book(name: &str) -> (Backend, RefCell<WorkspaceCore>, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("branchbooks-save-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        GitStore::init(&dir, None).unwrap();
        let author = Author {
            name: "test".into(),
            email: "test@example.invalid".into(),
        };
        let backend = Backend {
            dir: dir.clone(),
            author,
            scenario: None,
        };
        write(&backend, "", LEDGER, "seed");
        let core = RefCell::new(WorkspaceCore::new(None));
        let snap = backend.snapshot().unwrap();
        core.borrow_mut().reset(&snap.version, snap.files);
        (backend, core, dir)
    }

    fn write(backend: &Backend, base: &str, text: &str, op_id: &str) -> String {
        let file = FileChange {
            path: "main.beancount".into(),
            text: Some(text.into()),
        };
        let info = CommitInfo {
            op_id: op_id.into(),
            op_type: "test".into(),
            summary: op_id.into(),
        };
        backend.commit(base, &[file], &info).unwrap()
    }

    fn assign(amount: &str) -> Operation {
        Operation::Add(EntryInput::Assign(AssignInput {
            date: "2026-11-01".into(),
            category: "Expenses:Food:Groceries".into(),
            amount: amount.into(),
            currency: None,
        }))
    }

    fn assigned(core: &RefCell<WorkspaceCore>) -> String {
        let month = "2026-11".parse().unwrap();
        let view = core.borrow_mut().book().budget(month, None);
        let row = view.category("Expenses:Food:Groceries").unwrap().assigned;
        row.to_string()
    }

    #[test]
    fn a_mistyped_year_is_refused_before_anything_is_written() {
        let (backend, core, dir) = book("far-date");
        let before = core.borrow().version.clone();
        let far = Operation::Add(EntryInput::Assign(AssignInput {
            date: "9026-11-01".into(),
            category: "Expenses:Food:Groceries".into(),
            amount: "1.00".into(),
            currency: None,
        }));
        let e = apply(&backend, &core, far).unwrap_err().to_string();
        assert_eq!(e, "9026-11-01 is too far from today: check the year.");
        assert_eq!(core.borrow().version, before);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_save_commits_and_keeps_the_result() {
        let (backend, core, dir) = book("commit");
        let before = core.borrow().version.clone();
        apply(&backend, &core, assign("25.00")).unwrap();
        assert_ne!(core.borrow().version, before);
        assert_eq!(assigned(&core), "25.00");
        assert_eq!(backend.snapshot().unwrap().version, core.borrow().version);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_save_after_another_device_saved_reloads_and_keeps_both() {
        let (backend, core, dir) = book("conflict");
        let base = core.borrow().version.clone();
        let other = format!("{LEDGER}\n; from the other device\n");
        write(&backend, &base, &other, "other-device");
        apply(&backend, &core, assign("25.00")).unwrap();
        let snap = backend.snapshot().unwrap();
        assert!(snap.files["main.beancount"].contains("from the other device"));
        assert_eq!(snap.version, core.borrow().version);
        assert_eq!(assigned(&core), "25.00");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_refused_save_changes_nothing_and_says_why() {
        let (backend, core, dir) = book("refused");
        let before = backend.snapshot().unwrap().version;
        let e = apply(&backend, &core, assign("not a number")).unwrap_err();
        assert!(e.to_string().contains("invalid amount"), "{e}");
        assert_eq!(backend.snapshot().unwrap().version, before);
        assert_eq!(core.borrow().version, before);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
