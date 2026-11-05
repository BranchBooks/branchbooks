//! Sync over HTTPS with a remote: pull, merge, push, and the refs that travel with it.

use super::*;

impl GitStore {
    /// Clones `url` into `dir` (missing or empty) and opens it. `token`: sent as the password
    /// over HTTPS. Text is kept as committed (`core.autocrlf=false`), whatever the global git
    /// config says, so Windows gets the same ledger bytes.
    pub fn clone_from(
        url: &str,
        dir: impl Into<PathBuf>,
        token: Option<&str>,
        author: Option<Author>,
    ) -> Result<GitStore, StoreError> {
        let dir = dir.into();
        let http = http_for(url, token)?;
        let mut opts = CloneOptions::new(url, dir.clone());
        opts.config
            .push(("core.autocrlf".to_owned(), "false".to_owned()));
        clone(opts, &mut NoProgress, &NeverCancel, Some(&http)).map_err(g("clone"))?;
        Ok(GitStore::new(dir, author))
    }

    /// Makes this book the remote's, for a book whose history is separate from the remote's
    /// (`sync` refuses to merge those). Trunk becomes one commit joining both histories with this
    /// book's files, so the push is a fast-forward (a protected branch takes it) and the remote's
    /// old book stays in history. The remote's scenarios and closed months are set aside under
    /// `refs/tags/archive/replaced-<seconds>/`. One leased push: if the remote moved since the
    /// fetch, the remote is unchanged.
    pub fn replace_remote(&self, token: Option<&str>) -> Result<(), StoreError> {
        if self.rev != "HEAD" {
            return Err(StoreError::Refused(
                "only the book itself replaces a remote".into(),
            ));
        }
        let repo = self.open()?;
        let remote = repo
            .remotes()
            .get("origin")
            .map_err(|_| StoreError::Refused("this book has no remote".into()))?;
        let http = http_for(&self.remote_url().unwrap_or_default(), token)?;
        let mut opts = FetchOptions::new();
        opts.refspecs = vec![
            "+refs/heads/*:refs/remotes/origin/*".to_owned(),
            format!("+{CLOSES}*:{THEIR_CLOSES}*"),
        ];
        opts.prune = true;
        repo.remotes()
            .fetch(&remote, opts, &mut NoProgress, &NeverCancel, Some(&http))
            .map_err(g("fetch"))?;
        let ours =
            resolve_revision(&repo, "HEAD").map_err(|_| failed("this book has no commits yet"))?;
        let branch = self.branch(&repo)?;
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let kept = format!("{ARCHIVED}replaced-{secs}/");
        let mut push = Vec::new();
        let mut trunk = None;
        let mut theirs: Vec<(String, ObjectId)> = Vec::new();
        for (tracking, rev) in [
            ("refs/remotes/origin/", "refs/heads/"),
            (THEIR_CLOSES, CLOSES),
        ] {
            for (name, id) in refs::list_refs(repo.git_dir(), tracking).map_err(g("list refs"))? {
                let name = name.strip_prefix(tracking).unwrap_or(&name).to_owned();
                if name != "HEAD" {
                    theirs.push((format!("{rev}{name}"), id));
                }
            }
        }
        for (rev, id) in theirs {
            if rev == branch {
                trunk = Some(id);
                continue;
            }
            let short = rev.strip_prefix("refs/").unwrap_or(&rev);
            push.push(PushRefSpec {
                src: Some(id),
                dst: format!("{kept}{short}"),
                expect_absent: true,
                ..Default::default()
            });
            push.push(push_spec(&rev, None, Some(id)));
        }
        let new = match trunk {
            Some(t) if !is_ancestor(&repo, t, ours).map_err(g("compare versions"))? => {
                let info = CommitInfo {
                    op_id: format!("replace-{}-{}", &ours.to_hex()[..12], &t.to_hex()[..12]),
                    op_type: "sync".to_owned(),
                    summary: "Replace the remote's book with this one".to_owned(),
                };
                let tree = tree_of(&repo, &ours)?;
                let join =
                    self.write_commit(&repo, tree, vec![ours, t], &commit_message(&info), None)?;
                self.advance(&repo, join, Some(ours))?;
                join
            }
            _ => ours,
        };
        push.push(push_spec(&branch, Some(new), trunk));
        match send(&repo, &remote, &push, &http)?.first() {
            Some(refused) => Err(StoreError::Conflict(format!(
                "the remote did not take {refused}; try again"
            ))),
            None => Ok(()),
        }
    }

    /// Scenarios, closed months and what was archived, after `sync` brought trunk together.
    /// Archived refs (`refs/tags/archive/…`: pruned scenarios, reopened months) are the union of both
    /// sides. A scenario or close whose tip is archived on either side goes on both. Otherwise one
    /// missing on a side is copied there, a scenario that only moved forward is fast-forwarded,
    /// and one changed on both sides is kept as it is here and named in the notes returned (never
    /// merged). Pushes are leased, as trunk's are.
    pub fn sync_refs(&self, token: Option<&str>) -> Result<Vec<String>, StoreError> {
        let repo = self.open()?;
        let remote = repo
            .remotes()
            .get("origin")
            .map_err(|_| StoreError::Refused("this book has no remote".into()))?;
        let http = http_for(&self.remote_url().unwrap_or_default(), token)?;
        let mut opts = FetchOptions::new();
        opts.refspecs = vec![
            format!("+{SCENARIOS}*:{THEIR_SCENARIOS}*"),
            format!("+{CLOSES}*:{THEIR_CLOSES}*"),
            format!("+{ARCHIVED}*:{THEIR_ARCHIVED}*"),
        ];
        opts.prune = true;
        repo.remotes()
            .fetch(&remote, opts, &mut NoProgress, &NeverCancel, Some(&http))
            .map_err(g("fetch"))?;
        let list = |prefix: &str| -> Result<IndexMap<String, ObjectId>, StoreError> {
            Ok(refs::list_refs(repo.git_dir(), prefix)
                .map_err(g("list refs"))?
                .into_iter()
                .map(|(n, id)| (n.strip_prefix(prefix).unwrap_or(&n).to_owned(), id))
                .collect())
        };
        let mut notes = Vec::new();
        let mut push = Vec::new();

        let (ours, theirs) = (list(ARCHIVED)?, list(THEIR_ARCHIVED)?);
        for (n, t) in &theirs {
            if !ours.contains_key(n) {
                update_ref(&repo, &format!("{ARCHIVED}{n}"), Some(*t), None)
                    .map_err(g("archive"))?;
            }
        }
        for (n, o) in &ours {
            if !theirs.contains_key(n) {
                push.push(push_spec(&format!("{ARCHIVED}{n}"), Some(*o), None));
            }
        }
        let archived: HashSet<ObjectId> = ours.values().chain(theirs.values()).copied().collect();

        for (mine, tracking, what) in [
            (SCENARIOS, THEIR_SCENARIOS, "scenario"),
            (CLOSES, THEIR_CLOSES, "close of"),
        ] {
            let (ours, theirs) = (list(mine)?, list(tracking)?);
            let mut names: Vec<&String> = ours.keys().chain(theirs.keys()).collect();
            names.sort();
            names.dedup();
            for name in names {
                let rev = format!("{mine}{name}");
                match (ours.get(name).copied(), theirs.get(name).copied()) {
                    (Some(o), t) if archived.contains(&o) => {
                        update_ref(&repo, &rev, None, Some(o)).map_err(g("archive"))?;
                        if let Some(t) = t.filter(|t| archived.contains(t)) {
                            push.push(push_spec(&rev, None, Some(t)));
                        }
                    }
                    (o, Some(t)) if archived.contains(&t) => match o {
                        None => push.push(push_spec(&rev, None, Some(t))),
                        Some(_) => notes.push(format!(
                            "{what} {name} was set aside on another device but changed here: kept here"
                        )),
                    },
                    (Some(o), None) => push.push(push_spec(&rev, Some(o), None)),
                    (None, Some(t)) => {
                        update_ref(&repo, &rev, Some(t), None).map_err(g("update"))?;
                    }
                    (Some(o), Some(t)) if o == t => {}
                    (Some(o), Some(t)) => {
                        let forward = |a, b| is_ancestor(&repo, a, b).map_err(g("compare versions"));
                        if mine == SCENARIOS && forward(t, o)? {
                            push.push(push_spec(&rev, Some(o), Some(t)));
                        } else if mine == SCENARIOS && forward(o, t)? {
                            update_ref(&repo, &rev, Some(t), Some(o)).map_err(g("update"))?;
                        } else {
                            notes.push(format!(
                                "{what} {name} changed both here and on another device: kept here"
                            ));
                        }
                    }
                    (None, None) => {}
                }
            }
        }
        if !push.is_empty() {
            for refused in send(&repo, &remote, &push, &http)? {
                notes.push(format!("the remote did not take {refused}; sync again"));
            }
        }
        Ok(notes)
    }

    /// Brings trunk and the remote `origin` together: fetch, then push this device's new
    /// commits, fast-forward to the remote's, or, when both have new commits, merge them in one
    /// commit (Op-Type `sync`) and push that. `root`: the ledger's main file, for the check that
    /// a merge adds no ledger errors.
    ///
    /// Refused, with nothing pushed and local commits kept, when both changed the same lines,
    /// when the merge would add ledger errors, or when trunk's files have uncommitted changes.
    /// A conflict when the remote moved again before the push: sync again.
    pub fn sync(&self, root: &str, token: Option<&str>) -> Result<Synced, StoreError> {
        if self.rev != "HEAD" {
            return Err(StoreError::Refused("only trunk is synced".into()));
        }
        let repo = self.open()?;
        let remote = repo
            .remotes()
            .get("origin")
            .map_err(|_| StoreError::Refused("this book has no remote".into()))?;
        let http = http_for(&self.remote_url().unwrap_or_default(), token)?;
        // Pruned, so a branch the remote does not have (a new, empty remote) is not taken for one
        // it does: that would report "up to date" and push nothing.
        let mut opts = FetchOptions::new();
        opts.prune = true;
        repo.remotes()
            .fetch(&remote, opts, &mut NoProgress, &NeverCancel, Some(&http))
            .map_err(g("fetch"))?;
        let branch = self.branch(&repo)?;
        let tracking = format!(
            "refs/remotes/origin/{}",
            branch.strip_prefix("refs/heads/").unwrap_or(&branch)
        );
        let ours = resolve_revision(&repo, "HEAD").ok();
        let theirs = refs::resolve_ref(repo.git_dir(), &tracking).ok();
        let push = |new| push_branch(&repo, &remote, &branch, new, theirs, &http);
        let (o, t) = match (ours, theirs) {
            (None, None) => return Ok(Synced::UpToDate),
            (Some(o), None) => {
                push(o)?;
                return Ok(Synced::Pushed);
            }
            (None, Some(t)) => {
                self.advance(&repo, t, None)?;
                return Ok(Synced::Pulled);
            }
            (Some(o), Some(t)) => (o, t),
        };
        if o == t {
            return Ok(Synced::UpToDate);
        }
        if is_ancestor(&repo, t, o).map_err(g("compare versions"))? {
            push(o)?;
            return Ok(Synced::Pushed);
        }
        if is_ancestor(&repo, o, t).map_err(g("compare versions"))? {
            self.advance(&repo, t, Some(o))?;
            return Ok(Synced::Pulled);
        }
        if merge_bases_all(&repo, &[o, t])
            .map_err(g("merge base"))?
            .is_empty()
        {
            return Err(StoreError::Refused(
                "this book and the remote's are separate books (no shared history), so they are \
                 not merged. In Book settings, replace the remote with this book, or give this \
                 book an empty remote"
                    .into(),
            ));
        }
        let tree = match merge3(&repo, o, t)? {
            Ok(tree) => tree,
            Err(files) => {
                return Err(StoreError::Refused(format!(
                    "this device and the remote both changed the same lines in {}",
                    files.join(", ")
                )));
            }
        };
        let mine = ledger_files(&repo, &o)?;
        let errors = new_errors(
            root,
            &mine,
            &changes_between(&mine, &ledger_files(&repo, &tree)?),
        );
        if let Some(first) = errors.first() {
            return Err(StoreError::Refused(format!(
                "merging with the remote would add {} ledger error(s): {}",
                errors.len(),
                first.message
            )));
        }
        let (oh, th) = (o.to_hex(), t.to_hex());
        let info = CommitInfo {
            // The same two versions always make the same merge, so the same id.
            op_id: format!("sync-{}-{}", &oh[..12], &th[..12]),
            op_type: "sync".to_owned(),
            summary: "Sync with the remote".to_owned(),
        };
        let merge = self.write_commit(&repo, tree, vec![o, t], &commit_message(&info), None)?;
        self.advance(&repo, merge, Some(o))?;
        push(merge)?;
        Ok(Synced::Merged)
    }
}

/// A version for messages: "(no commits)" when empty.
pub(super) fn version_name(v: &str) -> String {
    if v.is_empty() {
        "(no commits)".to_owned()
    } else {
        v.to_owned()
    }
}

/// `ours` and `theirs` merged from where they last met: the merged tree, or the paths that both
/// changed in ways that cannot be combined.
pub(super) fn merge3(
    repo: &Repository,
    ours: ObjectId,
    theirs: ObjectId,
) -> Result<Result<ObjectId, Vec<String>>, StoreError> {
    let common = merge_bases_all(repo, &[ours, theirs])
        .map_err(g("merge base"))?
        .first()
        .copied()
        .ok_or_else(|| failed("the two versions have no common history"))?;
    let out = merge_trees_three_way(
        repo,
        tree_of(repo, &common)?,
        tree_of(repo, &ours)?,
        tree_of(repo, &theirs)?,
        Default::default(),
        WhitespaceMergeOptions::default(),
        None,
        TreeMergeConflictPresentation::default(),
        DirectoryRenameMode::default(),
    )
    .map_err(g("merge"))?;
    let mut conflicts: Vec<String> = out
        .index
        .entries()
        .iter()
        .filter(|e| e.stage() != 0)
        .map(|e| String::from_utf8_lossy(&e.path).into_owned())
        .collect();
    conflicts.dedup();
    Ok(if conflicts.is_empty() {
        Ok(out.result_tree)
    } else {
        Err(conflicts)
    })
}

/// A leased push of `rev`: to `new` (`None` deletes it), expected at `theirs` on the remote
/// (`None`: must not exist there).
fn push_spec(rev: &str, new: Option<ObjectId>, theirs: Option<ObjectId>) -> PushRefSpec {
    PushRefSpec {
        src: new,
        src_ref: new.map(|_| rev.to_owned()),
        dst: rev.to_owned(),
        delete: new.is_none(),
        expected_old: theirs,
        expect_absent: theirs.is_none(),
        ..Default::default()
    }
}

/// Pushes `specs` and names each ref the remote refused, with why.
fn send(
    repo: &Repository,
    remote: &Remote,
    specs: &[PushRefSpec],
    http: &Http,
) -> Result<Vec<String>, StoreError> {
    let out = repo
        .remotes()
        .push(
            remote,
            specs,
            Default::default(),
            &mut NoProgress,
            &NeverCancel,
            Some(http),
        )
        .map_err(g("push"))?;
    Ok(out
        .results
        .iter()
        .filter(|r| !matches!(r.status, PushRefStatus::Ok | PushRefStatus::UpToDate))
        .map(|r| {
            let why = r
                .message
                .as_deref()
                .map_or(String::new(), |m| format!(": {m}"));
            format!("{} ({:?}{why})", r.remote_ref, r.status)
        })
        .collect())
}

/// Pushes `new` to `branch` on `remote`, only if the remote's branch is still `theirs` (absent
/// when `None`), as fetched: another device's push in between is a conflict, never overwritten.
fn push_branch(
    repo: &Repository,
    remote: &Remote,
    branch: &str,
    new: ObjectId,
    theirs: Option<ObjectId>,
    http: &Http,
) -> Result<(), StoreError> {
    let spec = push_spec(branch, Some(new), theirs);
    match send(repo, remote, &[spec], http)?.first() {
        Some(refused) => Err(StoreError::Conflict(format!(
            "the remote did not take {refused}; sync again"
        ))),
        None => Ok(()),
    }
}

/// HTTPS for a remote, with a token as the password: git hosts take any user name with
/// it. No credential helpers or prompts, which an app on a phone does not have.
struct Http(Option<String>);

/// The client for `url`. A token only ever goes over HTTPS: anything else is refused here, the one
/// place every clone, fetch and push passes through.
fn http_for(url: &str, token: Option<&str>) -> Result<Http, StoreError> {
    let token = token.filter(|t| !t.is_empty());
    let https = url
        .trim()
        .get(..8)
        .is_some_and(|s| s.eq_ignore_ascii_case("https://"));
    if token.is_some() && !https {
        return Err(StoreError::Refused(
            "a token is sent only over HTTPS: the remote's URL must start https://".into(),
        ));
    }
    Ok(Http(token.map(str::to_owned)))
}

struct Token(String);

impl CredentialProvider for Token {
    fn fill(&self, input: &Credential) -> grit_lib::error::Result<Credential> {
        let mut c = input.clone();
        c.username = Some("oauth2".to_owned());
        c.password = Some(self.0.clone());
        Ok(c)
    }
    fn approve(&self, _: &Credential) -> grit_lib::error::Result<()> {
        Ok(())
    }
    fn reject(&self, _: &Credential) -> grit_lib::error::Result<()> {
        Ok(())
    }
}

impl HttpClientFactory for Http {
    fn create(&self, _: &ConfigSet) -> RemoteResult<Box<dyn HttpClient>> {
        Ok(Box::new(match &self.0 {
            Some(t) => UreqHttpClient::with_credentials(Box::new(Token(t.clone()))),
            None => UreqHttpClient::new(),
        }))
    }
    fn create_for_probe(&self, config: &ConfigSet) -> RemoteResult<Box<dyn HttpClient>> {
        self.create(config)
    }
}
