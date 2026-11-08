//! Where an entry goes: finding it, its file, and its place among its neighbours.

use super::*;

impl WorkspaceCore {
    /// Resolves a reference. The entry is normally still at its line. If other changes moved it,
    /// the same text is looked up elsewhere: the nearest match in the same file, or a single match
    /// in another file. If the text is nowhere, the entry was changed or deleted: `stale`.
    pub fn find(&self, r: &EntryRef) -> Result<EntryId> {
        let all: Vec<(EntryId, &Entry)> = self.current.entries().collect();
        let at = all
            .iter()
            .find(|(_, x)| *x.file == *r.file && x.line == r.line);
        if let Some((id, e)) = at
            && self.ref_of(e).fingerprint == r.fingerprint
        {
            return Ok(*id);
        }
        let mut by_file: HashMap<&str, Lines> = HashMap::new();
        let mut matches: Vec<(EntryId, &Entry)> = Vec::new();
        for (id, e) in &all {
            let l = by_file
                .entry(&e.file)
                .or_insert_with(|| split_lines(self.text(&e.file).unwrap_or("")));
            if fingerprint(entry_lines(l, e.line, e.end_line)) == r.fingerprint {
                matches.push((*id, e));
            }
        }
        let mut same_file: Vec<&(EntryId, &Entry)> =
            matches.iter().filter(|(_, e)| *e.file == *r.file).collect();
        same_file.sort_by_key(|(_, e)| e.line.abs_diff(r.line));
        if let Some((id, _)) = same_file.first() {
            return Ok(*id);
        }
        match (matches.len(), at) {
            (1, _) => Ok(matches[0].0),
            (n, _) if n > 1 => Err(OperationError::new(
                OperationErrorCode::Stale,
                format!(
                    "the entry from {}:{} now matches {n} entries",
                    r.file, r.line
                ),
                Vec::new(),
            )),
            (_, Some(_)) => Err(OperationError::new(
                OperationErrorCode::Stale,
                format!(
                    "the entry at {}:{} has changed since it was read",
                    r.file, r.line
                ),
                Vec::new(),
            )),
            _ => Err(OperationError::new(
                OperationErrorCode::NotFound,
                format!(
                    "no entry matches {}:{}; it was changed or deleted",
                    r.file, r.line
                ),
                Vec::new(),
            )),
        }
    }

    /// Root includes, from the parse cache when the root is unchanged (the root can be the whole
    /// ledger).
    fn root_includes(&self) -> Vec<IncludeLine> {
        let Some(text) = self.text(&self.root) else {
            return Vec::new();
        };
        match self.cache.get(&self.root) {
            Some((cached, result)) if cached == text => result.includes.clone(),
            _ => parse(text, &self.root).includes,
        }
    }

    /// Year layout: the root includes NNNN.beancount files.
    pub(super) fn year_includes(&self) -> Vec<YearInclude> {
        self.root_includes()
            .into_iter()
            .filter_map(|inc| {
                let (year, ext) = year_file(&inc.path)?;
                Some(YearInclude {
                    path: join_path(dirname(&self.root), &inc.path),
                    year,
                    line: inc.line,
                    ext: ext.to_owned(),
                })
            })
            .collect()
    }

    /// The file an entry of `kind` (`txn`, `open`, ...) dated `date` belongs in.
    pub fn target_file(&self, kind: &str, date: Date) -> String {
        if kind == "open" || kind == "close" {
            return self.root.clone();
        }
        let years = self.year_includes();
        let Some(first) = years.first() else {
            return self.root.clone();
        };
        let year = date.year().unsigned_abs();
        match years.iter().find(|y| y.year == year) {
            Some(hit) => hit.path.clone(),
            None => join_path(dirname(&self.root), &format!("{year}.{}", first.ext)),
        }
    }

    /// `file` relative to the root's directory.
    pub(super) fn relative(&self, file: &str) -> String {
        let dir = dirname(&self.root);
        let skip = if dir.is_empty() { 0 } else { dir.len() + 1 };
        file.get(skip..).unwrap_or("").to_owned()
    }

    /// The entries of a file as they will be once `target` is removed and `shifts` applied: lets
    /// `insert()` place an entry without re-parsing the file.
    fn peers(&self, file: &str, target: Option<EntryId>, shifts: &[Shift]) -> Vec<Peer> {
        let mut out: Vec<Peer> = self
            .current
            .entries()
            .filter(|(id, x)| *x.file == *file && Some(*id) != target)
            .map(|(_, x)| {
                let mut p = Peer::of(x);
                for s in shifts {
                    if s.file == file && p.line >= s.from {
                        p.line = crate::budget::shift(p.line, s.delta);
                        p.end_line = crate::budget::shift(p.end_line, s.delta);
                    }
                }
                p
            })
            .collect();
        out.sort_by_key(|p| p.line);
        out
    }

    /// Places a rendered entry in date order in the right file, creating a year file if needed.
    pub(super) fn insert(
        &self,
        e: &EntryInput,
        date: Date,
        block: Vec<String>,
        edits: &mut IndexMap<String, Lines>,
        known: Option<(Option<EntryId>, Vec<Shift>)>,
    ) -> (String, Inserted) {
        let file = self.target_file(input_kind(e), date);
        self.lines(edits, &file);

        if !self.files.contains_key(&file) && file != self.root {
            // New year file: add it, and an include in the root next to the other years.
            let year = date.year().unsigned_abs();
            let after = include_after(&self.year_includes(), year);
            let inc = format!("include \"{}\"", self.relative(&file));
            insert_block(
                self.lines(edits, &self.root),
                after,
                vec![inc],
                Separators {
                    before: false,
                    after: false,
                },
            );
        }

        let ty = match e {
            EntryInput::Open(_) => EntryType::Open,
            EntryInput::Close(_) => EntryType::Close,
            EntryInput::Balance(_) => EntryType::Balance,
            _ => EntryType::Txn,
        };
        let account_directive = matches!(ty, EntryType::Open | EntryType::Close);
        let own_entries: Vec<Peer> = if let Some((target, shifts)) = known {
            self.peers(&file, target, &shifts)
        } else {
            let text = join_lines(&edits[&file]);
            parse(&text, &file).entries.iter().map(Peer::of).collect()
        };
        // Opens and closes stay with the other opens and closes; everything else goes in date
        // order.
        let peers: Vec<&Peer> = if account_directive && own_entries.iter().any(Peer::is_account) {
            own_entries.iter().filter(|x| x.is_account()).collect()
        } else {
            own_entries.iter().collect()
        };
        let prev = peers.iter().rev().find(|x| x.before(date, ty));
        // Transactions get a blank line on each side; one-line directives and opens sit together.
        let gap = |x: Option<&Peer>| x.is_some_and(|x| x.ty == EntryType::Txn);
        let own = matches!(e, EntryInput::Txn(_));
        let l = self.lines(edits, &file);
        let placed = match (prev, peers.first()) {
            (Some(prev), _) => {
                let next = own_entries.iter().find(|x| x.line > prev.end_line);
                insert_block(
                    l,
                    prev.end_line,
                    block,
                    Separators {
                        before: own || gap(Some(prev)),
                        after: own || gap(next),
                    },
                )
            }
            (None, Some(first)) => {
                // Before the first entry, and above any comment lines that belong to it.
                let mut at = first.line - 1;
                while at > 0 && is_comment(l.lines.get(at as usize - 1)) {
                    at -= 1;
                }
                insert_block(
                    l,
                    at,
                    block,
                    Separators {
                        before: own,
                        after: own || gap(Some(first)),
                    },
                )
            }
            (None, None) => {
                let n = l.lines.len() as u32;
                insert_block(
                    l,
                    n,
                    block,
                    Separators {
                        before: n > 0,
                        after: false,
                    },
                )
            }
        };
        (file, placed)
    }
}

/// The root's line a new `year`'s `include` goes after, keeping the years in order.
pub(super) fn include_after(years: &[YearInclude], year: u32) -> u32 {
    match years.iter().rfind(|y| y.year < year) {
        Some(before) => before.line,
        None => years
            .iter()
            .find(|y| y.year > year)
            .map_or(0, |y| y.line - 1),
    }
}

pub(super) struct YearInclude {
    pub(super) path: String,
    pub(super) year: u32,
    pub(super) line: u32,
    pub(super) ext: String,
}
