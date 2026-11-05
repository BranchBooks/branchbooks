//! Screens drawn to HTML in plain `cargo test`, without a browser: what a view shows for a given
//! book. Clicking and typing are checked by hand on Windows until there is a desktop test driver.

use std::path::PathBuf;

use branchbooks_core::write::store::{CommitInfo, FileChange};
use branchbooks_ui::store::Backend;
use std::cell::RefCell;
use std::rc::Rc;

use branchbooks_core::write::store::Store;
use branchbooks_core::write::workspace::WorkspaceCore;
use branchbooks_ui::accounts::Accounts;
use branchbooks_ui::journal::{Journal, Shared};
use branchbooks_ui::overview::Overview;
use branchbooks_ui::reports::Reports;
use branchbooks_ui::{Screen, ScreenProps};
use dioxus::prelude::*;

const SAMPLE: &str = include_str!("../../../fixtures/examples-1-5.bean");

/// A book in a fresh git repository, holding `ledger`.
fn book(name: &str, ledger: &str) -> (Backend, PathBuf) {
    let dir = std::env::temp_dir().join(format!("branchbooks-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    branchbooks_git::GitStore::init(&dir, None).unwrap();
    let author = branchbooks_git::Author {
        name: "test".into(),
        email: "test@example.invalid".into(),
    };
    let backend = Backend {
        dir: dir.clone(),
        author,
        scenario: None,
    };
    let file = FileChange {
        path: "main.beancount".into(),
        text: Some(ledger.into()),
    };
    let info = CommitInfo {
        op_id: "seed".into(),
        op_type: "import".into(),
        summary: "Seed".into(),
    };
    backend.commit("", &[file], &info).unwrap();
    (backend, dir)
}

/// The screen's HTML once its book has loaded.
async fn render(props: ScreenProps) -> String {
    let mut dom = VirtualDom::new_with_props(Screen, props);
    dom.rebuild_in_place();
    // Loading is a future; let it finish (a folder book is ready at once).
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), dom.wait_for_work()).await;
    dom.render_immediate(&mut dioxus_core::NoOpMutations);
    dioxus_ssr::render(&dom)
}

/// The text of the element `<tag ... marker ...>`: the attribute that picks it out, then its text.
fn text_after(html: &str, marker: &str) -> String {
    let at = html
        .find(marker)
        .unwrap_or_else(|| panic!("no {marker} in {html}"));
    let rest = &html[at..];
    let start = rest.find('>').unwrap() + 1;
    rest[start..start + rest[start..].find('<').unwrap()].to_owned()
}

#[tokio::test(flavor = "current_thread")]
async fn the_budget_shows_unassigned_and_marks_overspent_envelopes() {
    let (backend, dir) = book("budget", SAMPLE);
    let html = render(ScreenProps {
        backend,
        today: "2026-10-27".parse().unwrap(),
        children: VNode::empty(),
        onsaved: None,
        page: Some("budget"),
        onscenario: None,
    })
    .await;
    assert_eq!(text_after(&html, r#"data-testid="unassigned""#), "1,100.00");
    assert_eq!(text_after(&html, r#"class="month-label""#), "October 2026");
    // Groceries is 20 cash-overspent (Example 4): red.
    let groceries = &html[html
        .find(r#"data-category="Expenses:Food:Groceries""#)
        .unwrap()..];
    assert_eq!(text_after(groceries, r#"class="pill cash""#), "-20.00");
    // October's three assignments (Example 2), the card, Move money, and targets settable
    // (October is the current month here).
    assert_eq!(html.matches(r#"data-testid="envelope-entry""#).count(), 3);
    assert!(html.contains("Assign 600.00 to Food:Groceries"));
    assert!(html.contains(r#"data-card="Liabilities:CreditCard""#));
    assert!(html.contains("Move money") && html.contains("Set a target for Food:Dining"));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The Journal for October in a book loaded the way Screen loads it.
#[component]
fn OctoberJournal(backend: Backend, filter: Option<String>) -> Element {
    let core = use_hook(|| {
        let dir = &backend.dir;
        let snap = branchbooks_git::GitStore::new(dir, None)
            .snapshot()
            .unwrap();
        let mut core = WorkspaceCore::new(None);
        core.reset(&snap.version, snap.files);
        Shared(Rc::new(RefCell::new(core)))
    });
    let version = use_signal(|| Some(String::new()));
    let notice = use_signal(|| None);
    rsx! {
        Journal { backend, core, version, notice, month: "2026-10".parse().unwrap(), today: "2026-10-27".parse().unwrap(), filter, onclear: |()| {}, last_account: use_signal(String::new) }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn the_journal_lists_the_month_newest_first_with_postings_as_booked() {
    let (backend, dir) = book("journal", SAMPLE);
    let mut dom = VirtualDom::new_with_props(
        OctoberJournal,
        OctoberJournalProps {
            backend,
            filter: None,
        },
    );
    dom.rebuild_in_place();
    let html = dioxus_ssr::render(&dom);
    let dates: Vec<String> = html
        .match_indices(r#"class="date""#)
        .map(|(at, _)| text_after(&html[at..], r#"class="date""#))
        .collect();
    let mut sorted = dates.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert!(dates.len() > 2 && dates == sorted, "{dates:?}");
    assert!(html.contains("Paycheck"));
    // The add row offers the categories and the accounts money comes from.
    assert!(html.contains(r#"value="Expenses:Food:Dining""#));
    assert!(html.contains(r#"value="Assets:Checking""#));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[component]
fn AccountsOf(backend: Backend) -> Element {
    let core = use_hook(|| {
        let dir = &backend.dir;
        let snap = branchbooks_git::GitStore::new(dir, None)
            .snapshot()
            .unwrap();
        let mut core = WorkspaceCore::new(None);
        core.reset(&snap.version, snap.files);
        Shared(Rc::new(RefCell::new(core)))
    });
    let version = use_signal(|| Some(String::new()));
    let notice = use_signal(|| None);
    rsx! {
        Accounts { backend, core, version, notice, today: "2026-11-02".parse().unwrap(), onshow: |_| {} }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn accounts_show_balances_by_type_with_credits_positive() {
    let (backend, dir) = book("accounts", SAMPLE);
    let mut dom = VirtualDom::new_with_props(AccountsOf, AccountsOfProps { backend });
    dom.rebuild_in_place();
    let html = dioxus_ssr::render(&dom);
    // Example 14 as of Nov 2: checking 2,735, brokerage 500; income 5,000 shown positive.
    let row = |a: &str| {
        let at = html.find(&format!(r#"title="{a}""#)).unwrap();
        text_after(&html[at..], r#"class="num""#)
    };
    assert_eq!(row("Assets:Checking"), "2,735.00");
    assert_eq!(row("Assets:Brokerage"), "500.00");
    assert_eq!(row("Income:Salary"), "5,000.00");
    assert!(html.contains("Expense categories") && html.contains("on budget"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn an_accounts_transactions_are_its_journal_filtered_with_the_months_change() {
    let (backend, dir) = book("journal-filter", SAMPLE);
    let props = OctoberJournalProps {
        backend,
        filter: Some("Expenses:Food:Groceries".into()),
    };
    let mut dom = VirtualDom::new_with_props(OctoberJournal, props);
    dom.rebuild_in_place();
    let html = dioxus_ssr::render(&dom);
    // Examples 3 and 4: groceries 120 and 500 in October, and nothing else.
    assert_eq!(html.matches(r#"data-testid="txn""#).count(), 2, "{html}");
    assert!(html.contains("Show all"));
    assert_eq!(
        text_after(&html, r#"data-testid="filter-total""#),
        "Change this month: 620.00"
    );
    assert_eq!(html.matches(r#"class="hit""#).count(), 2);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[component]
fn ReportsOf(backend: Backend) -> Element {
    let core = use_hook(|| {
        let dir = &backend.dir;
        let snap = branchbooks_git::GitStore::new(dir, None)
            .snapshot()
            .unwrap();
        let mut core = WorkspaceCore::new(None);
        core.reset(&snap.version, snap.files);
        Shared(Rc::new(RefCell::new(core)))
    });
    let version = use_signal(|| Some(String::new()));
    rsx! { Reports { core, version, month: "2026-10".parse().unwrap() } }
}

#[tokio::test(flavor = "current_thread")]
async fn reports_show_example_14() {
    let (backend, dir) = book("reports", SAMPLE);
    let mut dom = VirtualDom::new_with_props(ReportsOf, ReportsOfProps { backend });
    dom.rebuild_in_place();
    let html = dioxus_ssr::render(&dom);
    // October: income 5,000, expenses 1,765 (tax 800, 401k 300, groceries 620, dining 45).
    assert_eq!(text_after(&html, r#"data-testid="net-income""#), "3,235.00");
    assert_eq!(text_after(&html, r#"data-testid="net-worth""#), "3,235.00");
    assert!(html.contains("Spending by category") && html.contains("<svg"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[component]
fn OverviewOf(backend: Backend) -> Element {
    let core = use_hook(|| {
        let dir = &backend.dir;
        let snap = branchbooks_git::GitStore::new(dir, None)
            .snapshot()
            .unwrap();
        let mut core = WorkspaceCore::new(None);
        core.reset(&snap.version, snap.files);
        Shared(Rc::new(RefCell::new(core)))
    });
    let version = use_signal(|| Some(String::new()));
    rsx! { Overview { core, version, today: "2026-11-02".parse().unwrap(), onopen: |_| {} } }
}

#[tokio::test(flavor = "current_thread")]
async fn the_overview_shows_where_things_stand() {
    let (backend, dir) = book("overview", SAMPLE);
    let mut dom = VirtualDom::new_with_props(OverviewOf, OverviewOfProps { backend });
    dom.rebuild_in_place();
    let html = dioxus_ssr::render(&dom);
    // Example 14 as of Nov 2: net worth 3,235 (checking 2,735, brokerage 500); the state after
    // Example 5: Unassigned 1,100 and Groceries cash-overspent by 20.
    assert_eq!(text_after(&html, r#"data-testid="net-worth""#), "3,235.00");
    assert_eq!(text_after(&html, r#"data-testid="unassigned""#), "1,100.00");
    assert!(
        html.contains("Needs attention") && html.contains("is overspent by 20.00"),
        "{html}"
    );
    assert!(html.contains("Move to brokerage"));
    std::fs::remove_dir_all(&dir).unwrap();
}
