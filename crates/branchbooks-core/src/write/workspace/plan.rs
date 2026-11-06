//! Planning an operation into text edits, and keeping the result once it is committed.

use super::*;

impl WorkspaceCore {
    /// Turns an operation into validated text edits (not yet committed).
    pub fn plan(&mut self, op: &Operation) -> Result<Planned> {
        self.check_format()?;
        let mut plan = match op {
            Operation::Reconcile(r) => self.plan_reconcile(r)?,
            Operation::RenameAccount { from, to } => self.plan_rename(from, to)?,
            Operation::Add(e) => self.plan_edits(Some(e), None)?,
            Operation::Edit(r, e) => self.plan_edits(Some(e), Some(r))?,
            Operation::Delete(r) => self.plan_edits(None, Some(r))?,
        };
        let need = match op {
            Operation::Add(e) | Operation::Edit(_, e) => format::needed_by_input(e),
            _ => 1,
        };
        // Rare (once a format), so the reload costs little; it puts the format entry in the Book.
        if let Some(raised) = self.raise_format(&mut plan.edits, need) {
            plan.structural = true;
            plan.summary = format!("{}\n\n{}", plan.summary, format::commit_note(raised));
        }
        let (files, changes) = self.written(&plan.edits);
        if plan.structural {
            self.plan_full(plan, files, changes)
        } else {
            self.plan_incremental(plan, files, changes)
        }
    }

    fn plan_edits(&mut self, entry: Option<&EntryInput>, at: Option<&EntryRef>) -> Result<Plan> {
        let currency = self.current.currency().to_owned();
        let mut edits: IndexMap<String, Lines> = IndexMap::new();
        let target = at.map(|r| self.find(r)).transpose()?;
        let target_entry = match target {
            Some(id) => Some(self.current.entry(id).cloned().ok_or_else(|| {
                OperationError::new(
                    OperationErrorCode::NotFound,
                    "entry not in book",
                    Vec::new(),
                )
            })?),
            None => None,
        };
        self.check_placeholders(entry, target_entry.as_ref())?;
        let target_is_account = target_entry
            .as_ref()
            .is_some_and(|t| matches!(t.entry_type(), EntryType::Open | EntryType::Close));
        let entry_date = entry.map(|e| parse_date(e.date())).transpose()?;
        let mut p = Plan {
            op_type: String::new(),
            summary: String::new(),
            envelope: matches!(
                entry,
                Some(
                    EntryInput::Assign(_)
                        | EntryInput::Move(_)
                        | EntryInput::Target(_)
                        | EntryInput::Overspending(_)
                )
            ),
            target,
            edits: IndexMap::new(),
            shifts: Vec::new(),
            placed: None,
            structural: matches!(entry, Some(EntryInput::Open(_) | EntryInput::Close(_)))
                || target_is_account
                || entry.zip(entry_date).is_some_and(|(e, date)| {
                    !self
                        .files
                        .contains_key(&self.target_file(input_kind(e), date))
                }),
        };
        let block = entry.map(|e| render_entry(e, &currency)).transpose()?;

        let Some((entry, date)) = entry.zip(entry_date) else {
            let t = target_entry.ok_or_else(|| {
                OperationError::new(
                    OperationErrorCode::Invalid,
                    "a delete needs a target entry",
                    Vec::new(),
                )
            })?;
            let removed = remove_lines(self.lines(&mut edits, &t.file), t.line, t.end_line);
            p.shifts.push(Shift {
                file: t.file.to_string(),
                from: t.end_line + 1,
                delta: -i64::from(removed),
            });
            p.op_type = op_type_of(kind_of(&t), "delete");
            p.summary = format!("Delete {} {} at {}:{}", kind_of(&t), t.date, t.file, t.line);
            p.edits = edits;
            return Ok(p);
        };
        let block = block.ok_or_else(|| {
            OperationError::new(
                OperationErrorCode::Invalid,
                "the entry did not render",
                Vec::new(),
            )
        })?;
        let file = self.target_file(input_kind(entry), date);
        let same_spot = target_entry
            .as_ref()
            .is_some_and(|t| date == t.date && file == *t.file && kind_of(t) == input_kind(entry));
        if let (true, Some(t)) = (same_spot, &target_entry) {
            replace_lines(
                self.lines(&mut edits, &t.file),
                t.line,
                t.end_line,
                block.clone(),
            );
            let delta = block.len() as i64 - i64::from(t.end_line - t.line + 1);
            p.shifts.push(Shift {
                file: t.file.to_string(),
                from: t.end_line + 1,
                delta,
            });
            p.placed = Some(Placed {
                file: t.file.to_string(),
                line: t.line,
                block,
            });
        } else {
            if let Some(t) = &target_entry {
                let removed = remove_lines(self.lines(&mut edits, &t.file), t.line, t.end_line);
                p.shifts.push(Shift {
                    file: t.file.to_string(),
                    from: t.end_line + 1,
                    delta: -i64::from(removed),
                });
            }
            let known = (!p.structural).then(|| (target, p.shifts.clone()));
            let (file, at) = self.insert(entry, date, block.clone(), &mut edits, known);
            p.shifts.push(Shift {
                file: file.clone(),
                from: at.at,
                delta: i64::from(at.count),
            });
            p.placed = Some(Placed {
                file,
                line: at.start,
                block,
            });
        }
        let verb = if target.is_some() { "edit" } else { "add" };
        p.op_type = op_type_of(input_kind(entry), verb);
        // A zero target ends the category's target (T1).
        if let EntryInput::Target(t) = entry
            && verb == "add"
            && crate::money::parse_decimal(&t.amount.replace(',', "")).is_some_and(|a| a.is_zero())
        {
            p.op_type = "end-target".to_owned();
        }
        let what = describe(entry, &currency);
        p.summary = if verb == "edit" {
            format!("Edit {what}")
        } else {
            capitalize(&what)
        };
        p.edits = edits;
        Ok(p)
    }

    /// Refuses a new posting to a placeholder, and making a placeholder of an account that holds
    /// transactions. An edited transaction may keep a posting it already had, so that it can be
    /// edited at all. Envelope entries naming a placeholder are refused with the envelope checks.
    fn check_placeholders(
        &mut self,
        entry: Option<&EntryInput>,
        target: Option<&Entry>,
    ) -> Result<()> {
        let invalid = |m: String| OperationError::new(OperationErrorCode::Invalid, m, Vec::new());
        match entry {
            Some(EntryInput::Txn(t)) => {
                let had = |a: &str| {
                    target
                        .and_then(Entry::txn)
                        .is_some_and(|t| t.postings.iter().any(|p| p.account == a))
                };
                for p in &t.postings {
                    if self.current.is_placeholder(&p.account) && !had(&p.account) {
                        return Err(invalid(format!(
                            "{} is a placeholder, which only groups its sub-accounts: use one of them",
                            p.account
                        )));
                    }
                }
            }
            Some(EntryInput::Open(o)) if o.budget == Some(BudgetFlag::Placeholder) => {
                let used = self.current.transactions().iter().any(|e| {
                    e.txn()
                        .is_some_and(|t| t.postings.iter().any(|p| p.account == o.account))
                });
                if used {
                    return Err(invalid(format!(
                        "{} has transactions, so it cannot be a placeholder: move them to a sub-account first",
                        o.account
                    )));
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// The ledger with `edits` made, and the files they change.
    pub(super) fn written(
        &self,
        edits: &IndexMap<String, Lines>,
    ) -> (HashMap<String, String>, Vec<FileChange>) {
        let mut files = self.files.clone();
        let mut changes = Vec::new();
        for (path, l) in edits {
            let text = join_lines(l);
            if self.files.get(path) == Some(&text) {
                continue;
            }
            files.insert(path.clone(), text.clone());
            changes.push(FileChange {
                path: path.clone(),
                text: Some(text),
            });
        }
        (files, changes)
    }

    /// Reconcile (R2, R3). The statement must equal the cleared balance once `clear` is cleared.
    /// Then one change flips those transactions to `*` and writes a `balance` dated the day after.
    /// Beancount's assertion counts pending transactions too, so one left out up to the statement
    /// date is refused rather than written as a failing assertion (docs/DESIGN.md, "Reconcile").
    fn plan_reconcile(&mut self, r: &ReconcileInput) -> Result<Plan> {
        let invalid = |m: String| OperationError::new(OperationErrorCode::Invalid, m, Vec::new());
        let date = parse_date(&r.date)?;
        let balance = amount_text(&r.balance)?;
        let statement = parse_decimal(&balance).expect("checked by amount_text");
        let currency = match &r.currency {
            Some(c) => c.clone(),
            None => self.current.currency().to_owned(),
        };
        if !self.current.account_info().contains_key(&r.account) {
            return Err(invalid(format!("{} is not an open account", r.account)));
        }
        let mut ticked = Vec::new();
        for c in &r.clear {
            let e = self
                .current
                .entry(self.find(c)?)
                .expect("found entries exist");
            ticked.push((e.file.to_string(), e.line));
        }
        let rec = self.current.reconcile(&r.account, date, &currency);
        let mut cleared = rec.cleared;
        let mut left = 0;
        let mut found = 0;
        for (e, amount) in &rec.pending {
            if ticked.iter().any(|(f, l)| *f == *e.file && *l == e.line) {
                cleared += amount;
                found += 1;
            } else {
                left += 1;
            }
        }
        if found != ticked.len() {
            return Err(invalid(format!(
                "only pending transactions in {} up to {} can be cleared",
                r.account, r.date
            )));
        }
        if cleared != statement {
            return Err(invalid(format!(
                "the statement is {} {currency} from the cleared balance of {} {currency}",
                statement - cleared,
                cleared
            )));
        }
        if left > 0 {
            return Err(invalid(format!(
                "{left} pending transaction(s) up to {} are not on the statement; a balance \
                 assertion counts them, so date them after the statement or clear them",
                r.date
            )));
        }

        let mut edits: IndexMap<String, Lines> = IndexMap::new();
        for (file, line) in &ticked {
            let head = &mut self.lines(&mut edits, file).lines[*line as usize - 1];
            // The flag follows the date: `2026-01-05 ! "Payee"`.
            let rest = head[10..].trim_start();
            let at = head.len() - rest.len();
            head.replace_range(at..=at, "*");
        }
        let entry = EntryInput::Balance(BalanceInput {
            date: date.next_day().to_string(),
            account: r.account.clone(),
            amount: balance.clone(),
            currency: Some(currency.clone()),
        });
        let block = render_entry(&entry, &currency)?;
        self.insert(&entry, date.next_day(), block, &mut edits, None);
        let clearing = match ticked.len() {
            0 => String::new(),
            1 => ", clearing 1 transaction".to_owned(),
            n => format!(", clearing {n} transactions"),
        };
        Ok(Plan {
            op_type: "reconcile".to_owned(),
            summary: format!(
                "Reconcile {} at {balance} {currency} on {}{clearing}",
                r.account, r.date
            ),
            envelope: false,
            target: None,
            edits,
            shifts: Vec::new(),
            placed: None,
            structural: true,
        })
    }

    /// Renames `from` and its sub-accounts to `to` in every line that names them (opens, closes,
    /// postings, envelope entries, targets, metadata such as a transfer's `category:`), as one
    /// change. Comment lines are left alone. The type stays (Assets stays Assets): another root
    /// would change what the account means to the budget. Refused when a new name is taken.
    fn plan_rename(&mut self, from: &str, to: &str) -> Result<Plan> {
        let invalid = |m: String| OperationError::new(OperationErrorCode::Invalid, m, Vec::new());
        if !is_account(to) {
            return Err(invalid(format!(
                "{to} is not an account name, such as Expenses:Food:Dining"
            )));
        }
        if from == to {
            return Err(invalid("the new name is the same".to_owned()));
        }
        let root = |a: &str| a.split(':').next().unwrap_or("").to_owned();
        if root(from) != root(to) {
            return Err(invalid(format!(
                "{to} must stay under {}: another type would change what the account means",
                root(from)
            )));
        }
        let accounts = self.current.account_info();
        if !accounts.contains_key(from) {
            return Err(invalid(format!("{from} is not an open account")));
        }
        for a in accounts.keys() {
            if let Some(n) = renamed(a, from, to)
                && accounts.contains_key(&n)
            {
                return Err(invalid(format!("{n} already exists")));
            }
        }
        let mut edits: IndexMap<String, Lines> = IndexMap::new();
        let paths: Vec<String> = self.files.keys().cloned().collect();
        for path in paths {
            for line in &mut self.lines(&mut edits, &path).lines {
                if !line.trim_start().starts_with(';') {
                    *line = rename_in(line, from, to);
                }
            }
        }
        Ok(Plan {
            op_type: "rename-account".to_owned(),
            summary: format!("Rename {from} to {to}"),
            envelope: false,
            target: None,
            edits,
            shifts: Vec::new(),
            placed: None,
            structural: true,
        })
    }

    pub(super) fn lines<'e>(
        &self,
        edits: &'e mut IndexMap<String, Lines>,
        path: &str,
    ) -> &'e mut Lines {
        edits
            .entry(path.to_owned())
            .or_insert_with(|| split_lines(self.text(path).unwrap_or("")))
    }

    /// The general path: reload the whole candidate ledger and compare. Used when the operation
    /// changes accounts or creates a file, where classification or file order can change.
    fn plan_full(
        &mut self,
        plan: Plan,
        files: HashMap<String, String>,
        changes: Vec<FileChange>,
    ) -> Result<Planned> {
        let loaded = self.load_files(&files);
        let file_order = loaded.files.clone();
        let mut book = Book::from_load(loaded);
        let new_errors = added(&self.current.errors(), &book.errors());
        if !new_errors.is_empty() {
            return Err(rejected("change", new_errors));
        }
        let new_budget = added(
            &self.current.budget_diagnostics(),
            &book.budget_diagnostics(),
        );
        let new_blocked = added(&self.current.blocked(None), &book.blocked(None));
        if plan.envelope {
            check_envelope(&new_budget, &new_blocked)?;
        }
        let mut warnings = new_blocked;
        warnings.extend(new_budget);
        Ok(Self::planned(
            plan,
            files,
            changes,
            warnings,
            Apply::Full {
                book: Box::new(book),
                file_order,
            },
        ))
    }

    /// The fast path for transactions and envelope entries: validate only the changed entry, try
    /// the budget change on the live engine and roll it back. Costs time in proportion to the
    /// change and the months after it, not the ledger size.
    fn plan_incremental(
        &mut self,
        plan: Plan,
        files: HashMap<String, String>,
        changes: Vec<FileChange>,
    ) -> Result<Planned> {
        let target = plan.target;
        let target_entry = target.and_then(|id| self.current.entry(id)).cloned();
        let mut entry = None;
        if let Some(Placed { file, line, block }) = &plan.placed {
            let parsed = parse(&block.join("\n"), file);
            if !parsed.errors.is_empty() || parsed.entries.len() != 1 {
                let message = parsed.errors.first().map_or_else(
                    || "the entry did not render to a single directive".to_owned(),
                    |e| e.message.clone(),
                );
                return Err(OperationError::new(
                    OperationErrorCode::Invalid,
                    message,
                    Vec::new(),
                ));
            }
            let Some(mut e) = parsed.entries.into_iter().next() else {
                return Err(OperationError::new(
                    OperationErrorCode::Invalid,
                    "the entry did not render to a single directive",
                    Vec::new(),
                ));
            };
            // Parsed as a block starting at line 1; move it to where it was placed.
            let delta = line - 1;
            e.line += delta;
            e.end_line += delta;
            if let Body::Txn(t) = &mut e.body {
                for p in &mut t.postings {
                    p.line += delta;
                }
            }
            // Cannot miss: the fast path only writes to files the ledger already loads.
            let index = self.file_order.iter().position(|f| f == file).unwrap_or(0);
            e.seq = seq_of(index, e.line);
            entry = Some(Box::new(e));
        }

        let book = &mut self.current;
        let fx = entry.as_ref().map(|e| book.effect_of(e));
        let old_errors = target.map_or_else(Vec::new, |id| book.errors_of(id));
        let new_errors = added(&old_errors, fx.as_ref().map_or(&[][..], |f| &f.errors));
        if !new_errors.is_empty() {
            return Err(rejected("change", new_errors));
        }
        // A balance assertion depends on every earlier transaction, so check them all.
        let before = book.balance_failures(None, None);
        let booked = fx.as_ref().and_then(|f| f.booked.as_ref());
        let mut broken = added(&before, &book.balance_failures(target, booked));
        if !broken.is_empty() {
            for d in &mut broken {
                for s in &plan.shifts {
                    if *d.file == *s.file && d.line >= s.from {
                        d.line = crate::budget::shift(d.line, s.delta);
                    }
                }
            }
            return Err(rejected("change", broken));
        }
        let old_flags: Vec<Diagnostic> = match &target_entry {
            Some(t) => book
                .budget_diagnostics()
                .into_iter()
                .filter(|d| d.file == t.file && d.line == t.line)
                .collect(),
            None => Vec::new(),
        };
        let new_budget = added(&old_flags, fx.as_ref().map_or(&[][..], |f| &f.flags));
        let dates = target_entry
            .iter()
            .chain(entry.iter().map(|e| &**e))
            .map(|e| e.date);
        let from = dates.min().ok_or_else(|| {
            OperationError::new(OperationErrorCode::Invalid, "nothing to change", Vec::new())
        })?;
        let before = book.blocked(Some(from));
        let old_event = target.and_then(|id| book.event_of(id).cloned());
        let after = book.blocked_with(
            from,
            old_event.as_ref(),
            fx.as_ref().and_then(|f| f.event.as_ref()),
        );
        let new_blocked = added(&before, &after);
        if plan.envelope {
            check_envelope(&new_budget, &new_blocked)?;
        }
        let mut warnings = new_blocked;
        warnings.extend(new_budget.iter().cloned());
        let shifts = plan.shifts.clone();
        let apply = Apply::Incremental {
            target,
            shifts,
            entry,
            from,
            before,
            new_budget,
        };
        Ok(Self::planned(plan, files, changes, warnings, apply))
    }

    fn planned(
        plan: Plan,
        files: HashMap<String, String>,
        changes: Vec<FileChange>,
        warnings: Vec<Diagnostic>,
        apply: Apply,
    ) -> Planned {
        let entry = plan.placed.map(|p| EntryRef {
            file: p.file,
            line: p.line,
            fingerprint: fingerprint(&p.block),
        });
        Planned {
            op_type: plan.op_type,
            summary: plan.summary,
            changes,
            entry,
            warnings,
            files,
            apply,
        }
    }

    /// Brings the workspace to the committed `version` of a planned change, and returns the
    /// change's warnings.
    pub fn commit_planned(&mut self, planned: Planned, version: &str) -> Vec<Diagnostic> {
        self.files = planned.files;
        self.version = version.to_owned();
        match planned.apply {
            Apply::Full { book, file_order } => {
                self.file_order = file_order;
                self.current = *book;
                planned.warnings
            }
            Apply::Incremental {
                target,
                shifts,
                entry,
                from,
                before,
                new_budget,
            } => {
                let book = &mut self.current;
                if let Some(id) = target {
                    let removed = book.remove_entry(id);
                    debug_assert!(
                        removed.is_some(),
                        "a planned edit targets an entry in the book"
                    );
                }
                for s in &shifts {
                    book.shift_lines(&s.file, s.from, s.delta);
                }
                if let Some(e) = entry {
                    book.add_entry(*e);
                }
                // Report newly blocked entries with their line numbers after the change.
                let mut out = added(&before, &book.blocked(Some(from)));
                out.extend(new_budget);
                out
            }
        }
    }
}
