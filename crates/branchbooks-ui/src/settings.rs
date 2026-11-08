//! Settings: this book (in `BookSettings`), how the app looks, updates and about, and where the
//! books and the crash log are kept. How the app looks is this device's, kept beside its books.

use std::path::PathBuf;

use dioxus::prelude::*;

use crate::store::books_dir;

/// The choices for how the app looks: the value kept, and its label.
const THEMES: [(&str, &str); 3] = [
    ("system", "Match Windows"),
    ("light", "Light"),
    ("dark", "Dark"),
];

const SITE: &str = "https://branchbooks.dev";
const REPO: &str = "https://gitlab.com/branchbooks/branchbooks";

/// Where this device keeps how the app looks: next to the books folder.
fn theme_file() -> Option<PathBuf> {
    books_dir().parent().map(|d| d.join("theme"))
}

/// How the app looks on this device: "system", "light" or "dark".
pub fn load_theme() -> String {
    theme_file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .map(|t| t.trim().to_owned())
        .filter(|t| THEMES.iter().any(|(v, _)| v == t))
        .unwrap_or_else(|| "system".to_owned())
}

/// Shows `theme` now; the stylesheet reads it from the root's `data-theme`.
pub fn apply_theme(theme: &str) {
    // Only one of THEMES ever reaches here, so it is safe in the script.
    let theme = THEMES
        .iter()
        .find(|(v, _)| *v == theme)
        .map_or("system", |(v, _)| v);
    document::eval(&format!(
        "document.documentElement.dataset.theme = '{theme}';"
    ));
}

/// Opens a folder or web address with Windows' own handler: Explorer, or the default browser.
fn open_outside(target: &str) -> Result<(), String> {
    if cfg!(windows) {
        // Explorer's exit code says nothing useful, so only failing to start counts.
        std::process::Command::new("explorer")
            .arg(target)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Could not open {target}: {e}"))
    } else {
        Err(format!("Open {target} yourself on this device."))
    }
}

/// The settings page. Its children are this book's settings; the rest is this device's.
#[component]
pub fn Settings(
    book_name: String,
    /// What the last update check found, in words.
    update_text: String,
    /// The version downloaded and ready to restart into, if any.
    update_ready: Option<String>,
    /// Checking now, or restarting, is under way: the buttons wait.
    busy: bool,
    oncheck: EventHandler<()>,
    onrestart: EventHandler<()>,
    onback: EventHandler<()>,
    children: Element,
) -> Element {
    let mut theme = use_signal(load_theme);
    let mut problem = use_signal(|| None::<String>);
    let books = books_dir();
    let crash = crate::crash_log();
    let mut open = move |target: String| {
        problem.set(open_outside(&target).err());
    };
    rsx! {
        main { class: "shell settings",
            button { r#type: "button", class: "link back", onclick: move |_| onback.call(()), "← Back to {book_name}" }
            h1 { "Settings" }
            if let Some(p) = problem() {
                p { class: "notice error", role: "alert", "{p}" }
            }
            {children}
            section { "aria-label": "Appearance",
                h2 { "Appearance" }
                div { class: "segmented", role: "radiogroup", "aria-label": "Theme",
                    for (value, label) in THEMES {
                        button { key: "{value}", r#type: "button", role: "radio", "aria-checked": theme() == value,
                            "aria-pressed": theme() == value,
                            onclick: move |_| {
                                theme.set(value.to_owned());
                                apply_theme(value);
                                if let Some(f) = theme_file() {
                                    let saved = f.parent().map_or(Ok(()), std::fs::create_dir_all)
                                        .and_then(|()| std::fs::write(&f, value));
                                    problem.set(saved.err().map(|e| format!("The theme is used, but not kept for next time: {e}")));
                                }
                            },
                            {label}
                        }
                    }
                }
                p { class: "muted small", "Kept on this device." }
            }
            section { "aria-label": "Updates and about",
                h2 { "Updates and about" }
                p { "BranchBooks {env!(\"CARGO_PKG_VERSION\")}" }
                p { class: "muted", role: "status", "{update_text}" }
                div { class: "editor-actions",
                    if let Some(v) = update_ready {
                        button { r#type: "button", disabled: busy, onclick: move |_| onrestart.call(()), "Restart to update to {v}" }
                    } else {
                        button { r#type: "button", disabled: busy, onclick: move |_| oncheck.call(()), "Check for updates" }
                    }
                }
                ul { class: "links",
                    for (label, url) in [
                        ("Website and help", SITE.to_owned()),
                        ("What's new", format!("{REPO}/-/releases")),
                        ("Report a problem", format!("{REPO}/-/issues")),
                        ("Licence (GPL-3.0)", format!("{REPO}/-/blob/main/LICENSE")),
                        ("Trademark policy", format!("{REPO}/-/blob/main/TRADEMARKS.md")),
                    ] {
                        li { key: "{label}",
                            button { r#type: "button", class: "link", onclick: move |_| open(url.clone()), {label} }
                        }
                    }
                }
            }
            section { "aria-label": "Data and backups",
                h2 { "Data and backups" }
                p { "Each book is a git repository in this folder. Back the folder up, or sync each book to a remote." }
                p { class: "path", code { "{books.display()}" } }
                button { r#type: "button", class: "link", onclick: {
                    let books = books.display().to_string();
                    move |_| open(books.clone())
                }, "Open the books folder" }
                p { class: "muted small", "If BranchBooks closes unexpectedly, what happened is written here. It stays on this device, and can hold ledger text." }
                p { class: "path", code { "{crash.display()}" } }
                if crash.exists() {
                    button { r#type: "button", class: "link", onclick: {
                        let dir = crash.parent().map(|d| d.display().to_string()).unwrap_or_default();
                        move |_| open(dir.clone())
                    }, "Open its folder" }
                } else {
                    p { class: "muted small", "Nothing recorded." }
                }
            }
        }
    }
}
