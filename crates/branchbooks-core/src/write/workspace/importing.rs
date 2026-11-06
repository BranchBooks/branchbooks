//! Importing entries from other Beancount files.

use super::*;

impl WorkspaceCore {
    /// What importing `files` (path to text) would add to the ledger. Nothing is written.
    pub fn preview_import(&mut self, files: &IndexMap<String, String>) -> ImportPreview {
        let accounts = self
            .current
            .account_info()
            .iter()
            .map(|(a, info)| (a.clone(), info.close.is_some()))
            .collect();
        let text = |p: &str| self.files.get(p).map(String::as_str);
        let ledger = ImportLedger {
            entries: self.current.entries().map(|(_, e)| e).collect(),
            text: &text,
            accounts,
        };
        preview_import(files, &ledger)
    }

    /// Plans an import: the new entries in date order in their year files, opens and closes with
    /// the others in the root. Refused when the new entries would add ledger errors.
    pub fn plan_import(&mut self, preview: &ImportPreview) -> Result<Planned> {
        struct Insert {
            after: u32,
            items: Vec<ImportItem>,
            prev: Option<Peer>,
            next: Option<Peer>,
            block: Vec<String>,
        }
        self.check_format()?;
        let mut by_file: IndexMap<String, IndexMap<u32, Insert>> = IndexMap::new();
        let mut own: HashMap<String, Vec<Peer>> = HashMap::new();
        let mut file_lines: IndexMap<String, Lines> = IndexMap::new();

        // Where each item goes: after the last entry that sorts before it, as insert() would.
        for item in &preview.add {
            let ty = match item.kind {
                ImportKind::Open => EntryType::Open,
                ImportKind::Close => EntryType::Close,
                _ => EntryType::Txn,
            };
            let kind = match ty {
                EntryType::Open => "open",
                EntryType::Close => "close",
                _ => "txn",
            };
            let file = self.target_file(kind, item.date);
            let own_entries = own.entry(file.clone()).or_insert_with(|| {
                let mut v: Vec<Peer> = self
                    .current
                    .entries()
                    .filter(|(_, x)| *x.file == *file)
                    .map(|(_, x)| Peer::of(x))
                    .collect();
                v.sort_by_key(|x| x.line);
                v
            });
            let account_directive = ty != EntryType::Txn;
            let peers: Vec<&Peer> = if account_directive && own_entries.iter().any(Peer::is_account)
            {
                own_entries.iter().filter(|x| x.is_account()).collect()
            } else {
                own_entries.iter().collect()
            };
            let prev = peers
                .iter()
                .rev()
                .find(|x| x.before(item.date, ty))
                .map(|x| (*x).clone());
            let (after, next) = match (&prev, peers.first()) {
                (Some(prev), _) => (
                    prev.end_line,
                    own_entries.iter().find(|x| x.line > prev.end_line).cloned(),
                ),
                (None, Some(first)) => {
                    let mut after = first.line - 1;
                    while after > 0
                        && is_comment(
                            self.lines(&mut file_lines, &file)
                                .lines
                                .get(after as usize - 1),
                        )
                    {
                        after -= 1;
                    }
                    (after, Some((*first).clone()))
                }
                (None, None) => (self.lines(&mut file_lines, &file).lines.len() as u32, None),
            };
            let slots = by_file.entry(file).or_default();
            let slot = slots.entry(after).or_insert(Insert {
                after,
                items: Vec::new(),
                prev,
                next,
                block: Vec::new(),
            });
            slot.items.push(item.clone());
        }

        // New year files get an include in the root, in year order with the others.
        let years = self.year_includes();
        let mut new_years: Vec<String> = by_file
            .keys()
            .filter(|f| !self.files.contains_key(*f) && **f != self.root)
            .cloned()
            .collect();
        new_years.sort();
        for file in new_years {
            let year = year_file(&file).map_or(0, |(y, _)| y);
            let after = place::include_after(&years, year);
            let rel = self.relative(&file);
            let slots = by_file.entry(self.root.clone()).or_default();
            let slot = slots.entry(after).or_insert(Insert {
                after,
                items: Vec::new(),
                prev: None,
                next: None,
                block: Vec::new(),
            });
            slot.block.push(format!("include \"{rel}\""));
        }

        // Bottom up, so the line numbers above each insert still hold.
        let multi_item = |x: &ImportItem| x.lines.len() > 1;
        let multi_peer = |x: &Option<Peer>| x.as_ref().is_some_and(|x| x.end_line > x.line);
        for (file, slots) in &mut by_file {
            let mut slots: Vec<&Insert> = slots.values().collect();
            slots.sort_by_key(|s| std::cmp::Reverse(s.after));
            for s in slots {
                let l = self.lines(&mut file_lines, file);
                if let (Some(first), Some(last)) = (s.items.first(), s.items.last()) {
                    let mut block = Vec::new();
                    for (i, item) in s.items.iter().enumerate() {
                        if i > 0 && (multi_item(item) || multi_item(&s.items[i - 1])) {
                            block.push(String::new());
                        }
                        block.extend(item.lines.iter().cloned());
                    }
                    let empty = s.prev.is_none() && s.next.is_none();
                    let before = if empty {
                        !l.lines.is_empty()
                    } else {
                        multi_item(first) || multi_peer(&s.prev)
                    };
                    let after = multi_item(last) || multi_peer(&s.next);
                    insert_block(l, s.after, block, Separators { before, after });
                }
                if !s.block.is_empty() {
                    insert_block(
                        l,
                        s.after,
                        s.block.clone(),
                        Separators {
                            before: false,
                            after: false,
                        },
                    );
                }
            }
        }

        let need = preview
            .add
            .iter()
            .map(|i| format::needed_by_import(i.kind))
            .max();
        let raised = self.raise_format(&mut file_lines, need.unwrap_or(1));
        let (files, changes) = self.written(&file_lines);
        let loaded = self.load_files(&files);
        let file_order = loaded.files.clone();
        let mut book = Book::from_load(loaded);
        let new_errors = added(&self.current.errors(), &book.errors());
        if !new_errors.is_empty() {
            return Err(rejected("import", new_errors));
        }
        let mut warnings = added(&self.current.blocked(None), &book.blocked(None));
        warnings.extend(added(
            &self.current.budget_diagnostics(),
            &book.budget_diagnostics(),
        ));

        let mut counts: IndexMap<ImportKind, usize> = IndexMap::new();
        for item in &preview.add {
            *counts.entry(item.kind).or_insert(0) += 1;
        }
        let nouns = |k: ImportKind| match k {
            ImportKind::Txn => ("transaction", "transactions"),
            ImportKind::Open => ("account opening", "account openings"),
            ImportKind::Close => ("account closing", "account closings"),
            ImportKind::Assign => ("assignment", "assignments"),
            ImportKind::Move => ("move", "moves"),
            ImportKind::Target => ("target", "targets"),
            ImportKind::Overspending => ("overspending setting", "overspending settings"),
            ImportKind::Custom => ("custom entry", "custom entries"),
            ImportKind::Balance => ("balance assertion", "balance assertions"),
            ImportKind::Other => ("other entry", "other entries"),
        };
        let what: Vec<String> = counts
            .iter()
            .map(|(k, n)| {
                let (one, many) = nouns(*k);
                format!("{n} {}", if *n == 1 { one } else { many })
            })
            .collect();
        let summary = format!("Import {} from {}", what.join(", "), preview.root);
        Ok(Planned {
            op_type: "import".to_owned(),
            summary: match raised {
                Some(n) => format!("{summary}\n\n{}", format::commit_note(n)),
                None => summary,
            },
            changes,
            entry: None,
            warnings,
            files,
            apply: Apply::Full {
                book: Box::new(book),
                file_order,
            },
        })
    }
}
