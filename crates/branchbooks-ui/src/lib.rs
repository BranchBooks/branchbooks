//! The `BranchBooks` app in Dioxus, calling branchbooks-core directly: books in git on the device
//! (local, or synced to a remote), with the Budget, Journal and Accounts views.
//!   tools\windows\build.ps1       the Windows app (`desktop`), into dist\windows
//!   dx bundle --platform android  the phone app (needs the Android SDK and a grit-lib patch)
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

pub mod accounts;
pub mod books;
pub mod budget;
pub mod charts;
pub mod envelopes;
pub mod files;
pub mod journal;
pub mod overview;
pub mod reports;
pub mod scenarios;
pub mod settings;
pub mod store;
pub mod text;
pub mod update;

use dioxus::prelude::*;

pub use budget::{Screen, ScreenProps};
use store::local_today;

/// The stylesheet, inline: a plain `cargo build` (no `dx`) bundles no asset files. A component of its
/// own, with no props, so it does not redraw with the app: a `Style` cannot change once mounted, and
/// Dioxus warns on every redraw otherwise.
#[component]
fn Stylesheet() -> Element {
    rsx! { document::Style { {include_str!("../assets/style.css")} } }
}

/// The logo: a ledger book whose right page holds a git branch, drawn for small sizes.
#[component]
pub fn Logo() -> Element {
    rsx! { span { class: "logo", "aria-hidden": "true", dangerous_inner_html: include_str!("../icons/icon-small.svg") } }
}

/// What shows while the app or a book loads, with anything to say under it.
#[component]
pub fn Splash(children: Element) -> Element {
    rsx! {
        main { class: "splash",
            Logo {}
            p { class: "muted", "Loading…" }
            {children}
        }
    }
}

/// Runs `f` off the UI thread: git and the network block.
async fn blocking<T: Send + 'static, E: std::fmt::Display + Send + 'static>(
    f: impl FnOnce() -> Result<T, E> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Adds `id` to the books waiting for a sync, once. The first asked goes first.
fn enqueue(waiting: &mut Vec<String>, id: String) {
    if !waiting.contains(&id) {
        waiting.push(id);
    }
}

/// Where a panic's message goes: a release build on Windows has no console to show it in.
pub(crate) fn crash_log() -> std::path::PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map_or_else(std::env::temp_dir, std::path::PathBuf::from)
        .join("BranchBooks")
        .join("crash.log")
}

/// Appends one crash to `path`.
fn write_crash(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{text}")
}

/// Keeps panics in a file, so a window that just closes can still be reported. Ledger text can appear
/// in a panic's message, and the file stays on this device.
pub fn keep_crash_log() {
    let shown = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let text = format!(
            "{} BranchBooks {}: {info}",
            chrono::Local::now().to_rfc3339(),
            env!("CARGO_PKG_VERSION")
        );
        // Nothing more can be done for a crash that cannot be written down.
        let _ = write_crash(&crash_log(), &text);
        shown(info);
    }));
}

/// The state of syncing, shared by the button, opening a book and saving.
#[derive(Clone, Copy)]
struct Syncer {
    running: Signal<bool>,
    /// Books asked for while a sync runs: each syncs after it.
    waiting: Signal<Vec<String>>,
    /// The book on screen: only its sync may change the status or reload the screen.
    shown: Signal<Option<String>>,
    status: Signal<Option<String>>,
    /// Bumped to load the open book again, after a sync brings changes in.
    loads: Signal<u32>,
}

impl Syncer {
    /// Syncs `id` in the background, one sync at a time.
    fn start(mut self, id: String) {
        if *self.running.peek() {
            enqueue(&mut self.waiting.write(), id);
            return;
        }
        self.running.set(true);
        spawn(async move {
            let mut id = id;
            loop {
                let i = id.clone();
                let done = blocking(move || books::sync(&i)).await;
                if self.shown.peek().as_deref() == Some(id.as_str()) {
                    match done {
                        Ok((changed, text)) => {
                            self.status.set(Some(text));
                            if changed {
                                self.loads += 1;
                            }
                        }
                        Err(e) => self.status.set(Some(format!("Not synced: {e}"))),
                    }
                }
                if self.waiting.peek().is_empty() {
                    break;
                }
                id = self.waiting.write().remove(0);
            }
            self.running.set(false);
        });
    }
}

/// The books on this device, one open at a time, with a switcher, Add book, the book's Settings
/// and, for a book with a remote, Sync: by button, when the book opens and after each save.
#[component]
pub fn App() -> Element {
    // Checked each minute: an app left open overnight must not keep dating entries yesterday.
    let mut today = use_signal(local_today);
    use_future(move || async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            let now = local_today();
            if *today.peek() != now {
                today.set(now);
            }
        }
    });
    // Bumped to read the book list again, after a book is made, changed or deleted.
    let mut listed = use_signal(|| 0u32);
    let books = use_resource(move || async move {
        let _ = listed();
        blocking(books::list).await
    });
    // Offline is normal, and an update that fails to arrive is tried again at the next start, or
    // from Settings.
    let mut update = use_resource(|| blocking(update::download));
    // How the app looks, as this device last chose it.
    use_effect(|| settings::apply_theme(&settings::load_theme()));
    let mut open = use_signal(|| None::<String>);
    let mut adding = use_signal(|| false);
    let mut settings = use_signal(|| false);
    // The scenario open in the book, if any.
    let mut scenario = use_signal(|| None::<String>);
    let mut sync = Syncer {
        running: use_signal(|| false),
        waiting: use_signal(Vec::new),
        shown: use_signal(|| None),
        status: use_signal(|| None),
        loads: use_signal(|| 0),
    };

    // The open book: the one chosen, else the first; none on a new device.
    let book = match &*books.read() {
        Some(Ok(list)) => open()
            .and_then(|id| list.iter().find(|b| b.id == id).cloned())
            .or_else(|| list.first().cloned()),
        _ => None,
    };
    // Sync a book with a remote when it opens (before any return: hooks run in the same order).
    let opened = book.as_ref().map(|b| (b.id.clone(), b.remote));
    use_effect(use_reactive!(|opened| {
        sync.shown.set(opened.as_ref().map(|(id, _)| id.clone()));
        if let Some((id, true)) = opened {
            sync.start(id);
        }
    }));
    let list = match &*books.read() {
        None => {
            return rsx! { Stylesheet {} Splash {} };
        }
        Some(Err(e)) => {
            return rsx! { Stylesheet {} main { class: "shell", p { class: "notice error", "{e}" } } };
        }
        Some(Ok(list)) => list.clone(),
    };
    let Some(book) = book.filter(|_| !adding()) else {
        let done = move |made: Option<(String, String)>| {
            if let Some((id, said)) = made {
                scenario.set(None);
                open.set(Some(id));
                sync.status.set(Some(said));
            }
            adding.set(false);
            listed += 1;
        };
        return rsx! { Stylesheet {} NewBook { first: list.is_empty(), ondone: done } };
    };
    if settings() {
        let done = move |(deleted, said): (bool, Option<String>)| {
            if deleted {
                open.set(None);
            }
            settings.set(false);
            sync.status.set(said);
            listed += 1;
            sync.loads += 1;
        };
        let (update_text, update_ready) = match &*update.read() {
            None => ("Checking for updates…".to_owned(), None),
            Some(Ok(Some(v))) => (
                format!("Version {v} is downloaded and ready."),
                Some(v.clone()),
            ),
            Some(Ok(None)) => ("No update is waiting.".to_owned(), None),
            Some(Err(e)) => (format!("Could not check for updates: {e}"), None),
        };
        return rsx! {
            Stylesheet {}
            settings::Settings { book_name: book.name.clone(), update_text, update_ready,
                busy: update.read().is_none() || (sync.running)(),
                oncheck: move |()| update.restart(),
                onrestart: move |()| {
                    if let Err(e) = update::restart() {
                        sync.status.set(Some(format!("The update did not start: {e}")));
                    }
                },
                onback: move |()| settings.set(false),
                BookSettings { id: book.id.clone(), ondone: done }
            }
        };
    }
    let (id, remote) = (book.id.clone(), book.remote);
    rsx! {
        Screen { key: "{book.id}-{scenario().unwrap_or_default()}", reloads: (sync.loads)(),
            backend: books::backend(&book.id, scenario().as_deref()), today: today(),
            onscenario: move |s: Option<String>| scenario.set(s),
            onsettings: move |()| settings.set(true),
            onsaved: {
                let id = id.clone();
                move |()| if remote { sync.start(id.clone()) }
            },
            label { class: "muted small-inline", "Book"
                select { "aria-label": "Book",
                    onchange: move |e| {
                        sync.status.set(None);
                        scenario.set(None);
                        open.set(Some(e.value()));
                    },
                    for b in list {
                        option { key: "{b.id}", value: "{b.id}", selected: b.id == book.id, "{b.name}" }
                    }
                }
            }
            button { r#type: "button", class: "link", onclick: move |_| adding.set(true), "Add book" }
            if remote {
                button { r#type: "button", class: "link", disabled: (sync.running)(),
                    onclick: move |_| sync.start(id.clone()),
                    if (sync.running)() { "Syncing…" } else { "Sync" }
                }
            }
            if let Some(s) = (sync.status)() {
                span { class: "muted small-inline", role: "status", "{s}" }
            }
            if let Some(Ok(Some(version))) = &*update.read() {
                // Not during a sync: restarting ends the process, and the sync with it.
                button { r#type: "button", class: "link", disabled: (sync.running)(),
                    onclick: move |_| {
                        if let Err(e) = update::restart() {
                            sync.status.set(Some(format!("The update did not start: {e}")));
                        }
                    },
                    "Restart to update to {version}"
                }
            }
        }
        Stylesheet {}
    }
}

/// One book's settings: its name, its remote and token, who commits to it, and deleting it.
/// `ondone`: whether the book was deleted, and what was done, to tell the person.
#[component]
fn BookSettings(id: String, ondone: EventHandler<(bool, Option<String>)>) -> Element {
    let now = use_hook({
        let id = id.clone();
        move || books::settings(&id)
    });
    let mut name = use_signal(|| now.name.clone());
    let mut url = use_signal(|| now.remote.clone().unwrap_or_default());
    let mut token = use_signal(String::new);
    let mut who = use_signal(|| now.author_name.clone());
    let mut email = use_signal(|| now.author_email.clone());
    let mut error = use_signal(|| None::<String>);
    let mut deleting = use_signal(|| false);
    let mut replacing = use_signal(|| false);
    let save = {
        let id = id.clone();
        move |e: FormEvent| {
            e.prevent_default();
            let (id, n, u, t, w, m) = (
                id.clone(),
                name.peek().clone(),
                url.peek().clone(),
                token.peek().clone(),
                who.peek().clone(),
                email.peek().clone(),
            );
            spawn(async move {
                let r = blocking(move || {
                    books::rename(&id, &n)?;
                    books::set_author(&id, &w, &m)?;
                    books::set_remote(&id, &u, &t)
                })
                .await;
                match r {
                    Ok(()) => ondone.call((false, None)),
                    Err(e) => error.set(Some(e)),
                }
            });
        }
    };
    let delete = {
        let id = id.clone();
        move |_| {
            let id = id.clone();
            spawn(async move {
                match blocking(move || books::delete(&id)).await {
                    Ok(()) => ondone.call((true, None)),
                    Err(e) => error.set(Some(e)),
                }
            });
        }
    };
    rsx! {
        section { "aria-label": "This book",
            h2 { "This book" }
            form { class: "user-form", onsubmit: save,
                label { "Name"
                    input { value: "{name}", required: true, oninput: move |e| name.set(e.value()) }
                }
                label { "Remote (HTTPS URL; blank: this device only)"
                    input { r#type: "url", value: "{url}", placeholder: "https://git.example/you/book.git",
                        oninput: move |e| url.set(e.value()),
                    }
                }
                label { "Access token (blank: keep the saved one)"
                    input { r#type: "password", autocomplete: "off", value: "{token}",
                        oninput: move |e| token.set(e.value()),
                    }
                }
                label { "Commits by (name)"
                    input { value: "{who}", oninput: move |e| who.set(e.value()) }
                }
                label { "Commits by (email)"
                    input { r#type: "email", value: "{email}", oninput: move |e| email.set(e.value()) }
                }
                p { class: "muted small",
                    "Name and email are kept in this book's own git config; both blank uses git's global identity."
                }
                if let Some(e) = error() {
                    p { class: "field-error", role: "alert", "{e}" }
                }
                div { class: "editor-actions",
                    button { r#type: "submit", "Save" }
                    button { r#type: "button", class: "link", onclick: move |_| ondone.call((false, None)), "Cancel" }
                }
            }
            if now.remote.is_some() {
                div { class: "settings-part", "aria-label": "Replace the remote",
                    if replacing() {
                        p {
                            "Make this book the remote's? The remote's old book stays in its history and its scenarios and "
                            "closed months are set aside there. Other devices take this book at their next sync."
                        }
                        button { r#type: "button", class: "danger",
                            onclick: {
                                let id = id.clone();
                                move |_| {
                                    let id = id.clone();
                                    spawn(async move {
                                        match blocking(move || books::replace_remote(&id)).await {
                                            Ok(()) => ondone.call((false, Some("This book is now the remote's; its old history is set aside there.".into()))),
                                            Err(e) => error.set(Some(e)),
                                        }
                                    });
                                }
                            },
                            "Replace the remote"
                        }
                        button { r#type: "button", class: "link", onclick: move |_| replacing.set(false), "Keep" }
                    } else {
                        button { r#type: "button", class: "link", onclick: move |_| replacing.set(true), "Replace the remote with this book" }
                        p { class: "muted small", "For a book whose history is separate from the remote's, which sync will not merge." }
                    }
                }
            }
            div { class: "settings-part", "aria-label": "Delete this book",
                if deleting() {
                    p { "Delete {now.name}? It moves to .deleted in the books folder, so it can be restored by hand." }
                    button { r#type: "button", class: "danger", onclick: delete, "Delete" }
                    button { r#type: "button", class: "link", onclick: move |_| deleting.set(false), "Keep" }
                } else {
                    button { r#type: "button", class: "link", onclick: move |_| deleting.set(true), "Delete this book" }
                }
            }
        }
    }
}
/// Adds a book: one already kept in git, opened on this device as well, or a new one, empty, the
/// sample ledger or a `GnuCash` file. `first`: this device has no books yet, so there is no Cancel.
/// `ondone`: the book's id and what was done, or None when cancelled.
#[component]
fn NewBook(first: bool, ondone: EventHandler<Option<(String, String)>>) -> Element {
    use books::Start;
    // "git" opens a book from a remote; "new" starts one.
    let mut mode = use_signal(|| "git");
    let mut name = use_signal(String::new);
    let mut start = use_signal(|| "sample".to_owned());
    let mut url = use_signal(String::new);
    let mut token = use_signal(String::new);
    let mut currency = use_signal(|| "USD".to_owned());
    // The GnuCash file picked: its name and bytes.
    let mut gnucash = use_signal(|| None::<(String, Vec<u8>)>);
    let mut error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let git = mode() == "git";
    let token_label = if books::keeps_tokens() {
        "Access token, for a private remote (kept in Windows Credential Manager)"
    } else {
        "Access token (this device cannot keep one yet: leave it empty for a public remote)"
    };
    rsx! {
        main { class: "shell start",
            if first {
                h1 { "Welcome to BranchBooks" }
                p { class: "muted", "Open a book you already keep in git, to use it on this device too, or start a new one." }
            } else {
                h1 { "Add a book" }
            }
            div { class: "segmented", role: "group", "aria-label": "Add",
                button { r#type: "button", "aria-pressed": git, onclick: move |_| mode.set("git"), "Open a book from git" }
                button { r#type: "button", "aria-pressed": !git, onclick: move |_| mode.set("new"), "Start a new book" }
            }
            form { class: "user-form",
                onsubmit: move |e: FormEvent| {
                    e.prevent_default();
                    let n = name.peek().clone();
                    let s = match (*mode.peek(), start.peek().as_str()) {
                        ("git", _) => Start::Clone { url: url.peek().clone(), token: token.peek().clone() },
                        (_, "empty") => Start::Empty { currency: currency.peek().trim().to_owned() },
                        (_, "gnucash") => match gnucash.peek().as_ref() {
                            Some((_, bytes)) => Start::GnuCash(bytes.clone()),
                            None => return error.set(Some("Pick the GnuCash file.".into())),
                        },
                        _ => Start::Sample,
                    };
                    busy.set(true);
                    error.set(None);
                    spawn(async move {
                        match blocking(move || books::create(&n, s)).await {
                            Ok(made) => ondone.call(Some(made)),
                            Err(e) => {
                                busy.set(false);
                                error.set(Some(e));
                            }
                        }
                    });
                },
                if git {
                    label { "Remote (HTTPS URL)"
                        input { r#type: "url", value: "{url}", required: true,
                            placeholder: "https://git.example/you/book.git",
                            oninput: move |e| url.set(e.value()),
                        }
                    }
                    label { "{token_label}"
                        input { r#type: "password", autocomplete: "off", value: "{token}",
                            oninput: move |e| token.set(e.value()),
                        }
                    }
                    label { "Name on this device"
                        input { value: "{name}", placeholder: "{books::name_from_url(&url())}",
                            oninput: move |e| name.set(e.value()),
                        }
                    }
                    p { class: "muted small", "The book's history comes down from the remote, and it syncs from then on, like the other devices." }
                } else {
                    label { "Name"
                        input { value: "{name}", required: true, oninput: move |e| name.set(e.value()) }
                    }
                    label { "Start with"
                        select { value: "{start}", onchange: move |e| start.set(e.value()),
                            option { value: "sample", "The sample ledger" }
                            option { value: "empty", "Nothing" }
                            option { value: "gnucash", "A GnuCash file (full history)" }
                        }
                    }
                }
                if !git && start() == "gnucash" {
                    label { "GnuCash file (.gnucash, saved as XML)"
                        input { r#type: "file", accept: ".gnucash,.xml,.gz",
                            onchange: move |e: FormEvent| async move {
                                let Some(f) = e.files().into_iter().next() else { return };
                                match f.read_bytes().await {
                                    Ok(bytes) => gnucash.set(Some((f.name(), bytes.to_vec()))),
                                    Err(err) => error.set(Some(format!("Could not read {}: {err}", f.name()))),
                                }
                            },
                        }
                    }
                    if let Some((file, bytes)) = gnucash() {
                        p { class: "muted small", "{file}: {bytes.len() / 1024} KB. Accounts and every transaction come over; budget assignments start fresh." }
                    }
                }
                if !git && start() == "empty" {
                    label { "Currency"
                        input { value: "{currency}", required: true, oninput: move |e| currency.set(e.value()) }
                    }
                }
                if let Some(e) = error() {
                    p { class: "field-error", role: "alert", "{e}" }
                }
                div { class: "editor-actions",
                    button { r#type: "submit", disabled: busy(),
                        match (busy(), git) {
                            (true, true) => "Opening…",
                            (true, false) => "Creating…",
                            (false, true) => "Open",
                            (false, false) => "Create",
                        }
                    }
                    if !first {
                        button { r#type: "button", class: "link", onclick: move |_| ondone.call(None), "Cancel" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn books_waiting_for_a_sync_keep_their_order_and_wait_once() {
        let mut waiting = Vec::new();
        for id in ["b", "c", "b"] {
            enqueue(&mut waiting, id.to_owned());
        }
        assert_eq!(waiting, ["b", "c"]);
    }

    #[test]
    fn crashes_are_appended_to_the_log() {
        let dir = std::env::temp_dir().join(format!("branchbooks-crash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let log = dir.join("BranchBooks").join("crash.log");
        write_crash(&log, "first").unwrap();
        write_crash(&log, "second").unwrap();
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "first\nsecond\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
