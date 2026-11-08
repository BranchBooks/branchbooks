//! Loads a ledger and its includes through an injected file source, so the core has no dependency
//! on a file system or any storage backend.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use indexmap::IndexMap;

use crate::parser::{OptionLine, ParseResult, parse};
use crate::types::{Diagnostic, Entry};

pub trait FileSource {
    /// Returns the file's text, or `None` when it does not exist. Paths use forward slashes.
    fn read(&self, path: &str) -> Option<String>;
}

/// Options by key, in the order first written, each with every value given.
pub type Options = IndexMap<String, Vec<String>>;

#[derive(Clone, Debug, Default)]
pub struct LoadResult {
    pub root: String,
    pub files: Vec<String>,
    /// Every entry in global file order: includes are expanded depth-first at the include line.
    pub entries: Vec<Entry>,
    /// Options from the root file only (see docs/DESIGN.md).
    pub options: Options,
    pub plugins: Vec<OptionLine>,
    pub errors: Vec<Diagnostic>,
}

pub fn dirname(path: &str) -> &str {
    match path.rfind('/') {
        None => "",
        Some(0) => "/",
        Some(i) => &path[..i],
    }
}

pub fn join_path(dir: &str, rel: &str) -> String {
    let b = rel.as_bytes();
    let drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/';
    if rel.starts_with('/') || drive {
        return normalize(rel);
    }
    if dir.is_empty() {
        normalize(rel)
    } else {
        normalize(&format!("{dir}/{rel}"))
    }
}

fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let lead = if path.starts_with('/') { "/" } else { "" };
    for p in path.split('/') {
        if p.is_empty() || p == "." {
            continue;
        }
        if p == ".." && parts.last().is_some_and(|l| *l != "..") {
            parts.pop();
        } else {
            parts.push(p);
        }
    }
    format!("{lead}{}", parts.join("/"))
}

/// Parse results keyed by path, reused while a file's text is unchanged (the write path reloads
/// often).
pub type ParseCache = HashMap<String, (String, ParseResult)>;

pub fn load(
    root_path: &str,
    source: &dyn FileSource,
    mut cache: Option<&mut ParseCache>,
) -> LoadResult {
    let root = normalize(&root_path.replace('\\', "/"));
    let mut result = LoadResult {
        root: root.clone(),
        ..Default::default()
    };
    let mut seen = HashSet::new();
    visit(&root, None, source, &mut cache, &mut seen, &mut result);
    let file_index: HashMap<&str, usize> = result
        .files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.as_str(), i))
        .collect();
    for e in &mut result.entries {
        e.seq = seq_of(file_index.get(&*e.file).copied().unwrap_or(0), e.line);
    }
    result
}

fn visit(
    path: &str,
    from: Option<(&Arc<str>, u32)>,
    source: &dyn FileSource,
    cache: &mut Option<&mut ParseCache>,
    seen: &mut HashSet<String>,
    result: &mut LoadResult,
) {
    let (at_file, at_line) = match from {
        Some((f, l)) => (f.clone(), l),
        None => (Arc::from(path), 0),
    };
    if !seen.insert(path.to_owned()) {
        result.errors.push(Diagnostic::new(
            &at_file,
            at_line,
            "include",
            format!("Duplicate filename parsed: {path}"),
        ));
        return;
    }
    let Some(text) = source.read(path) else {
        result.errors.push(Diagnostic::new(
            &at_file,
            at_line,
            "include",
            format!("File not found: {path}"),
        ));
        return;
    };
    result.files.push(path.to_owned());
    let parsed = cached_parse(text, path, cache);
    result.errors.extend(parsed.errors.iter().cloned());
    result.plugins.extend(parsed.plugins.iter().cloned());
    if from.is_none() {
        for o in &parsed.options {
            result
                .options
                .entry(o.key.clone())
                .or_default()
                .push(o.value.clone());
        }
    }
    let mut entries = parsed.entries.into_iter();
    let mut next = 0;
    for inc in &parsed.includes {
        result.entries.extend(entries.by_ref().take(inc.at - next));
        next = inc.at;
        visit(
            &join_path(dirname(path), &inc.path),
            Some((&inc.file, inc.line)),
            source,
            cache,
            seen,
            result,
        );
    }
    result.entries.extend(entries);
}

/// The parse of `text`, from the cache when it holds this exact text. Hands out copies, since
/// entries get a fresh `seq` on every load.
fn cached_parse(text: String, path: &str, cache: &mut Option<&mut ParseCache>) -> ParseResult {
    let Some(cache) = cache.as_deref_mut() else {
        return parse(&text, path);
    };
    if let Some((cached, result)) = cache.get(path)
        && *cached == text
    {
        return result.clone();
    }
    let result = parse(&text, path);
    cache.insert(path.to_owned(), (text, result.clone()));
    result
}

/// File order for same-day ties (rule 14): files in the order they were loaded, then line.
/// Derived from the line, so inserting or removing lines only shifts seq; nothing needs
/// renumbering.
pub const SEQ_FILE_STRIDE: u64 = 10_000_000;

pub fn seq_of(file_index: usize, line: u32) -> u64 {
    file_index as u64 * SEQ_FILE_STRIDE + u64::from(line)
}

/// An in-memory file source, for tests and in-memory backends.
#[derive(Clone, Debug, Default)]
pub struct MemorySource(pub HashMap<String, String>);

impl MemorySource {
    pub fn new<'a>(files: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        MemorySource(
            files
                .into_iter()
                .map(|(p, t)| (p.to_owned(), t.to_owned()))
                .collect(),
        )
    }
}

impl FileSource for MemorySource {
    fn read(&self, path: &str) -> Option<String> {
        self.0.get(path).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(dirname("/l/main.beancount"), "/l");
        assert_eq!(dirname("/main.beancount"), "/");
        assert_eq!(dirname("main.beancount"), "");
        assert_eq!(
            join_path("/l", "../x/./2026.beancount"),
            "/x/2026.beancount"
        );
        assert_eq!(join_path("", "../../a"), "../../a");
        assert_eq!(join_path("/l", "C:/abs/a"), "C:/abs/a");
        assert_eq!(join_path("/", "a"), "/a");
    }
}
