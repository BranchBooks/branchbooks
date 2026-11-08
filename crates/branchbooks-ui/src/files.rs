//! Files: the ledger's errors, importing .beancount
//! files into this book (a preview first, then one commit), the book's history, and its files.

use branchbooks_core::write::import::ImportPreview;
use branchbooks_core::write::store::CommitInfo;
use branchbooks_core::write::workspace::{ApplyError, retry_on_conflict};
use dioxus::prelude::*;
use indexmap::IndexMap;

use crate::journal::Shared;
use crate::store::{Backend, new_id};

/// Commits an import the preview showed; the warnings it leaves.
fn import(
    backend: &Backend,
    core: &Shared,
    files: &IndexMap<String, String>,
) -> Result<String, ApplyError> {
    let op_id = new_id();
    // A sync can land while this runs, as with any save.
    let warnings = retry_on_conflict(2, |again| {
        if again {
            let snap = backend.snapshot()?;
            core.0.borrow_mut().reset(&snap.version, snap.files);
        }
        let (planned, base) = {
            let mut c = core.0.borrow_mut();
            let preview = c.preview_import(files);
            let planned = c.plan_import(&preview)?;
            (planned, c.version.clone())
        };
        let info = CommitInfo {
            op_id: op_id.clone(),
            op_type: planned.op_type.clone(),
            summary: planned.summary.clone(),
        };
        let version = backend.commit(&base, &planned.changes, &info)?;
        Ok(core.0.borrow_mut().commit_planned(planned, &version))
    })?;
    Ok(if warnings.is_empty() {
        "Imported.".to_owned()
    } else {
        let w: Vec<&str> = warnings.iter().map(|w| w.message.as_str()).collect();
        format!("Imported, but: {}", w.join("; "))
    })
}

/// The preview in words.
fn summary(p: &ImportPreview) -> Vec<String> {
    let mut out = vec![format!(
        "{} new entr{}; {} already in the book.",
        p.add.len(),
        if p.add.len() == 1 { "y" } else { "ies" },
        p.duplicates
    )];
    if !p.opens.is_empty() {
        out.push(format!("Opened on first use: {}.", p.opens.join(", ")));
    }
    if !p.ignored.is_empty() {
        out.push(format!("Kept this book's own: {}.", p.ignored.join(", ")));
    }
    out
}

#[component]
pub fn Files(
    backend: Backend,
    core: Shared,
    version: Signal<Option<String>>,
    notice: Signal<Option<(bool, String)>>,
) -> Element {
    let mut picked = use_signal(IndexMap::<String, String>::new);
    let mut shown = use_signal(|| None::<String>);
    let at: String = version().unwrap_or_default().chars().take(10).collect();
    let errors = core.0.borrow_mut().book().errors();
    let preview =
        (!picked.read().is_empty()).then(|| core.0.borrow_mut().preview_import(&picked.read()));
    let history = use_resource({
        let backend = backend.clone();
        move || {
            let backend = backend.clone();
            let _ = version();
            async move { backend.log(50) }
        }
    });
    let snapshot = use_resource({
        let backend = backend.clone();
        move || {
            let backend = backend.clone();
            let _ = version();
            async move { backend.snapshot() }
        }
    });
    let dir = backend.dir().to_path_buf();
    rsx! {
        section { "aria-label": "Ledger errors",
            h2 { "Ledger errors" }
            if errors.is_empty() {
                p { class: "muted", "None: every entry loads and every balance assertion holds." }
            } else {
                ul { class: "problems",
                    for d in errors.iter() {
                        li { "{d.file}:{d.line}: {d.message}" }
                    }
                }
            }
        }
        section { "aria-label": "Import",
            h2 { "Import Beancount files" }
            p { class: "muted small",
                "Adds what this book does not have yet, in date order, as one change. To bring in a GnuCash book, use Add book."
            }
            input { r#type: "file", multiple: true, accept: ".beancount,.bean",
                onchange: move |e: FormEvent| async move {
                    let mut files = IndexMap::new();
                    for f in e.files() {
                        match f.read_string().await {
                            Ok(text) => {
                                files.insert(f.name(), text);
                            }
                            Err(err) => notice.set(Some((false, format!("Could not read {}: {err}", f.name())))),
                        }
                    }
                    picked.set(files);
                },
            }
            if let Some(p) = &preview {
                if !p.errors.is_empty() {
                    div { class: "notice error", role: "alert",
                        p { "Nothing can be imported while the files have errors:" }
                        ul { for d in p.errors.iter() { li { "{d.file}:{d.line}: {d.message}" } } }
                    }
                } else {
                    for line in summary(p) { p { "{line}" } }
                    if !p.add.is_empty() {
                        button { r#type: "button",
                            onclick: {
                                let (backend, core) = (backend.clone(), core.clone());
                                move |_| {
                                    let (backend, core) = (backend.clone(), core.clone());
                                    let files = picked.peek().clone();
                                    let r = import(&backend, &core, &files);
                                    let ok = r.is_ok();
                                    notice.set(Some(match r {
                                        Ok(t) => (true, t),
                                        Err(e) => (false, e.to_string()),
                                    }));
                                    if ok {
                                        picked.set(IndexMap::new());
                                    }
                                    let v = core.0.borrow().version.clone();
                                    version.set(Some(v));
                                }
                            },
                            "Import {p.add.len()} entries"
                        }
                    }
                }
            }
        }
        section { "aria-label": "History",
            h2 { "History" }
            match &*history.read() {
                Some(Ok(log)) => rsx! {
                    table { class: "report",
                        thead { tr { th { "When" } th { "Who" } th { "What" } } }
                        tbody {
                            for c in log.iter() {
                                tr { key: "{c.version}",
                                    td { class: "date", {c.at.get(..16).unwrap_or(&c.at).replace('T', " ")} }
                                    td { "{c.author}" }
                                    td { title: c.op_type.clone().unwrap_or_default(), "{c.summary}" }
                                }
                            }
                        }
                    }
                },
                Some(Err(e)) => rsx! { p { class: "notice error", "{e}" } },
                None => rsx! { p { class: "muted", "Loading…" } },
            }
        }
        section { "aria-label": "Files",
            div { class: "section-head",
                h2 { "Files" }
                button { r#type: "button", class: "link",
                    onclick: move |_| open_folder(&dir),
                    "Open the book's folder"
                }
            }
            if let Some(Ok(snap)) = &*snapshot.read() {
                ul { class: "files",
                    for (path, text) in snap.files.iter() {
                        li { key: "{path}",
                            button { r#type: "button", class: "link",
                                onclick: {
                                    let path = path.clone();
                                    move |_| {
                                        let same = shown.peek().as_deref() == Some(path.as_str());
                                        shown.set((!same).then(|| path.clone()));
                                    }
                                },
                                "{path}"
                            }
                            span { class: "muted small-inline", " {text.lines().count()} lines" }
                            if shown().as_deref() == Some(path.as_str()) {
                                pre { class: "file-text", "{text}" }
                            }
                        }
                    }
                }
            }
            p { class: "muted small", "Version {at}" }
        }
    }
}

/// Shows the folder in the system's file manager.
fn open_folder(dir: &std::path::Path) {
    let program = if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(program).arg(dir).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_import_preview_reads_as_words() {
        let p = ImportPreview {
            root: "main.beancount".into(),
            add: Vec::new(),
            duplicates: 3,
            opens: vec!["Assets:Cash".into()],
            ignored: vec!["option \"title\"".into()],
            errors: Vec::new(),
        };
        assert_eq!(
            summary(&p),
            [
                "0 new entries; 3 already in the book.",
                "Opened on first use: Assets:Cash.",
                "Kept this book's own: option \"title\".",
            ]
        );
    }
}
