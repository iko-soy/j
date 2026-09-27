//! jj-lib backend (§7): repository location (§7.1), building the `Repo`
//! value with snapshotting (§7.2/§7.4), persistence (§7.5), and the lock
//! (§7.7). Reserved commands (fetch/push/undo/ops/init/clone/remote) remain
//! stubs for a later session.

use crate::config::Config;
use crate::domain::{Backend, MetaInfo, ROOT_ID};
use crate::eval::Interp;
use crate::value::{BlobContent, BlobKind, BlobVal, ConflictSide, Crash, LazyBlob, Value};
use futures::TryStreamExt;
use jj_lib::backend::{CopyId, Signature, Timestamp, TreeValue};
use jj_lib::commit::Commit;
use jj_lib::config::{ConfigLayer, ConfigSource, StackedConfig};
use jj_lib::default_backend_factories::{default_backend_factories, default_working_copy_factories};
use jj_lib::gitignore::GitIgnoreFile;
use jj_lib::matchers::{EverythingMatcher, NothingMatcher};
use jj_lib::merge::{Diff, Merge};
use jj_lib::merged_tree::MergedTree;
use jj_lib::merged_tree_builder::MergedTreeBuilder;
use jj_lib::backend::{CommitId, FileId, MergedTreeValueExt, TreeId};
use jj_lib::object_id::ObjectId;
use jj_lib::op_store::OperationId;
use jj_lib::ref_name::{RefName, RemoteName, WorkspaceName, WorkspaceNameBuf};
use jj_lib::repo::{ReadonlyRepo, Repo};
use jj_lib::repo_path::{RepoPath, RepoPathBuf, RepoPathComponentBuf};
use jj_lib::revset::{ResolvedRevsetExpression, RevsetStreamExt};
use jj_lib::settings::UserSettings;
use jj_lib::store::Store;
use jj_lib::transaction::UnpublishedOperation;
use jj_lib::working_copy::{LockedWorkingCopy, SnapshotOptions};
use jj_lib::workspace::Workspace;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::future::Future;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};

fn block_on<F: Future>(f: F) -> F::Output {
    pollster::block_on(f)
}

type OpenError = (u8, String);

// ----------------------------------------------------------------------
// locating and opening the repository (§7.1)
// ----------------------------------------------------------------------

fn find_repo_root() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    for dir in cwd.ancestors() {
        if dir.join(".jj").is_dir() {
            return Some(dir.to_path_buf());
        }
    }
    None
}

fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// UserSettings from a StackedConfig containing only `user.name` and
/// `user.email` (§7.9). Falls back to a placeholder identity when the
/// config's `user` record cannot be evaluated here (undo/redo/ops read no
/// config, §6.1).
fn user_settings(cfg: Option<&Config>) -> UserSettings {
    let (name, email) = match cfg {
        Some(cfg) => crate::config::eval_user_only(&mut Interp::dummy(), cfg)
            .unwrap_or_else(|_| ("j".to_string(), "j".to_string())),
        None => ("j".to_string(), "j".to_string()),
    };
    let mut config = StackedConfig::with_defaults();
    let text = format!(
        "[user]\nname = {}\nemail = {}\n",
        toml_string(&name),
        toml_string(&email)
    );
    let layer =
        ConfigLayer::parse(ConfigSource::User, &text).expect("generated user config should parse");
    config.add_layer(layer);
    UserSettings::from_config(config).expect("generated user config should be valid")
}

// ----------------------------------------------------------------------
// repo -> value mapping (§7.2)
// ----------------------------------------------------------------------

struct StoredCommit {
    commit: Commit,
    change_id: String,
    /// change id of the first jj parent ("" for the root commit)
    first_parent: String,
    /// change ids of all jj parents ("" entries impossible: the root has none)
    all_parents: Vec<String>,
    committer_millis: i64,
}

struct VisibleRepo {
    /// change id -> stored commit
    commits: BTreeMap<String, StoredCommit>,
    /// change id -> remote bookmarks of `origin` pointing at it (§7.6)
    labels: BTreeMap<String, Vec<String>>,
    /// change id -> child change ids (first-parent tree)
    children: BTreeMap<String, Vec<String>>,
    /// the default workspace's working-copy commit, as a change id
    wc_change_id: String,
}

impl Clone for VisibleRepo {
    fn clone(&self) -> Self {
        VisibleRepo {
            commits: self
                .commits
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        StoredCommit {
                            commit: v.commit.clone(),
                            change_id: v.change_id.clone(),
                            first_parent: v.first_parent.clone(),
                            all_parents: v.all_parents.clone(),
                            committer_millis: v.committer_millis,
                        },
                    )
                })
                .collect(),
            labels: self.labels.clone(),
            children: self.children.clone(),
            wc_change_id: self.wc_change_id.clone(),
        }
    }
}

async fn read_visible(repo: &Arc<ReadonlyRepo>) -> Result<VisibleRepo, OpenError> {
    let store = repo.store().clone();
    // visible commits = ancestors (inclusive) of the visible heads (§7.2)
    let mut commits: BTreeMap<String, StoredCommit> = BTreeMap::new();
    let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut count_by_change: BTreeMap<String, usize> = BTreeMap::new();
    let mut visited: BTreeSet<CommitId> = BTreeSet::new();
    let mut stack: Vec<CommitId> = repo.view().heads().iter().cloned().collect();
    while let Some(id) = stack.pop() {
        if !visited.insert(id.clone()) {
            continue;
        }
        let commit = store
            .get_commit_async(&id)
            .await
            .map_err(|e| (2, format!("cannot read commit: {}", e)))?;
        let change_id = commit.change_id().reverse_hex();
        *count_by_change.entry(change_id.clone()).or_default() += 1;
        let mut first_parent = String::new();
        if let Some(first) = commit.parent_ids().first() {
            let parent_commit = store
                .get_commit_async(first)
                .await
                .map_err(|e| (2, format!("cannot read commit: {}", e)))?;
            let pcid = parent_commit.change_id().reverse_hex();
            children.entry(pcid.clone()).or_default().push(change_id.clone());
            first_parent = pcid;
        }
        let parent_ids = commit.parent_ids().to_vec();
        let mut all_parents = Vec::new();
        for pid in &parent_ids {
            let pcommit = store
                .get_commit_async(pid)
                .await
                .map_err(|e| (2, format!("cannot read commit: {}", e)))?;
            all_parents.push(pcommit.change_id().reverse_hex());
        }
        commits.insert(
            change_id.clone(),
            StoredCommit {
                committer_millis: commit.committer().timestamp.timestamp.0,
                commit,
                change_id,
                first_parent,
                all_parents,
            },
        );
        stack.extend(parent_ids);
    }
    // §7.2: one change id on two visible commits is unsupported
    for (change_id, n) in &count_by_change {
        if *n > 1 {
            return Err((
                2,
                format!(
                    "change {} has two visible commits; j cannot operate on this repository",
                    change_id
                ),
            ));
        }
    }

    // labels: remote bookmarks of `origin` pointing at a commit (§7.6)
    let mut labels: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, remote_ref) in repo.view().remote_bookmarks(RemoteName::new("origin")) {
        for cid in remote_ref.target.added_ids() {
            if let Ok(c) = store.get_commit_async(cid).await {
                labels
                    .entry(c.change_id().reverse_hex())
                    .or_default()
                    .push(name.as_str().to_string());
            }
        }
    }

    let wc_commit_id = repo
        .view()
        .get_wc_commit_id(WorkspaceName::DEFAULT)
        .cloned()
        .ok_or_else(|| (2, "the default workspace has no working-copy commit".to_string()))?;
    let wc_commit = store
        .get_commit_async(&wc_commit_id)
        .await
        .map_err(|e| (2, format!("cannot read the working-copy commit: {}", e)))?;

    Ok(VisibleRepo {
        commits,
        labels,
        children,
        wc_change_id: wc_commit.change_id().reverse_hex(),
    })
}

// ----------------------------------------------------------------------
// the backend
// ----------------------------------------------------------------------

pub struct JjInner {
    workspace_root: PathBuf,
    /// the one workspace (and store) for the whole run; every repo load and
    /// working-copy mutation goes through it so trees always live in the
    /// same store
    workspace: Mutex<Workspace>,
    /// repo at the operation the program is working against; updated after a
    /// snapshot so persistence and the snapshot form one operation (§1.2)
    repo: Mutex<Arc<ReadonlyRepo>>,
    workspace_name: WorkspaceNameBuf,
    /// visible map backing the `Backend` lookups during evaluation; filled by
    /// build_interp with the post-snapshot map, or the loaded one when the
    /// snapshot goes into a new child of an immutable focus (§7.2)
    visible: Mutex<Option<Arc<VisibleRepo>>>,
    lock_guard: Mutex<Option<FileLock>>,
    /// the snapshot's unpublished rewrite, kept between build_interp and
    /// persist so the snapshot and the expression's edits form one jj
    /// operation (§1.2); dropped uncommitted by printing runs. The
    /// working-copy lock is not held: its saved state is still the tree
    /// recorded before the snapshot until `persist`'s checkout scans the
    /// directory again
    pending: Mutex<Option<PendingSnapshot>>,
    /// whether the workspace's filesystem folds case, probed on first use
    /// (`Backend::folds_case`)
    folds_case: OnceLock<bool>,
    /// whether the workspace's filesystem holds each name longer than
    /// `NAME_MAX` asked about so far (`Backend::name_fits`)
    long_names: Mutex<HashMap<String, bool>>,
}

/// holding the file keeps the flock
pub struct FileLock(#[allow(dead_code)] std::fs::File);

struct PendingSnapshot {
    /// repo as loaded at head, before the snapshot
    pre_repo: Arc<ReadonlyRepo>,
    /// the snapshot's rewritten wc commit and tree, to be folded into the
    /// persisting operation (§1.2 step 8, §7.7)
    new_wc_id: CommitId,
    tree: MergedTree,
    /// false when the wc commit is immutable: the snapshot then belongs to
    /// the focus's new child (§7.2), and the wc commit is kept as stored
    fold_into_wc: bool,
}

/// How every scan of the working directory reads it (§7.4): each file
/// not ignored by the directory's own `.gitignore`s is tracked, whatever
/// its size. `persist`'s checkout rescans with the same options and
/// relies on getting the snapshot's tree back.
fn snapshot_options() -> SnapshotOptions<'static> {
    SnapshotOptions {
        base_ignores: GitIgnoreFile::empty(),
        progress: None,
        start_tracking_matcher: &EverythingMatcher,
        force_tracking_matcher: &NothingMatcher,
        max_new_file_size: u64::MAX,
    }
}

/// Put back what a checkout that failed part way wrote: scan what it left
/// (the state still describes `on_disk`, so each path it changed reads as
/// changed), then check `on_disk` out over that.
async fn put_back(wc: &mut dyn LockedWorkingCopy, on_disk: &Commit) -> Result<(), String> {
    wc.snapshot(&snapshot_options()).await.map_err(|e| e.to_string())?;
    wc.check_out(on_disk).await.map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Clone)]
pub struct JjBackend {
    inner: Arc<JjInner>,
}

impl JjBackend {
    pub fn open(cfg: Option<&Config>) -> Result<JjBackend, OpenError> {
        let root = find_repo_root().ok_or_else(|| {
            (
                2,
                "no jj repository found in this directory or any parent".to_string(),
            )
        })?;
        let settings = user_settings(cfg);
        let workspace = Workspace::load(
            &settings,
            &root,
            &default_backend_factories(),
            &default_working_copy_factories(),
        )
        .map_err(|e| (2, format!("cannot load the workspace: {}", e)))?;
        // only the default workspace is supported (§7.1)
        if workspace.workspace_name() != WorkspaceName::DEFAULT {
            return Err((
                2,
                format!(
                    "this working copy belongs to workspace `{}`; only the default workspace is supported",
                    workspace.workspace_name().as_str()
                ),
            ));
        }
        let workspace_name = workspace.workspace_name().to_owned();
        let repo = block_on(workspace.repo_loader().load_at_head())
            .map_err(|e| (2, format!("cannot load the repository at head: {}", e)))?;
        Ok(JjBackend {
            inner: Arc::new(JjInner {
                workspace_root: root,
                workspace: Mutex::new(workspace),
                repo: Mutex::new(repo),
                workspace_name,
                visible: Mutex::new(None),
                lock_guard: Mutex::new(None),
                pending: Mutex::new(None),
                folds_case: OnceLock::new(),
                long_names: Mutex::new(HashMap::new()),
            }),
        })
    }

    fn current_repo(&self) -> Arc<ReadonlyRepo> {
        self.inner.repo.lock().unwrap().clone()
    }

    pub fn take_lock(&self) {
        // an flock, not a lockfile: the OS releases it when the process dies,
        // so a killed j (SIGINT, §1.3) never bricks the repository
        let lock_path = self.inner.workspace_root.join(".jj").join("j.lock");
        let file = match std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&lock_path)
        {
            Ok(f) => f,
            Err(e) => {
                crate::report!("j: cannot create {}: {}", lock_path.display(), e);
                std::process::exit(2);
            }
        };
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            crate::report!("j: another j is running in this repository");
            std::process::exit(2);
        }
        *self.inner.lock_guard.lock().unwrap() = Some(FileLock(file));
    }

    pub fn build_interp(
        &self,
        cfg: &mut Config,
        _text: &str,
        snapshot: bool,
    ) -> Result<(Interp, Value, Value), OpenError> {
        // reload at head: a previous run (or another tool) may have advanced
        // the operation since `open`
        let head_repo = block_on(self.inner.workspace.lock().unwrap().repo_loader().load_at_head())
            .map_err(|e| (2, format!("cannot reload the repository at head: {}", e)))?;
        *self.inner.repo.lock().unwrap() = head_repo;
        let loaded_visible = Arc::new(block_on(read_visible(&self.current_repo()))?);
        let loaded = build_repo_value(&loaded_visible, &loaded_visible.wc_change_id)?;

        let current = if snapshot {
            // §7.4: snapshot the working directory into the wc commit; the
            // snapshot operation is written but unpublished — a persisting
            // run folds it into its own operation (§7.7)
            let (post, pending) = block_on(self.snapshot_repo())?;
            match pending {
                // no working-copy change: the repo is the one already loaded,
                // so reuse its value instead of rebuilding it
                None => {
                    *self.inner.visible.lock().unwrap() = Some(loaded_visible.clone());
                    loaded.clone()
                }
                Some(pending) => {
                    *self.inner.pending.lock().unwrap() = Some(pending);
                    let vis = block_on(read_visible(&post))?;
                    let cur = build_repo_value(&vis, &vis.wc_change_id)?;
                    *self.inner.visible.lock().unwrap() = Some(Arc::new(vis));
                    cur
                }
            }
        } else {
            *self.inner.visible.lock().unwrap() = Some(loaded_visible.clone());
            loaded.clone()
        };

        let mut interp = Interp::new(
            Rc::new(self.clone()),
            cfg.shapes.clone(),
            crate::value::Env::empty(),
        );
        // §1.2.5: resolve the config's `@prefix` literals now that the visible
        // ids are known, and before anything evaluates a definition
        if let Some((def, prefix, _)) =
            crate::config::resolve_config_ids(cfg, &interp).into_iter().next()
        {
            return Err((
                3,
                format!(
                    "config.j: `@{}` in `{}` does not resolve to a unique commit",
                    prefix, def
                ),
            ));
        }
        // §7.2: if the focused commit is in the immutable set, the snapshot
        // cannot go into it; the focus is a new empty child of it holding the
        // snapshot, and persisting records that child
        match self.focus_if_immutable(&mut interp, cfg, &loaded, &current)? {
            Some((loaded, current)) => {
                // the snapshot's rewrite of the wc commit (and its rebase of
                // the descendants) is not the repository's: lookups answer
                // from the stored commits, and persisting leaves the wc
                // commit as stored rather than folding the snapshot into it
                *self.inner.visible.lock().unwrap() = Some(loaded_visible);
                if let Some(p) = self.inner.pending.lock().unwrap().as_mut() {
                    p.fold_into_wc = false;
                }
                Ok((interp, loaded, current))
            }
            None => Ok((interp, loaded, current)),
        }
    }

    /// If the focus of `current` is immutable, return the loaded and current
    /// repos refocused on a new empty child of it (§7.2). Both are `loaded`
    /// with that one child added: the snapshot goes into neither the focus
    /// nor, through a rebase, its descendants. The child holds the focus's
    /// stored files in the loaded repo and the snapshot in the current one,
    /// so a dirty `j id` differs from the loaded value and records the child
    /// (§1.2 step 8).
    fn focus_if_immutable(
        &self,
        interp: &mut Interp,
        cfg: &Config,
        loaded: &Value,
        current: &Value,
    ) -> Result<Option<(Value, Value)>, OpenError> {
        // Evaluate the config to compute the immutable set. This uses the
        // interpreter the run will go on to use, rather than a throwaway one,
        // so that the `immutable` (and the `trunk` inside it) computed here is
        // reused when rendering or persisting asks for the same Repo value —
        // each is a full interpreted walk of the history.
        crate::config::eval_config(interp, cfg).map_err(|c| (3, c.msg))?;
        let immutable = crate::repo::compute_immutable(interp, current)
            .map_err(|c| (1, c.msg))?;
        let focus_id = match current.field("root").and_then(|r| r.field("id")) {
            Ok(Value::Id(i)) => i.to_string(),
            _ => return Ok(None),
        };
        if !immutable.contains(&focus_id) {
            return Ok(None);
        }
        // the focus's location in `loaded`, pre-snapshot; the child is its
        // last child, so every existing child sits to the left of it
        let field = |v: &Value, name: &str| v.field(name).map_err(|c| (1, c.msg));
        let root = field(loaded, "root")?;
        let frame = Value::record(&[
            ("left", field(loaded, "children")?),
            ("parent", root.clone()),
            ("right", Value::list(vec![])),
        ]);
        let mut frames = vec![frame];
        frames.extend_from_slice(field(loaded, "context")?.as_list().map_err(|c| (1, c.msg))?);
        let context = Value::list(frames);
        let id = Value::Id(Rc::new(interp.mint_id()));
        let at_child = |files: Value| {
            Value::record(&[
                ("children", Value::list(vec![])),
                ("context", context.clone()),
                (
                    "root",
                    Value::record(&[
                        ("files", files),
                        ("id", id.clone()),
                        ("labels", Value::list(vec![])),
                        ("message", Value::text("")),
                    ]),
                ),
            ])
        };
        let current_at_child = at_child(field(&field(current, "root")?, "files")?);
        // with nothing snapshotted `current` is `loaded` itself: share one
        // value, so a run that changes nothing is a no-op without comparing
        // the whole repository
        let loaded_at_child = match (loaded, current) {
            (Value::Record(a), Value::Record(b)) if Rc::ptr_eq(a, b) => current_at_child.clone(),
            _ => at_child(field(&root, "files")?),
        };
        Ok(Some((loaded_at_child, current_at_child)))
    }

    /// Snapshot the working copy into the wc commit; returns the repo at the
    /// snapshot operation (unpublished but written), or the pre-snapshot repo
    /// if nothing changed.
    async fn snapshot_repo(
        &self,
    ) -> Result<(Arc<ReadonlyRepo>, Option<PendingSnapshot>), OpenError> {
        let mut ws_guard = self.inner.workspace.lock().unwrap();
        let loader = ws_guard.repo_loader().clone();
        let mut locked_ws = ws_guard.start_working_copy_mutation()
            .await
            .map_err(|e| (2, format!("cannot lock the working copy: {}", e)))?;
        let options = snapshot_options();
        let (new_tree, _stats) = locked_ws
            .locked_wc()
            .snapshot(&options)
            .await
            .map_err(|e| (2, format!("cannot snapshot the working copy: {}", e)))?;
        let changed = new_tree.tree_ids() != locked_ws.locked_wc().old_tree().tree_ids();
        if !changed {
            locked_ws
                .finish(self.current_repo().operation().id().clone())
                .await
                .map_err(|e| (2, format!("cannot finish the snapshot: {}", e)))?;
            return Ok((self.current_repo(), None));
        }
        // the snapshotted tree lives in the workspace's store; drive the
        // transaction from the workspace's repo loader so stores match
        let ws_repo = loader
            .load_at_head()
            .await
            .map_err(|e| (2, format!("cannot reload the repository: {}", e)))?;
        let mut tx = ws_repo.start_transaction();
        tx.set_is_snapshot(true);
        let wc_commit = self.wc_commit(&ws_repo).await?;
        let new_wc = tx
            .repo_mut()
            .rewrite_commit(&wc_commit)
            .set_tree(new_tree)
            .write()
            .await
            .map_err(|e| (2, format!("cannot write the snapshot commit: {}", e)))?;
        tx.repo_mut()
            .set_wc_commit(self.inner.workspace_name.clone(), new_wc.id().clone())
            .map_err(|e| (2, format!("cannot set the working-copy commit: {}", e)))?;
        tx.repo_mut()
            .rebase_descendants()
            .await
            .map_err(|e| (2, format!("cannot rebase descendants: {}", e)))?;
        // write but do NOT publish: a persisting run folds the snapshot into
        // its own operation (§7.7); a printing run discards it entirely
        let unpublished = tx
            .write("snapshot working copy")
            .await
            .map_err(|e| (2, format!("cannot write the snapshot operation: {}", e)))?;
        let op_id = unpublished.operation().id().clone();
        let new_repo = unpublished.leave_unpublished();
        // Release the lock *without* finishing: `finish` saves the scanned
        // tree state to disk, which would record the working directory as
        // already snapshotted while the published head still has the old wc
        // tree. The next run's incremental snapshot would then find nothing
        // to do and build the Repo from the stale tree — and a persisting run
        // would check that stale tree back out, destroying the edits. The
        // state is advanced only by `persist`'s checkout, which first scans
        // the directory again and finishes against the operation that
        // actually recorded the tree (§7.4/§7.7).
        drop(locked_ws);
        let _ = op_id;
        let pending = PendingSnapshot {
            pre_repo: self.current_repo(),
            new_wc_id: new_wc.id().clone(),
            tree: new_wc.tree().clone(),
            fold_into_wc: true,
        };
        Ok((new_repo, Some(pending)))
    }



    async fn wc_commit(&self, repo: &Arc<ReadonlyRepo>) -> Result<Commit, OpenError> {
        let id = repo
            .view()
            .get_wc_commit_id(WorkspaceName::DEFAULT)
            .cloned()
            .ok_or_else(|| (2, "no working-copy commit".to_string()))?;
        repo.store()
            .get_commit_async(&id)
            .await
            .map_err(|e| (2, format!("cannot read the working-copy commit: {}", e)))
    }

    pub fn persist(
        &self,
        interp: &mut Interp,
        cfg: &Config,
        loaded: &Value,
        _current: &Value,
        new: &Value,
        text: &str,
    ) -> Result<(), Crash> {
        // §1.2 step 8: unchanged, up to the order of snapshot entries (§7.3)
        if crate::repo::same_repo(loaded, new)? {
            return Ok(());
        }
        *interp.old_repo.borrow_mut() = Some(loaded.clone());
        crate::repo::validate_repo(interp, new)?;

        // fold the snapshot into this operation (§1.2 step 8): start from the
        // pre-snapshot repo and reapply the snapshot's wc rewrite here
        let pending = self.inner.pending.lock().unwrap().take();
        let base = match &pending {
            Some(p) => p.pre_repo.clone(),
            None => self.current_repo(),
        };
        let store = base.store().clone();
        let user_sig = self.user_signature(cfg)?;

        let mut tx = base.start_transaction();
        // the snapshot's wc rewrite, folded into this operation (§1.2): the
        // walk below compares against its output, so the snapshot change and
        // the expression's edits become one rewrite
        let mut snapshot_wc: Option<Commit> = None;
        if let Some(p) = pending.as_ref().filter(|p| p.fold_into_wc) {
            let wc_commit = block_on(self.wc_commit(&base)).map_err(|e| Crash::new(e.1))?;
            let c = block_on(
                tx.repo_mut()
                    .rewrite_commit(&wc_commit)
                    .set_tree(p.tree.clone())
                    .write(),
            )
            .map_err(|e| Crash::new(format!("cannot write the snapshot commit: {}", e)))?;
            snapshot_wc = Some(c);
        }
        // compare against the stored commits of the base operation (the
        // snapshot's unpublished rewrite must not become a predecessor)
        let mut old_stored: Arc<VisibleRepo> =
            Arc::new(block_on(read_visible(&base)).map_err(|e| Crash::new(e.1))?);
        if let Some(c) = &snapshot_wc {
            let mut vis = (*old_stored).clone();
            let change = c.change_id().reverse_hex();
            if let Some(rec) = vis.commits.get_mut(&change) {
                rec.commit = c.clone();
            }
            old_stored = Arc::new(vis);
        }
        let mut written: BTreeMap<String, CommitId> = BTreeMap::new();
        // shared across the whole persist walk so unchanged blobs are inflated once
        let cache = BlobCache::default();
        let entries = EntryCache::default();

        // the root commit must be the top and unchanged (§7.5 step 4)
        let top = crate::repo::by_id(new, ROOT_ID)?
            .and_then(|loc| {
                loc.field("context")
                    .ok()
                    .and_then(|c| c.as_list().ok().map(|l| l.is_empty()))
                    .and_then(|is_top| if is_top { Some(loc) } else { None })
            })
            .ok_or_else(|| Crash::new("persistence: the root commit must be the top"))?;
        let root_jj = block_on(store.get_commit_async(store.root_commit_id()))
            .map_err(|e| Crash::new(format!("cannot read the root commit: {}", e)))?;
        {
            let root_v = top.field("root")?;
            let msg = root_v.field("message")?.as_text()?.to_string();
            if root_jj.description() != msg {
                return Err(Crash::new("persistence: the root commit cannot be changed"));
            }
            let stored_files = block_on(tree_to_files(&store, &root_jj.tree(), &cache, &entries))
                .map_err(|e| Crash::new(e.msg))?;
            if !crate::repo::snapshot_eq(&Value::list(stored_files), &root_v.field("files")?)? {
                return Err(Crash::new("persistence: the root commit cannot be changed"));
            }
        }
        written.insert(ROOT_ID.to_string(), root_jj.id().clone());

        // walk `new` top-down (§7.5 step 4), parents before children
        // a deque, not a Vec: `remove(0)` shifts everything after it, which
        // is quadratic in the number of commits for a wide history
        let mut queue: std::collections::VecDeque<(Value, String)> =
            std::collections::VecDeque::new(); // (subtree value, parent change id)
        for kid in top.field("children")?.as_list()?.iter() {
            queue.push_back((kid.clone(), ROOT_ID.to_string()));
        }
        while let Some((subtree, parent_change)) = queue.pop_front() {
            let commit_v = subtree.field("root")?;
            let id = commit_id_of(&commit_v)?;
            let parent_jj = written
                .get(&parent_change)
                .cloned()
                .ok_or_else(|| Crash::new("persistence: internal: parent not yet written"))?;
            let files_v = commit_v.field("files")?;
            let message = commit_v.field("message")?.as_text()?.to_string();
            let new_id = match old_stored.commits.get(&id) {
                None => {
                    let tree = block_on(build_tree(&store, &files_v))?;
                    let change_id = jj_lib::backend::ChangeId::try_from_reverse_hex(&id).ok_or_else(|| {
                        Crash::new(format!("persistence: `{}` is not a valid change id", id))
                    })?;
                    let c = block_on(
                        tx.repo_mut()
                            .new_commit(vec![parent_jj], tree)
                            .set_change_id(change_id)
                            .set_description(message)
                            .set_author(user_sig.clone())
                            .set_committer(user_sig.clone())
                            .write(),
                    )
                    .map_err(|e| Crash::new(format!("cannot create commit: {}", e)))?;
                    c.id().clone()
                }
                Some(stored) => {
                    let stored_commit = &stored.commit;
                    let parent_changed = hex_of(stored_commit.parent_ids().first()) != parent_jj.hex();
                    let files_changed = {
                        let stored_files =
                            block_on(tree_to_files(&store, &stored_commit.tree(), &cache, &entries))?;
                        // a tree has no order: entries listed differently
                        // are not a change to rewrite (§7.3)
                        !crate::repo::snapshot_eq(&Value::list(stored_files), &files_v)?
                    };
                    let msg_changed = stored_commit.description() != message;
                    if parent_changed || files_changed || msg_changed {
                        let tree = block_on(build_tree(&store, &files_v))?;
                        let c = block_on(
                            tx.repo_mut()
                                .rewrite_commit(stored_commit)
                                .set_parents(vec![parent_jj])
                                .set_tree(tree)
                                .set_description(message)
                                .set_committer(user_sig.clone())
                                .write(),
                        )
                        .map_err(|e| Crash::new(format!("cannot rewrite commit: {}", e)))?;
                        c.id().clone()
                    } else {
                        stored_commit.id().clone()
                    }
                }
            };
            written.insert(id.clone(), new_id);
            for kid in subtree.field("children")?.as_list()?.iter() {
                queue.push_back((kid.clone(), id.clone()));
            }
        }

        // §7.5 step 5: abandon ids in old that are not in new. Children were
        // already re-placed explicitly by the top-down walk above, so record
        // the abandon with no rebase targets (repo.rs:1075); calling
        // record_abandoned_commit instead would rebase our new commits and
        // trip the "descendants not rebased" assert in Transaction::write.
        let new_ids = new_id_set(new)?;
        for (id, stored) in &old_stored.commits {
            if id == ROOT_ID || new_ids.contains(id) {
                continue;
            }
            // abandon (§7.5 step 5): the walk has already re-placed every
            // surviving descendant, so jj's rebase machinery has nothing left
            // to do; the abandon is recorded for evolution bookkeeping
            tx.repo_mut().record_abandoned_commit_with_parents(
                stored.commit.id().clone(),
                [stored.commit.parent_ids()[0].clone()],
            );
        }

        // §7.5 step 6: set the working-copy commit to the focus (validate_repo
        // has already checked mutability)
        let focus_id = commit_id_of(&new.field("root")?)?;
        let focus_jj = written
            .get(&focus_id)
            .cloned()
            .ok_or_else(|| Crash::new("persistence: internal: focus not written"))?;
        tx.repo_mut()
            .set_wc_commit(self.inner.workspace_name.clone(), focus_jj.clone())
            .map_err(|e| Crash::new(format!("cannot set the working-copy commit: {}", e)))?;

        // the walk rewrites commits; descendants already written must not be
        // left dangling (transaction invariant)
        // every descendant of a rewritten or abandoned commit was re-placed
        // by the walk itself, so jj's rebase machinery is a no-op here — but
        // the transaction requires the bookkeeping to be flushed
        block_on(tx.repo_mut().rebase_descendants())
            .map_err(|e| Crash::new(format!("cannot rebase descendants: {}", e)))?;
        let desc = truncate_chars(text, 200);
        let unpublished = block_on(tx.write(desc))
            .map_err(|e| Crash::new(format!("cannot write the operation: {}", e)))?;

        // §7.4/§7.5 step 7: check out the focus to the working directory,
        // and only then publish the operation
        let focus_commit = block_on(store.get_commit_async(&focus_jj))
            .map_err(|e| Crash::new(format!("cannot read the focus commit: {}", e)))?;
        // the working directory holds the snapshot's tree; when it was not
        // folded (an immutable wc commit, §7.2) the snapshot's own unpublished
        // commit carries that tree, and with no snapshot it holds the focus
        // as loaded
        let on_disk = match (snapshot_wc, &pending) {
            (Some(c), _) => c,
            (None, Some(p)) => block_on(store.get_commit_async(&p.new_wc_id))
                .map_err(|e| Crash::new(format!("cannot read the snapshot commit: {}", e)))?,
            (None, None) => block_on(self.wc_commit(&base)).map_err(|e| Crash::new(e.1))?,
        };
        block_on(self.checkout(&focus_commit, Some(&on_disk), Some(unpublished)))?;

        Ok(())
    }

    /// Check `commit` out, then publish `op` and record it as the operation
    /// the working copy is at; with no `op` it stays at the head. `on_disk`
    /// is the commit whose tree the working directory holds: a persisting
    /// run's snapshot (§7.4), or the focus a run starts from. A checkout
    /// that fails publishes nothing (§7.5 step 7, §7.7), and leaves the
    /// saved state as it was, so the next run snapshots the directory into
    /// the commit it came from; what the checkout had already written is
    /// put back first, when the directory held exactly `on_disk`.
    async fn checkout(
        &self,
        commit: &Commit,
        on_disk: Option<&Commit>,
        op: Option<UnpublishedOperation>,
    ) -> Result<(), Crash> {
        let mut ws_guard = self.inner.workspace.lock().unwrap();
        let mut locked_ws = ws_guard.start_working_copy_mutation()
            .await
            .map_err(|e| Crash::new(format!("cannot lock the working copy: {}", e)))?;
        // `snapshot_repo` released its lock without saving what it scanned,
        // so the state just loaded still describes the tree recorded before
        // the edits, and jj checks out by diffing that tree against the new
        // one: a path whose recorded content equals the focus's would keep
        // the user's edit, a file created since would stay, and a file
        // replaced by a directory (or back) would leave state contradicting
        // the tree. Scan the directory again first, as `snapshot_repo` did,
        // so the checkout diffs from what is actually on disk (§7.4). A scan
        // keeps what the state knows of each file where a reset to the
        // snapshot's tree would forget it: a conflict whose sides hold
        // marker-like lines is materialized with longer markers, and a
        // forgotten length makes the next snapshot read those markers back
        // at 7 characters, fail to parse them and record the edited conflict
        // as resolved text, markers and all (§7.4). The scan also tells
        // whether the directory still holds `on_disk`, which a failed
        // checkout is put back to.
        let mut restorable = false;
        if let Some(on_disk) = on_disk {
            match locked_ws.locked_wc().snapshot(&snapshot_options()).await {
                Ok((scanned, _stats)) if scanned.tree_ids() == on_disk.tree().tree_ids() => {
                    restorable = true;
                }
                // the directory changed while the program ran, or can no
                // longer be scanned (an entry appeared, vanished or cannot
                // be read; a failed scan leaves the state as it was):
                // describe `on_disk` instead. A reset writes no file; the
                // paths it changes are re-read by the next run.
                _ => locked_ws
                    .locked_wc()
                    .reset(on_disk)
                    .await
                    .map_err(|e| Crash::new(format!("cannot reset the working copy: {}", e)))?,
            }
        }
        let published = match locked_ws.locked_wc().check_out(commit).await {
            Err(e) => Err(format!("cannot check out the focus: {}", e)),
            Ok(_) => match op {
                None => Ok(None),
                Some(op) => op
                    .publish()
                    .await
                    .map(Some)
                    .map_err(|e| format!("cannot publish the operation: {}", e)),
            },
        };
        let op_id = match published {
            Ok(Some(repo)) => {
                let op_id = repo.operation().id().clone();
                *self.inner.repo.lock().unwrap() = repo;
                op_id
            }
            Ok(None) => self.current_repo().operation().id().clone(),
            Err(mut msg) => {
                // nothing is recorded (§7.7). The lock is dropped without
                // saving, so the state still describes the tree recorded at
                // the head, as it did before the run.
                if let Some(on_disk) = on_disk {
                    let restored = if restorable {
                        put_back(locked_ws.locked_wc(), on_disk).await
                    } else {
                        Err("it changed while the program ran".to_string())
                    };
                    if let Err(e) = restored {
                        msg.push_str(&format!(
                            "; the working directory may hold part of it and could not be put back: {}",
                            e
                        ));
                    }
                }
                return Err(Crash::new(msg));
            }
        };
        // the state names the operation it was checked out at, so it is
        // saved only once that operation is published
        locked_ws
            .finish(op_id)
            .await
            .map_err(|e| Crash::new(format!("cannot finish the checkout: {}", e)))?;
        Ok(())
    }

    fn user_signature(&self, cfg: &Config) -> Result<Signature, Crash> {
        let (name, email) = crate::config::eval_user_only(&mut Interp::dummy(), cfg)?;
        Ok(Signature {
            name,
            email,
            timestamp: Timestamp::now(),
        })
    }

    fn repo_loader(&self) -> jj_lib::repo::RepoLoader {
        self.inner.workspace.lock().unwrap().repo_loader().clone()
    }

    fn head_repo(&self) -> Result<Arc<ReadonlyRepo>, OpenError> {
        block_on(self.repo_loader().load_at_head())
            .map_err(|e| (2, format!("cannot reload the repository at head: {}", e)))
    }

    async fn publish_tx(
        &self,
        tx: jj_lib::transaction::Transaction,
        description: &str,
    ) -> Result<(OperationId, Arc<ReadonlyRepo>), OpenError> {
        let unpublished = tx
            .write(description)
            .await
            .map_err(|e| (1, format!("cannot write the operation: {}", e)))?;
        let op_id = unpublished.operation().id().clone();
        let repo = unpublished
            .publish()
            .await
            .map_err(|e| (1, format!("cannot publish the operation: {}", e)))?;
        *self.inner.repo.lock().unwrap() = repo.clone();
        Ok((op_id, repo))
    }

    fn git_subprocess_options(&self) -> Result<jj_lib::git::GitSubprocessOptions, OpenError> {
        jj_lib::git::GitSubprocessOptions::from_settings(self.current_repo().settings())
            .map_err(|e| (2, format!("cannot read git settings: {}", e)))
    }

    fn git_import_options(&self) -> Result<jj_lib::git::GitImportOptions, OpenError> {
        let settings = jj_lib::git::GitSettings::from_settings(self.current_repo().settings())
            .map_err(|e| (2, format!("cannot read git settings: {}", e)))?;
        Ok(jj_lib::git::GitImportOptions {
            abandon_unreachable_commits: settings.abandon_unreachable_commits,
            record_synthetic_predecessors: settings.record_synthetic_predecessors,
            remote_auto_track_bookmarks: std::collections::HashMap::new(),
        })
    }

    /// fetch all bookmarks/tags of `remote` into the git repo, via the git
    /// subprocess (jj-lib's GitFetch handles remote-name validation and the
    /// refspec retry loop); the fetched refs are imported by the caller
    fn git_fetch_refs(
        &self,
        mut_repo: &mut jj_lib::repo::MutableRepo,
        remote: &RemoteName,
    ) -> Result<(), OpenError> {
        use jj_lib::str_util::StringExpression;
        let expr = jj_lib::git::GitFetchRefExpression {
            bookmark: StringExpression::all(),
            tag: StringExpression::all(),
        };
        let expanded = jj_lib::git::expand_fetch_refspecs(remote, expr)
            .map_err(|e| (1, format!("cannot expand the fetch refspecs: {}", e)))?;
        let subprocess_options = self.git_subprocess_options()?;
        let import_options = jj_lib::git::GitImportOptions {
            abandon_unreachable_commits: true,
            record_synthetic_predecessors: true,
            remote_auto_track_bookmarks: std::collections::HashMap::new(),
        };
        let mut git_fetch = jj_lib::git::GitFetch::new(mut_repo, subprocess_options, &import_options)
            .map_err(|_| (2, "the repository does not have a git backend".to_string()))?;
        let mut callback = QuietGitCallback;
        match git_fetch.fetch(remote, expanded, &mut callback, None) {
            Ok(()) => Ok(()),
            Err(jj_lib::git::GitFetchError::NoSuchRemote(_)) => {
                // the backend's cached gix handle can be stale when the
                // remote was just written to .git/config in this same run;
                // fall back to a direct `git fetch` subprocess
                let store = self.current_repo().store().clone();
                let backend = git_backend(&store)?;
                if remote_has_url(backend, remote) {
                    git_fetch_subprocess(backend.git_repo_path(), remote)
                } else {
                    Err((1, format!("no remote {}", remote.as_str())))
                }
            }
            Err(other) => Err((1, format!("fetch failed: {}", other))),
        }
    }

    pub fn cmd_remote(&self, url: &str) -> Result<(), OpenError> {
        self.take_lock();
        let base = self.current_repo();
        let origin = RemoteName::new("origin");
        let exists = {
            let git_repo = jj_lib::git::get_git_repo(base.store())
                .map_err(|_| (2, "the repository does not have a git backend".to_string()))?;
            git_repo.try_find_remote(origin.as_str()).is_some()
        };
        if exists {
            jj_lib::git::set_remote_urls(base.store(), origin, Some(url), None)
        } else {
            let mut tx = base.start_transaction();
            jj_lib::git::add_remote(tx.repo_mut(), origin, url, None)
        }
        .map_err(|e| (1, format!("cannot set the remote: {}", e)))
    }

    pub fn cmd_fetch(&self) -> Result<(), OpenError> {
        let origin = RemoteName::new("origin");
        let base = self.head_repo()?;
        let mut tx = base.start_transaction();
        self.git_fetch_refs(tx.repo_mut(), origin)?;
        let import_options = self.git_import_options()?;
        block_on(jj_lib::git::import_refs(tx.repo_mut(), &import_options))
            .map_err(|e| (1, format!("cannot import the fetched refs: {}", e)))?;
        block_on(tx.repo_mut().rebase_descendants())
            .map_err(|e| (1, format!("cannot rebase descendants: {}", e)))?;
        let _ = block_on(self.publish_tx(tx, "fetch"))?;
        Ok(())
    }
    pub fn cmd_push(&self, cfg: &mut Config, expr_text: &str) -> Result<(), OpenError> {
        let origin = RemoteName::new("origin");
        let base = self.head_repo()?;
        let (value, vis, immutable) = self.eval_push_expr(cfg, expr_text, &base)?;
        let records = parse_push_records(&value, &vis, &base)?;
        check_push_records(&records, &base, &vis, &immutable)?;
        let _ = self.push_to_origin(&base, origin, &records, expr_text)?;
        Ok(())
    }

    /// evaluate `EXPR` against the recorded repository, without snapshotting
    /// (§1.1), apply a function result to the repo value once (§7.6); also
    /// return the immutable set of that repository (§7.5 step 3)
    fn eval_push_expr(
        &self,
        cfg: &mut Config,
        expr_text: &str,
        base: &Arc<ReadonlyRepo>,
    ) -> Result<(Value, Arc<VisibleRepo>, BTreeSet<String>), OpenError> {
        let outer = Rc::new(cfg.global_names.clone());
        let expr = crate::parse::parse_expr(expr_text, outer)
            .map_err(|p| (3, format!("line {}: {}", p.line, p.msg)))?;
        let (mut interp, _loaded, current) = self.build_interp(cfg, expr_text, false)?;
        let expr = crate::config::resolve_ids(&expr, &interp).map_err(|failures| {
            let (prefix, candidates) = &failures[0];
            if candidates.is_empty() {
                (1, format!("crash: `@{}` matches no commit", prefix))
            } else {
                (1, format!("crash: `@{}` is ambiguous", prefix))
            }
        })?;
        crate::config::eval_config(&mut interp, cfg).map_err(|c| (3, format!("config.j: {}", c.msg)))?;
        // the set a push may not move a bookmark out of (§7.6); build_interp
        // evaluated it for this value to place the focus, so it is cached
        let immutable = crate::repo::compute_immutable(&mut interp, &current)
            .map_err(|c| (1, format!("crash: {}", c.msg)))?;
        let env = interp.global_env();
        let expr_rc = Rc::new(expr);
        let mut v = interp
            .eval(&expr_rc, &env)
            .map_err(|c| (1, format!("crash: {}", c.msg)))?;
        if matches!(v, Value::Fun(_)) {
            v = interp
                .apply(v, current)
                .map_err(|c| (1, format!("crash: {}", c.msg)))?;
        }
        let _ = base;
        let vis = self
            .inner
            .visible
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| (2, "internal: repository value not loaded".to_string()))?;
        Ok((v, vis, immutable))
    }

    /// push the updates to `origin` and record the new remote-bookmark
    /// positions as one operation (§7.6)
    fn push_to_origin(
        &self,
        base: &Arc<ReadonlyRepo>,
        origin: &RemoteName,
        records: &[(String, Option<CommitId>)],
        description: &str,
    ) -> Result<Arc<ReadonlyRepo>, OpenError> {
        let mut targets = jj_lib::git::GitPushRefTargets::default();
        for (name, after) in records {
            let before = base
                .view()
                .get_remote_bookmark(RefName::new(name).to_remote_symbol(origin))
                .target
                .as_resolved()
                .cloned()
                .unwrap_or(None);
            targets
                .bookmarks
                .push((jj_lib::ref_name::RefNameBuf::from(name.clone()), Diff::new(before, after.clone())));
        }
        let mut tx = base.start_transaction();
        if records.is_empty() {
            // nothing to set or delete; push_refs would still run `git push
            // origin` without refspecs, which pushes whatever git's
            // push.default picks
            if !remote_has_url(git_backend(base.store())?, origin) {
                return Err((1, "no remote origin".to_string()));
            }
            let (_, repo) = block_on(self.publish_tx(tx, &truncate_chars(description, 200)))?;
            return Ok(repo);
        }
        let subprocess_options = self.git_subprocess_options()?;
        let mut callback = QuietGitCallback;
        let stats = jj_lib::git::push_refs(
            tx.repo_mut(),
            subprocess_options,
            origin,
            &targets,
            &mut callback,
            &jj_lib::git::GitPushOptions::default(),
        )
        .map_err(|e| match e {
            jj_lib::git::GitPushError::NoSuchRemote(_) => (1, "no remote origin".to_string()),
            other => (1, format!("push failed: {}", other)),
        })?;
        // git push is not atomic: the remote may accept some updates and
        // reject others. push_refs has set the accepted ones in `tx`, and
        // they are on the remote, so they are recorded before the rest is
        // reported; a push the remote refused entirely records nothing
        let mut repo = base.clone();
        if stats.all_ok() || stats.some_exported() {
            let unpublished = block_on(tx.write(truncate_chars(description, 200)))
                .map_err(|e| (1, format!("cannot write the operation: {}", e)))?;
            repo = block_on(unpublished.publish())
                .map_err(|e| (1, format!("cannot publish the operation: {}", e)))?;
            *self.inner.repo.lock().unwrap() = repo.clone();
        }
        if !stats.all_ok() {
            let mut names: Vec<String> = Vec::new();
            for (name, _) in stats.rejected.iter().chain(stats.remote_rejected.iter()) {
                names.push(name.as_str().to_string());
            }
            // pushed, but not exported to the colocated git repo, so jj-lib
            // left them out of `tx`
            for (symbol, _) in &stats.unexported_bookmarks {
                names.push(format!("{} (pushed but not recorded)", symbol.name.as_str()));
            }
            let mut msg = format!("push rejected: {}", names.join(", "));
            if stats.some_exported() {
                msg.push_str("; the other bookmarks were pushed and recorded");
            }
            return Err((1, msg));
        }
        Ok(repo)
    }

    pub fn cmd_undo(&self, redo: bool) -> Result<(), OpenError> {
        let base = self.head_repo()?;
        self.check_wc_clean(&base)?;
        let loader = self.repo_loader();
        let mut cur = base.operation().clone();
        let target_op_id: OperationId;
        loop {
            match (op_marker(cur.metadata()), redo) {
                (None, false) => {
                    let Some(parent) = cur.parent_ids().first().cloned() else {
                        return Err((1, "nothing to undo".to_string()));
                    };
                    target_op_id = parent;
                    break;
                }
                (None, true) => return Err((1, "nothing to redo".to_string())),
                (Some(m), false) if m.kind == "undo" => {
                    if !m.authored_by_j {
                        // degrade (§7.7): cannot follow the marker, so undo
                        // this operation's own parent
                        let Some(parent) = cur.parent_ids().first().cloned() else {
                            return Err((1, "nothing to undo".to_string()));
                        };
                        target_op_id = parent;
                        break;
                    }
                    let Some(grandparent) = op_parent_id(&loader, &m.id)? else {
                        return Err((1, "nothing to undo".to_string()));
                    };
                    cur = block_on(loader.load_operation(&grandparent))
                        .map_err(|e| (2, format!("cannot load an operation: {}", e)))?;
                }
                (Some(m), true) if m.kind == "undo" => {
                    target_op_id = m.id;
                    break;
                }
                (Some(m), false) => {
                    // a redo: "move past the operation it reapplied" (§7.7).
                    // The redo marker names the *undo* it reversed; the
                    // operation that undo discarded is the one this redo put
                    // back, so that is where the walk continues. Stopping at
                    // the undo instead would make the loop treat it as an undo
                    // to follow and step back one operation too far.
                    if !m.authored_by_j {
                        let Some(parent) = cur.parent_ids().first().cloned() else {
                            return Err((1, "nothing to undo".to_string()));
                        };
                        target_op_id = parent;
                        break;
                    }
                    let undo_op = block_on(loader.load_operation(&m.id))
                        .map_err(|e| (2, format!("cannot load an operation: {}", e)))?;
                    cur = match op_marker(undo_op.metadata()) {
                        Some(u) if u.kind == "undo" && u.authored_by_j => {
                            block_on(loader.load_operation(&u.id))
                                .map_err(|e| (2, format!("cannot load an operation: {}", e)))?
                        }
                        // cannot follow the marker: degrade to the undo itself
                        _ => undo_op,
                    };
                }
                (Some(m), true) => {
                    // a redo: the undo it reversed is already accounted for,
                    // so keep looking outward from that undo's parent for an
                    // undo that has not been reversed yet (§7.7: the head must
                    // be an undo, "possibly reached through redos").
                    if !m.authored_by_j {
                        target_op_id = m.id;
                        break;
                    }
                    let Some(prev) = op_parent_id(&loader, &m.id)? else {
                        return Err((1, "nothing to redo".to_string()));
                    };
                    cur = block_on(loader.load_operation(&prev))
                        .map_err(|e| (2, format!("cannot load an operation: {}", e)))?;
                }
            }
        }
        // before the operation init or clone recorded there was no
        // repository (§7.7); the view jj recorded then has an empty child of
        // the root as the working-copy commit, and checking it out would
        // delete every file of the head
        if !redo && op_created_repo(cur.metadata()) {
            return Err((1, "nothing to undo".to_string()));
        }
        let target_op = block_on(loader.load_operation(&target_op_id))
            .map_err(|e| (2, format!("cannot load an operation: {}", e)))?;
        let target_view = block_on(
            loader
                .op_store()
                .read_view(target_op.view_id()),
        )
        .map_err(|e| (2, format!("cannot load an operation view: {}", e)))?;
        // undoing the operation that established the working copy (e.g. the
        // very first operation after init) would restore a view with no
        // checked-out commit, leaving the repository unusable; there is
        // nothing meaningful to undo
        if !target_view
            .wc_commit_ids
            .contains_key(WorkspaceName::DEFAULT)
        {
            return Err((
                1,
                if redo {
                    "nothing to redo".to_string()
                } else {
                    "nothing to undo".to_string()
                },
            ));
        }
        let marker = if redo {
            format!("redo: {}", cur.id().hex())
        } else {
            format!("undo: {}", cur.id().hex())
        };
        let mut tx = base.start_transaction();
        tx.repo_mut().set_view(target_view.clone());
        let unpublished = block_on(tx.write(&marker))
            .map_err(|e| (1, format!("cannot write the operation: {}", e)))?;
        // the restored focus is checked out before the operation is
        // published, so a checkout that fails records nothing (§7.7)
        let wc_id = &target_view.wc_commit_ids[WorkspaceName::DEFAULT];
        let wc_commit = block_on(base.store().get_commit_async(wc_id))
            .map_err(|e| (2, format!("cannot read the working-copy commit: {}", e)))?;
        let on_disk = block_on(self.wc_commit(&base))?;
        block_on(self.checkout(&wc_commit, Some(&on_disk), Some(unpublished)))
            .map_err(|c| (1, c.msg))?;
        Ok(())
    }

    /// refuse if the working directory differs from the focused commit's
    /// files (§7.7): reserved commands never snapshot, so compare the locked
    /// working copy's recorded tree with the focus commit's tree
    fn check_wc_clean(&self, base: &Arc<ReadonlyRepo>) -> Result<(), OpenError> {
        let dirty = (1, "working copy has changes not in @; run `j id` to record them or discard them".to_string());
        let wc_commit = block_on(self.wc_commit(base))?;
        // snapshot the working directory (without persisting anything) and
        // compare against the focused commit's files (§7.7)
        let mut ws_guard = self.inner.workspace.lock().unwrap();
        let mut locked_ws = block_on(ws_guard.start_working_copy_mutation())
            .map_err(|e| (2, format!("cannot lock the working copy: {}", e)))?;
        let options = snapshot_options();
        let (new_tree, _stats) = block_on(locked_ws.locked_wc().snapshot(&options))
            .map_err(|e| (2, format!("cannot read the working copy: {}", e)))?;
        let same = new_tree.tree_ids() == wc_commit.tree().tree_ids();
        if same {
            // the scanned tree is the one already recorded: saving the state
            // only refreshes the mtime cache
            block_on(locked_ws.finish(base.operation().id().clone()))
                .map_err(|e| (2, format!("cannot finish: {}", e)))?;
        } else {
            // the working copy differs from @ and this command records
            // nothing: drop the lock without saving, or the state would claim
            // the directory was snapshotted and hide the changes from the next
            // run (§7.7)
            drop(locked_ws);
        }
        drop(ws_guard);
        if !same {
            return Err(dirty);
        }
        Ok(())
    }

    /// The operation log for `ops` (§7.7), one line per operation, newest
    /// first; the caller writes it to stdout.
    pub fn cmd_ops(&self) -> Result<String, OpenError> {
        let base = self.head_repo()?;
        let loader = self.repo_loader();
        let head = base.operation().clone();
        let mut out = String::new();
        let mut cur = Some(head);
        while let Some(op) = cur {
            let end = op.metadata().time.end.timestamp.0.div_euclid(1000);
            let age = crate::render::render_age(end);
            let mut desc = op.metadata().description.trim().to_string();
            let mut tag = "";
            match op_kind(op.metadata()) {
                OpKind::Normal => {}
                OpKind::Undo { .. } => tag = "undo",
                OpKind::Redo { .. } => tag = "redo",
            }
            if desc.is_empty() {
                desc = "(no description)".to_string();
            }
            let mark = if op.id() == base.operation().id() {
                "*"
            } else {
                " "
            };
            if tag.is_empty() {
                out.push_str(&format!("{} {:>4}  {}\n", mark, age, desc));
            } else {
                out.push_str(&format!("{} {:>4}  {}  ({})\n", mark, age, desc, tag));
            }
            cur = match op.parent_ids().first() {
                Some(p) => Some(
                    block_on(loader.load_operation(p))
                        .map_err(|e| (2, format!("cannot load an operation: {}", e)))?,
                ),
                None => None,
            };
        }
        Ok(out)
    }
}

pub fn cmd_init(cfg: &Config) -> Result<(), OpenError> {
    let settings = user_settings_strict(cfg)?;
    let cwd = std::env::current_dir()
        .map_err(|e| (2, format!("cannot read the current directory: {}", e)))?;
    for dir in cwd.ancestors() {
        if dir.join(".jj").is_dir() {
            return Err((
                2,
                format!("already inside a jj repository at {}", dir.display()),
            ));
        }
    }
    let git_dir = cwd.join(".git");
    if git_dir.exists() && !git_dir.is_dir() {
        return Err((
            2,
            ".git exists but is not a directory (a git worktree or submodule)".to_string(),
        ));
    }
    let had_git = git_dir.is_dir();
    let (workspace, repo) = if had_git {
        // adopt the existing git repository (§7.8); init_colocated_git
        // refuses to init over one
        block_on(Workspace::init_external_git(&settings, &cwd, &git_dir))
    } else {
        block_on(Workspace::init_colocated_git(
            &settings,
            &cwd,
            gix::hash::Kind::Sha1,
        ))
    }
    .map_err(|e| (2, format!("cannot create the repository: {}", e)))?;
    let backend = JjBackend {
        inner: Arc::new(JjInner {
            workspace_root: cwd.clone(),
            workspace: Mutex::new(workspace),
            repo: Mutex::new(repo),
            workspace_name: WorkspaceName::DEFAULT.to_owned(),
            visible: Mutex::new(None),
            lock_guard: Mutex::new(None),
            pending: Mutex::new(None),
            folds_case: OnceLock::new(),
            long_names: Mutex::new(HashMap::new()),
        }),
    };
    // the import and the working-copy commit are one operation, the one
    // undo never undoes (§7.7)
    let mut tx = backend.current_repo().start_transaction();
    if had_git {
        let import_options = backend.git_import_options()?;
        block_on(jj_lib::git::import_refs(tx.repo_mut(), &import_options))
            .map_err(|e| (2, format!("cannot import the git history: {}", e)))?;
        // HEAD too: it is the head the working-copy commit starts on, and a
        // detached HEAD's commits are reachable from no ref
        block_on(jj_lib::git::import_head(
            tx.repo_mut(),
            &backend.inner.workspace_name,
            &cwd,
        ))
        .map_err(|e| (2, format!("cannot import the git HEAD: {}", e)))?;
        block_on(tx.repo_mut().rebase_descendants())
            .map_err(|e| (2, format!("cannot rebase descendants: {}", e)))?;
    }
    // §7.8: the current head is git's HEAD (none on an unborn branch)
    let head = tx
        .repo()
        .view()
        .git_head(&backend.inner.workspace_name)
        .as_resolved()
        .cloned()
        .flatten();
    create_initial_wc_commit(&backend, cfg, tx, "init", head, had_git)?;
    Ok(())
}

pub fn cmd_clone(cfg: &Config, url: &str, dir: &str) -> Result<(), OpenError> {
    let settings = user_settings_strict(cfg)?;
    let dir_path = PathBuf::from(dir);
    if dir_path.join(".jj").is_dir() {
        return Err((2, format!("{} already contains a jj repository", dir)));
    }
    if dir_path.is_dir() {
        let mut entries = std::fs::read_dir(&dir_path)
            .map_err(|e| (2, format!("cannot read {}: {}", dir, e)))?;
        if entries.next().is_some() {
            return Err((2, format!("{} already exists and is not empty", dir)));
        }
    } else if dir_path.exists() {
        return Err((2, format!("{} already exists and is not empty", dir)));
    }
    let existed = dir_path.is_dir();
    std::fs::create_dir_all(&dir_path)
        .map_err(|e| (2, format!("cannot create {}: {}", dir, e)))?;
    let cloned = clone_into(cfg, &settings, url, &dir_path);
    if cloned.is_err() {
        // like `git clone`, a clone that fails leaves nothing behind: its
        // working-copy commit would claim files the checkout never wrote,
        // and a later clone would refuse the directory (§7.8)
        if !existed {
            let _ = std::fs::remove_dir_all(&dir_path);
        } else if let Ok(entries) = std::fs::read_dir(&dir_path) {
            for entry in entries.flatten() {
                let _ = match entry.file_type() {
                    Ok(t) if t.is_dir() => std::fs::remove_dir_all(entry.path()),
                    _ => std::fs::remove_file(entry.path()),
                };
            }
        }
    }
    cloned
}

/// `clone`'s work once `dir_path` exists and is empty (§7.8)
fn clone_into(
    cfg: &Config,
    settings: &UserSettings,
    url: &str,
    dir_path: &std::path::Path,
) -> Result<(), OpenError> {
    let (workspace, repo) = block_on(Workspace::init_colocated_git(
        settings,
        dir_path,
        gix::hash::Kind::Sha1,
    ))
    .map_err(|e| (2, format!("cannot create the repository: {}", e)))?;
    let backend = JjBackend {
        inner: Arc::new(JjInner {
            workspace_root: dir_path.to_path_buf(),
            workspace: Mutex::new(workspace),
            repo: Mutex::new(repo),
            workspace_name: WorkspaceName::DEFAULT.to_owned(),
            visible: Mutex::new(None),
            lock_guard: Mutex::new(None),
            pending: Mutex::new(None),
            folds_case: OnceLock::new(),
            long_names: Mutex::new(HashMap::new()),
        }),
    };
    let origin = RemoteName::new("origin");
    let mut tx = backend.current_repo().start_transaction();
    jj_lib::git::add_remote(tx.repo_mut(), origin, url, None)
        .map_err(|e| (1, format!("cannot set the remote: {}", e)))?;
    backend.git_fetch_refs(tx.repo_mut(), origin)?;
    let default_branch =
        git_default_branch(git_backend(tx.repo().store())?.git_repo_path(), origin)?;
    let import_options = backend.git_import_options()?;
    block_on(jj_lib::git::import_refs(tx.repo_mut(), &import_options))
        .map_err(|e| (1, format!("cannot import the fetched refs: {}", e)))?;
    block_on(tx.repo_mut().rebase_descendants())
        .map_err(|e| (1, format!("cannot rebase descendants: {}", e)))?;
    // §7.8: the default bookmark's target (none when the remote's HEAD names
    // no branch, or a branch it does not have)
    let head = default_branch.and_then(|name| {
        tx.repo()
            .view()
            .get_remote_bookmark(RefName::new(&name).to_remote_symbol(origin))
            .target
            .as_resolved()
            .cloned()
            .flatten()
    });
    // the fetch and the working-copy commit are one operation, the one undo
    // never undoes (§7.7)
    create_initial_wc_commit(&backend, cfg, tx, &format!("clone {}", url), head, false)?;
    Ok(())
}

/// §7.8: after init/clone, create a working-copy commit as a child of
/// `head` (the root commit when there is none), check it out, and only
/// then record it in `tx` with what the command has imported. The commit
/// holds its parent's files less any a checkout cannot create
/// (`checkout_tree`), so it is empty unless the parent has one. `adopt`
/// says the working directory already holds a checkout of the parent (init
/// over an existing git repository): the working copy is then reset to the
/// commit instead of written, so no file is touched and what the directory
/// holds beyond the parent is the next snapshot's change.
fn create_initial_wc_commit(
    backend: &JjBackend,
    cfg: &Config,
    mut tx: jj_lib::transaction::Transaction,
    description: &str,
    head: Option<CommitId>,
    adopt: bool,
) -> Result<(), OpenError> {
    let store = tx.base_repo().store().clone();
    let parent = head.unwrap_or_else(|| store.root_commit_id().clone());
    // jj's own init already created a working-copy commit; if it is already
    // a child of the target parent, keep it and just check it out
    let mut to_abandon: Option<CommitId> = None;
    let existing = tx.repo().view().get_wc_commit_id(&backend.inner.workspace_name).cloned();
    if let Some(existing) = existing.and_then(|id| block_on(store.get_commit_async(&id)).ok()) {
        if existing.parent_ids().first() == Some(&parent) {
            // jj's commit is an empty child of the root; record only what
            // was imported, if anything
            let op = if tx.repo().has_changes() {
                Some(
                    block_on(tx.write(truncate_chars(description, 200)))
                        .map_err(|e| (1, format!("cannot write the operation: {}", e)))?,
                )
            } else {
                None
            };
            block_on(backend.checkout(&existing, None, op)).map_err(|c| (1, c.msg))?;
            return Ok(());
        }
        if existing.id() != store.root_commit_id() {
            to_abandon = Some(existing.id().clone());
        }
    }
    let parent_tree = block_on(store.get_commit_async(&parent))
        .map_err(|e| (1, format!("cannot read the parent commit: {}", e)))?
        .tree();
    let wc_tree = block_on(checkout_tree(parent_tree, backend))?;
    let user_sig = backend
        .user_signature(cfg)
        .map_err(|c| (3, format!("config.j: {}", c.msg)))?;
    if let Some(old_wc) = &to_abandon {
        tx.repo_mut().record_abandoned_commit_with_parents(
            old_wc.clone(),
            [store.root_commit_id().clone()],
        );
    }
    let wc = block_on(
        tx.repo_mut()
            .new_commit(vec![parent], wc_tree)
            .set_author(user_sig.clone())
            .set_committer(user_sig)
            .write(),
    )
    .map_err(|e| (1, format!("cannot create the working-copy commit: {}", e)))?;
    tx.repo_mut()
        .set_wc_commit(backend.inner.workspace_name.clone(), wc.id().clone())
        .map_err(|e| (1, format!("cannot set the working-copy commit: {}", e)))?;
    block_on(tx.repo_mut().rebase_descendants())
        .map_err(|e| (1, format!("cannot rebase descendants: {}", e)))?;
    let unpublished = block_on(tx.write(truncate_chars(description, 200)))
        .map_err(|e| (1, format!("cannot write the operation: {}", e)))?;
    // an adopted directory is reset to the commit, not checked out over:
    // checking out from the empty tree jj's init recorded would recreate
    // the files the user deleted there. Anything else is written, and the
    // operation published only once it is (§7.7).
    let on_disk = if adopt { Some(&wc) } else { None };
    block_on(backend.checkout(&wc, on_disk, Some(unpublished))).map_err(|c| (1, c.msg))?;
    Ok(())
}

/// `tree` without the files at a path a checkout cannot create (§7.5 step
/// 1), which a git branch can hold: a `.jj` directory committed by mistake,
/// a name the workspace's filesystem does not hold. jj's checkout stops at
/// the first such path, failing the whole clone, so the working-copy commit
/// init or clone starts on leaves them out, and their removal is its change
/// (§7.8).
async fn checkout_tree(tree: MergedTree, backend: &JjBackend) -> Result<MergedTree, OpenError> {
    let folds_case = backend.folds_case();
    let refused: Vec<RepoPathBuf> = tree
        .entries()
        .map(|(path, _)| path)
        .filter(|path| {
            path.components().any(|c| {
                crate::repo::checkout_refuses(c.as_internal_str(), folds_case, |name| backend.name_fits(name))
            })
        })
        .collect();
    if refused.is_empty() {
        return Ok(tree);
    }
    let mut builder = MergedTreeBuilder::new(tree);
    for path in refused {
        builder.set_or_remove(path, Merge::absent());
    }
    builder
        .write_tree()
        .await
        .map_err(|e| (1, format!("cannot write a tree: {}", e)))
}

/// user settings for init/clone: a missing or malformed `user` is a
/// configuration error (exit 3) (§7.9)
fn user_settings_strict(cfg: &Config) -> Result<UserSettings, OpenError> {
    let (name, email) = crate::config::eval_user_only(&mut Interp::dummy(), cfg)
        .map_err(|c| (3, format!("config.j: {}", c.msg)))?;
    let mut config = StackedConfig::with_defaults();
    let text = format!(
        "[user]\nname = {}\nemail = {}\n",
        toml_string(&name),
        toml_string(&email)
    );
    let layer =
        ConfigLayer::parse(ConfigSource::User, &text).expect("generated user config should parse");
    config.add_layer(layer);
    UserSettings::from_config(config)
        .map_err(|e| (3, format!("config.j: invalid user settings: {}", e)))
}

// ----------------------------------------------------------------------
// reserved-command helpers
// ----------------------------------------------------------------------

/// discards git subprocess sideband/progress output
struct QuietGitCallback;

impl jj_lib::git::GitSubprocessCallback for QuietGitCallback {
    fn needs_progress(&self) -> bool {
        false
    }
    fn progress(&mut self, _progress: &jj_lib::git::GitProgress) -> std::io::Result<()> {
        Ok(())
    }
    fn local_sideband(
        &mut self,
        _message: &[u8],
        _term: Option<jj_lib::git::GitSidebandLineTerminator>,
    ) -> std::io::Result<()> {
        Ok(())
    }
    fn remote_sideband(
        &mut self,
        _message: &[u8],
        _term: Option<jj_lib::git::GitSidebandLineTerminator>,
    ) -> std::io::Result<()> {
        Ok(())
    }
}

fn git_backend(store: &Store) -> Result<&jj_lib::git_backend::GitBackend, OpenError> {
    jj_lib::git::get_git_backend(store)
        .map_err(|_| (2, "the repository does not have a git backend".to_string()))
}

/// whether the remote is configured in .git/config, seen through a fresh
/// gix handle (the backend's cached handle may predate a just-written config)
fn remote_has_url(backend: &jj_lib::git_backend::GitBackend, remote: &RemoteName) -> bool {
    let Ok(fresh) = gix::open(backend.git_repo_path()) else {
        return false;
    };
    match fresh.try_find_remote(remote.as_str()) {
        Some(Ok(r)) => {
            r.url(gix::remote::Direction::Fetch).is_some()
                || r.url(gix::remote::Direction::Push).is_some()
        }
        _ => false,
    }
}

/// `git fetch <remote>` + prune, with the backend's configured git binary
fn git_fetch_subprocess(git_dir: &std::path::Path, remote: &RemoteName) -> Result<(), OpenError> {
    let exe = std::env::var("J_GIT").unwrap_or_else(|_| "git".to_string());
    let out = std::process::Command::new(exe)
        .arg("--git-dir")
        .arg(git_dir)
        .args(["fetch", "--prune", remote.as_str()])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| (1, format!("cannot run git fetch: {}", e)))?;
    if out.status.success() {
        Ok(())
    } else {
        Err((
            1,
            format!(
                "fetch failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ))
    }
}

/// the branch the remote's HEAD names, its default bookmark (§7.8), asked of
/// the remote with `git ls-remote --symref` (the git `git_fetch_subprocess`
/// runs); `None` when HEAD names no branch (a detached HEAD)
fn git_default_branch(
    git_dir: &std::path::Path,
    remote: &RemoteName,
) -> Result<Option<String>, OpenError> {
    let exe = std::env::var("J_GIT").unwrap_or_else(|_| "git".to_string());
    let out = std::process::Command::new(exe)
        .arg("--git-dir")
        .arg(git_dir)
        .args(["ls-remote", "--symref", remote.as_str(), "HEAD"])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| (1, format!("cannot run git ls-remote: {}", e)))?;
    if !out.status.success() {
        return Err((
            1,
            format!(
                "cannot read the remote's default branch: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ));
    }
    // `ref: refs/heads/<branch>\tHEAD`
    Ok(String::from_utf8_lossy(&out.stdout).lines().find_map(|line| {
        let branch = line.strip_prefix("ref: refs/heads/")?.strip_suffix("\tHEAD")?;
        Some(branch.to_string())
    }))
}

enum OpKind {
    Normal,
    /// the operation this undo discarded the view of
    Undo { #[allow(dead_code)] undid: OperationId },
    /// the undo this redo reversed
    Redo { #[allow(dead_code)] redid: OperationId },
}

/// a marker parsed from an operation description (§7.7 kinds)
struct OpMarker {
    kind: &'static str,
    id: OperationId,
    /// whether the description is j's own exact marker (`undo: <id>` /
    /// `redo: <id>`); only those are reliably loadable for the walk
    authored_by_j: bool,
}

/// §7.7 kinds: `j` marks undo/redo operations as `undo: <opid>` /
/// `redo: <opid>` in the description (0.45's Transaction cannot write
/// metadata tags); jj's own `undo operation <id>` / `redo operation <id>`
/// markers are treated the same way.
fn op_kind(meta: &jj_lib::op_store::OperationMetadata) -> OpKind {
    match op_marker(meta) {
        Some(OpMarker { kind: "undo", id, .. }) => OpKind::Undo { undid: id },
        Some(OpMarker { kind: "redo", id, .. }) => OpKind::Redo { redid: id },
        _ => OpKind::Normal,
    }
}

fn op_marker(meta: &jj_lib::op_store::OperationMetadata) -> Option<OpMarker> {
    let desc = meta.description.trim();
    for kind in ["undo", "redo"] {
        if let Some(rest) = desc.strip_prefix(&format!("{}:", kind)) {
            if let Some(id) = parse_op_id(rest.trim()) {
                return Some(OpMarker { kind, id, authored_by_j: true });
            }
        }
        if let Some(rest) = desc.strip_prefix(&format!("{} operation", kind)) {
            if let Some(id) = parse_op_id(rest.trim()) {
                return Some(OpMarker { kind, id, authored_by_j: false });
            }
        }
    }
    None
}

/// Whether `meta` is the operation `init` or `clone` recorded (§7.7), by its
/// description, `init` or `clone URL`: no expression is described so, both
/// words being reserved (§1.1).
fn op_created_repo(meta: &jj_lib::op_store::OperationMetadata) -> bool {
    let desc = meta.description.trim();
    desc == "init" || desc.starts_with("clone ")
}

/// a full-length (or prefix) hex operation id
fn parse_op_id(s: &str) -> Option<OperationId> {
    if s.is_empty() || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    hex::decode(pad_hex(s)).ok().map(OperationId::new)
}

fn pad_hex(s: &str) -> String {
    if s.len() % 2 == 0 {
        s.to_string()
    } else {
        format!("0{}", s)
    }
}

fn op_parent_id(
    loader: &jj_lib::repo::RepoLoader,
    op_id: &OperationId,
) -> Result<Option<OperationId>, OpenError> {
    let op = block_on(loader.load_operation(op_id))
        .map_err(|e| (2, format!("cannot load an operation: {}", e)))?;
    Ok(op.parent_ids().first().cloned())
}

/// parse the push-record list (§7.6): records of shape {id, name} set a
/// bookmark, records of shape {delete} remove one; duplicates merge, the same
/// name with different effects crashes (exit 1)
fn parse_push_records(
    value: &Value,
    vis: &VisibleRepo,
    repo: &Arc<ReadonlyRepo>,
) -> Result<Vec<(String, Option<CommitId>)>, OpenError> {
    let items = value
        .as_list()
        .map_err(|_| (1, "push: the expression must evaluate to a list of push records".to_string()))?;
    let mut effects: BTreeMap<String, Option<String>> = BTreeMap::new();
    for item in items {
        let fields = item.field_set().ok_or_else(|| {
            (1, "push: every element must be a record of shape {id, name} or {delete}".to_string())
        })?;
        if fields.len() == 1 && fields.contains("delete") {
            let name = item
                .field("delete")
                .and_then(|v| v.as_text().map(|t| t.to_string()))
                .map_err(|_| (1, "push: `delete` must be a Text".to_string()))?;
            match effects.get(&name) {
                Some(None) => {}
                Some(Some(_)) => {
                    return Err((1, format!("crash: two push records for `{}` disagree", name)))
                }
                None => {
                    effects.insert(name, None);
                }
            }
        } else if fields.len() == 2 && fields.contains("id") && fields.contains("name") {
            let name = item
                .field("name")
                .and_then(|v| v.as_text().map(|t| t.to_string()))
                .map_err(|_| (1, "push: `name` must be a Text".to_string()))?;
            let id = match item.field("id") {
                Ok(Value::Id(i)) => i.to_string(),
                _ => return Err((1, "push: `id` must be an Id".to_string())),
            };
            match effects.get(&name) {
                Some(Some(prev)) if prev == &id => {}
                Some(_) => {
                    return Err((1, format!("crash: two push records for `{}` disagree", name)))
                }
                None => {
                    effects.insert(name, Some(id));
                }
            }
        } else {
            return Err((
                1,
                "push: every element must be a record of shape {id, name} or {delete}".to_string(),
            ));
        }
    }
    let origin = RemoteName::new("origin");
    let mut out = Vec::new();
    for (name, effect) in effects {
        if !is_valid_git_branch_name(&name) {
            return Err((1, format!("push: `{}` is not a valid git branch name", name)));
        }
        match effect {
            Some(change_id) => {
                let rec = vis.commits.get(&change_id).ok_or_else(|| {
                    (1, format!("push: `@{}` is not a visible commit", change_id))
                })?;
                out.push((name, Some(rec.commit.id().clone())));
            }
            None => {
                let remote_ref = repo
                    .view()
                    .get_remote_bookmark(RefName::new(&name).to_remote_symbol(origin));
                if remote_ref.target.is_absent() {
                    return Err((
                        1,
                        format!("push: the remote does not have a bookmark `{}`", name),
                    ));
                }
                out.push((name, None));
            }
        }
    }
    Ok(out)
}

/// §7.6: refuse to move a bookmark whose current target is immutable to a
/// commit that does not descend from that target, or to delete it while no
/// bookmark left on `origin` after the push reaches that target; and refuse
/// to send commits with unresolved files or empty descriptions
fn check_push_records(
    records: &[(String, Option<CommitId>)],
    repo: &Arc<ReadonlyRepo>,
    vis: &VisibleRepo,
    immutable: &BTreeSet<String>,
) -> Result<(), OpenError> {
    let store = repo.store().clone();
    let origin = RemoteName::new("origin");
    // the targets of the bookmarks on `origin` after the push: those no
    // record names, where they are as of the last fetch or push, and the
    // records' new ones; computed for the first delete that needs them
    let mut left_on_origin: Option<Vec<CommitId>> = None;
    for (name, target) in records {
        let Some(current) = repo
            .view()
            .get_remote_bookmark(RefName::new(name).to_remote_symbol(origin))
            .target
            .as_resolved()
            .cloned()
            .unwrap_or(None)
        else {
            continue;
        };
        let change_id = block_on(store.get_commit_async(&current))
            .map_err(|e| (2, format!("cannot read a commit: {}", e)))?
            .change_id()
            .reverse_hex();
        // the immutable set holds visible commits: a target rewritten here
        // since the last fetch or push is hidden, and not in it although the
        // visible rewrite carries its change id (and its label)
        let visible = vis.commits.get(&change_id).is_some_and(|c| c.commit.id() == &current);
        if !visible || !immutable.contains(&change_id) {
            continue;
        }
        match target {
            None => {
                // a delete loses no history while another bookmark still
                // reaches the commit: a merged feature, a second name
                let heads = left_on_origin.get_or_insert_with(|| {
                    let named: BTreeSet<&str> = records.iter().map(|(n, _)| n.as_str()).collect();
                    repo.view()
                        .remote_bookmarks(origin)
                        .filter(|(n, _)| !named.contains(n.as_str()))
                        .flat_map(|(_, remote_ref)| remote_ref.target.added_ids())
                        .chain(records.iter().filter_map(|(_, t)| t.as_ref()))
                        .cloned()
                        .collect()
                });
                let unreached = ResolvedRevsetExpression::commits(vec![current.clone()])
                    .intersection(&ResolvedRevsetExpression::commits(heads.clone()).ancestors())
                    .evaluate(repo.as_ref())
                    .and_then(|r| r.is_empty())
                    .map_err(|e| (2, format!("cannot read the index: {}", e)))?;
                if unreached {
                    return Err((
                        1,
                        format!(
                            "push: `{}` is on the immutable commit `@{}` and cannot be deleted while no bookmark left on origin reaches it",
                            name, change_id
                        ),
                    ));
                }
            }
            Some(new) => {
                let descends = block_on(repo.index().is_ancestor(&current, new))
                    .map_err(|e| (2, format!("cannot read the index: {}", e)))?;
                if !descends {
                    return Err((
                        1,
                        format!(
                            "push: `{}` is on the immutable commit `@{}` and can only move to a descendant of it",
                            name, change_id
                        ),
                    ));
                }
            }
        }
    }
    // the commits that would be sent: ancestors of the new targets that no
    // bookmark on the remote already reaches; a delete sends none
    let heads: Vec<CommitId> = records.iter().filter_map(|(_, t)| t.clone()).collect();
    if heads.is_empty() {
        return Ok(());
    }
    let on_remote: Vec<CommitId> = repo
        .view()
        .remote_bookmarks(origin)
        .flat_map(|(_, remote_ref)| remote_ref.target.added_ids())
        .cloned()
        .collect();
    let sent = ResolvedRevsetExpression::commits(on_remote)
        .range(&ResolvedRevsetExpression::commits(heads))
        .evaluate(repo.as_ref())
        .map_err(|e| (2, format!("cannot list the commits to send: {}", e)))?;
    let sent: Vec<Commit> = block_on(sent.stream().commits(&store).try_collect())
        .map_err(|e| (2, format!("cannot list the commits to send: {}", e)))?;
    for commit in &sent {
        let change_id = commit.change_id().reverse_hex();
        for (_path, value) in commit.tree().entries() {
            let value = value.map_err(|e| (2, format!("cannot read a tree: {}", e)))?;
            if !value.is_resolved() {
                return Err((1, format!("push: commit `@{}` has unresolved files", change_id)));
            }
        }
        if commit.description().trim().is_empty() && commit.id() != store.root_commit_id() {
            return Err((1, format!("push: commit `@{}` has an empty description", change_id)));
        }
    }
    Ok(())
}

/// change-id closure of jj-parents from the seeds (all jj parents, not just
/// the first), for the immutable set (§7.5)
fn ancestors_of_change_ids(vis: &VisibleRepo, seeds: &[String]) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut stack: Vec<String> = seeds.to_vec();
    while let Some(id) = stack.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if let Some(rec) = vis.commits.get(&id) {
            for pid in rec.all_parents.iter() {
                if !pid.is_empty() {
                    stack.push(pid.clone());
                }
            }
        }
    }
    seen
}

/// git-check-ref-format --branch rules
fn is_valid_git_branch_name(name: &str) -> bool {
    if name.is_empty()
        || name.starts_with('-')
        || name.starts_with('.')
        || name.ends_with('.')
        || name.ends_with('/')
        || name.ends_with(".lock")
        || name.contains("..")
        || name.contains("//")
        || name.contains("@{")
        || name == "@"
    {
        return false;
    }
    name.chars().all(|c| {
        !(c.is_ascii_control()
            || matches!(c, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\'))
    })
}

// ----------------------------------------------------------------------
// Backend impl: metadata lookups and replay (§7.2, §7.3)
// ----------------------------------------------------------------------

impl Backend for JjBackend {
    fn visible_ids(&self) -> Vec<String> {
        match self.inner.visible.lock().unwrap().as_ref() {
            Some(v) => v.commits.keys().cloned().collect(),
            None => Vec::new(),
        }
    }

    fn meta(&self, id: &str) -> Result<MetaInfo, Crash> {
        let vis = self.inner.visible.lock().unwrap().clone();
        let vis = vis.ok_or_else(|| Crash::new("meta: repository not loaded"))?;
        let rec = vis
            .commits
            .get(id)
            .ok_or_else(|| Crash::new("meta: no stored commit for this id"))?;
        Ok(MetaInfo {
            hash: rec.commit.id().hex(),
            author: rec.commit.author().name.clone(),
            email: rec.commit.author().email.clone(),
            time: rec.commit.committer().timestamp.timestamp.0.div_euclid(1000),
        })
    }

    fn is_merge(&self, id: &str) -> bool {
        let vis = self.inner.visible.lock().unwrap().clone();
        vis.and_then(|v| v.commits.get(id).map(|r| r.commit.parent_ids().len() >= 2))
            .unwrap_or(false)
    }

    fn has_conflict(&self, id: &str) -> Option<bool> {
        let vis = self.inner.visible.lock().unwrap().clone();
        // O(1): a commit is conflicted iff its tree id is an unresolved merge
        vis.and_then(|v| v.commits.get(id).map(|r| r.commit.has_conflict()))
    }

    fn is_empty(&self, id: &str, parent: &str) -> Option<bool> {
        let vis = self.inner.visible.lock().unwrap().clone();
        // O(1): empty iff the tree id equals the parent's tree id
        vis.and_then(|v| {
            let rec = v.commits.get(id)?;
            let parent = v.commits.get(parent)?;
            Some(rec.commit.tree_ids() == parent.commit.tree_ids())
        })
    }

    fn folds_case(&self) -> bool {
        // jj's checkout refuses a name whose file identity is that of
        // `.git` or `.jj` in the same directory (local_working_copy.rs), so
        // where `.JJ` is the workspace's own `.jj` it refuses `.GIT` too.
        // Where each spelling of a name gets its own inode number
        // (exfat-fuse) the identities differ and the checkout writes
        // `.GIT/x` into `.git`, so folding is told by how names resolve:
        // `.JJ` resolves, but the directory does not list it under that
        // spelling. A directory that cannot be listed is taken to fold
        // case, the answer that refuses more.
        *self.inner.folds_case.get_or_init(|| {
            use jj_lib::file_util::FileIdentity;
            let root = &self.inner.workspace_root;
            let Ok(upper) = FileIdentity::from_symlink_path(root.join(".JJ")) else {
                return false;
            };
            FileIdentity::from_symlink_path(root.join(".jj")).is_ok_and(|lower| lower == upper)
                || std::fs::read_dir(root).map_or(true, |mut entries| {
                    !entries.any(|e| e.is_ok_and(|e| e.file_name() == ".JJ"))
                })
        })
    }

    fn name_fits(&self, name: &str) -> bool {
        if name.len() <= crate::repo::NAME_MAX {
            return true;
        }
        // a component, never a path: one with `/` would be created elsewhere
        if name.contains(['/', '\0']) {
            return false;
        }
        // no call tells whether the limit counts bytes (ext4) or UTF-16
        // units or characters (exFAT, NTFS, APFS), so the name is tried:
        // created in a directory of its own under `.jj`, which is then
        // removed, once per name and run
        let mut known = self.inner.long_names.lock().unwrap();
        if let Some(fits) = known.get(name) {
            return *fits;
        }
        let fits = match tempfile::Builder::new()
            .prefix("name-")
            .tempdir_in(self.inner.workspace_root.join(".jj"))
        {
            Ok(dir) => {
                let fits = std::fs::File::create(dir.path().join(name)).is_ok();
                drop(dir);
                fits
            }
            Err(_) => false,
        };
        known.insert(name.to_string(), fits);
        fits
    }

    fn ancestors_closed(&self, ids: &BTreeSet<String>) -> BTreeSet<String> {
        let vis = self.inner.visible.lock().unwrap().clone();
        let Some(vis) = vis else {
            return BTreeSet::new();
        };
        // every jj parent, not just the first: rewriting a commit reached
        // only through a merge's second parent rewrites the merge too
        // (§7.5 step 3)
        let seeds: Vec<String> = ids.iter().cloned().collect();
        ancestors_of_change_ids(&vis, &seeds)
    }

    fn replay(&self, onto: &[Value], from: &[Value], to: &[Value]) -> Result<Vec<Value>, Crash> {
        let store = self.current_repo().store().clone();
        block_on(jj_replay(&store, onto, from, to))
    }
}

// ----------------------------------------------------------------------
// jj three-way tree merge for `replay` (§7.3)
// ----------------------------------------------------------------------

async fn jj_replay(
    store: &Arc<Store>,
    onto: &[Value],
    from: &[Value],
    to: &[Value],
) -> Result<Vec<Value>, Crash> {
    let onto_tree = build_tree(store, &Value::list(onto.to_vec())).await?;
    let from_tree = build_tree(store, &Value::list(from.to_vec())).await?;
    let to_tree = build_tree(store, &Value::list(to.to_vec())).await?;
    // any of the three may itself be conflicted (a snapshot with unresolved
    // blobs, §7.3), so merge the trees the way jj's rebase does:
    // MergedTree::merge flattens conflicted inputs into one merge, simplifies
    // it, and resolves file contents line-level with tree_merge::merge_trees,
    // keeping unresolvable paths as conflicts. Empty labels leave the result
    // unlabeled; the labels would not survive tree_to_files anyway.
    let merged = MergedTree::merge(Merge::from_removes_adds(
        vec![(from_tree.clone(), String::new())],
        vec![(onto_tree.clone(), String::new()), (to_tree.clone(), String::new())],
    ))
    .await
    .map_err(|e| Crash::new(format!("replay: {}", e)))?;
    refuse_file_directory_clash(&merged, &onto_tree, &from_tree, &to_tree).await?;
    // conflicts stay as unresolved blobs (§7.3)
    tree_to_files(store, &merged, &BlobCache::default(), &EntryCache::default()).await
}

/// Crash if the merge left a conflict with a file on one side and, on
/// another, a directory `onto` or `to` holds at that path: one entry cannot
/// list that directory's entries (§7.3). A conflict that `onto`, `from` or
/// `to` already holds at the path is not made here: its sides, a directory
/// side jj made earlier included, came in as an unresolved blob, and replay
/// carries them beside the others, as when the child resolving that conflict
/// is squashed into it.
async fn refuse_file_directory_clash(
    merged: &MergedTree,
    onto: &MergedTree,
    from: &MergedTree,
    to: &MergedTree,
) -> Result<(), Crash> {
    let value_at = async |tree: &MergedTree, path: &RepoPath| {
        tree.path_value(path).await.map_err(|e| Crash::new(format!("replay: {}", e)))
    };
    // walks only the unresolved part of the tree: nothing when it is resolved
    for (path, value) in merged.conflicts() {
        let value = value.map_err(|e| Crash::new(format!("replay: {}", e)))?;
        // sides that cancel are no sides
        let value = value.simplify();
        let has = |dir: bool| {
            value
                .adds()
                .any(|v| matches!(v, Some(t) if matches!(t, TreeValue::Tree(_)) == dir))
        };
        if !has(false) || !has(true) {
            continue;
        }
        let held = [
            value_at(onto, &path).await?,
            value_at(from, &path).await?,
            value_at(to, &path).await?,
        ];
        // an input's conflict at the path, as opposed to one inside the
        // directory there, which leaves the path a directory in the input
        if held.iter().any(|h| !h.is_resolved() && !h.is_tree()) {
            continue;
        }
        let side = |dir: &MergedTreeValueT| {
            dir.is_tree() && value.adds().any(|v| v.is_some() && dir.iter().any(|h| h == v))
        };
        if side(&held[0]) || side(&held[2]) {
            let comps: Vec<String> =
                path.components().map(|c| c.as_internal_str().to_string()).collect();
            return Err(crate::domain::file_directory_clash(&comps));
        }
    }
    Ok(())
}

// ----------------------------------------------------------------------
// values -> jj trees
// ----------------------------------------------------------------------

/// A merged tree value: Merge<Option<TreeValue>> (jj_lib::backend::MergedTreeValue)
type MergedTreeValueT = Merge<Option<TreeValue>>;

struct ConflictTreeValue;

impl ConflictTreeValue {
    fn resolved(v: TreeValue) -> MergedTreeValueT {
        Merge::resolved(Some(v))
    }
    fn new(adds: Vec<Option<TreeValue>>, removes: Vec<Option<TreeValue>>) -> MergedTreeValueT {
        Merge::from_removes_adds(removes, adds)
    }
}

async fn build_tree(store: &Arc<Store>, files: &Value) -> Result<MergedTree, Crash> {
    let entries = files
        .as_list()
        .map_err(|c| Crash::new(format!("persistence: {}", c.msg)))?;
    let map = crate::domain::snapshot_map(entries)?;
    let mut builder = MergedTreeBuilder::new(store.empty_merged_tree());
    for (path, content) in &map {
        let repo_path = to_repo_path(path)?;
        let value = blob_to_tree_value(store, content).await?;
        builder.set_or_remove(repo_path, value);
    }
    builder
        .write_tree()
        .await
        .map_err(|e| Crash::new(format!("cannot write a tree: {}", e)))
}

async fn blob_to_tree_value(store: &Arc<Store>, content: &Value) -> Result<MergedTreeValueT, Crash> {
    let Value::Blob(b) = content else {
        return Err(Crash::new("persistence: content is not a Blob"));
    };
    match &b.content {
        // an unchanged blob loaded from this same store: reuse its file id
        // instead of inflating and rewriting the bytes
        BlobContent::Lazy(l) => {
            let id = FileId::try_from_hex(&l.id)
                .ok_or_else(|| Crash::new("persistence: bad blob id".to_string()))?;
            Ok(ConflictTreeValue::resolved(TreeValue::File {
                id,
                executable: b.kind == BlobKind::Executable,
                copy_id: CopyId::placeholder(),
            }))
        }
        BlobContent::Resolved(bytes) if b.kind == BlobKind::Symlink => {
            let target = std::str::from_utf8(bytes)
                .map_err(|_| Crash::new("persistence: symlink target is not UTF-8"))?;
            let id = store
                .write_symlink(&RepoPathBuf::root(), target)
                .await
                .map_err(|e| Crash::new(format!("cannot write a symlink: {}", e)))?;
            Ok(ConflictTreeValue::resolved(TreeValue::Symlink(id)))
        }
        BlobContent::Resolved(bytes) => {
            let mut slice: &[u8] = bytes.as_ref();
            let id = store
                .write_file(&RepoPathBuf::root(), &mut slice)
                .await
                .map_err(|e| Crash::new(format!("cannot write a file: {}", e)))?;
            Ok(ConflictTreeValue::resolved(TreeValue::File {
                id,
                executable: b.kind == BlobKind::Executable,
                copy_id: CopyId::placeholder(),
            }))
        }
        BlobContent::Conflict(sides) => {
            if sides.len() % 2 != 1 {
                return Err(Crash::new("persistence: malformed conflict blob"));
            }
            // jj order: [add, (remove, add)*] → Merge with interleaved adds
            // and removes; each side keeps its absence, its own file type or
            // its directory (§7.3), so the tree written is the one the
            // conflict was read from
            let mut values: Vec<Option<TreeValue>> = Vec::new();
            for side in sides {
                let Some(side) = side else {
                    values.push(None);
                    continue;
                };
                let value = if let Some(hex) = &side.tree {
                    TreeValue::Tree(
                        TreeId::try_from_hex(hex)
                            .ok_or_else(|| Crash::new("persistence: bad tree id"))?,
                    )
                } else if side.kind == BlobKind::Symlink {
                    let target = std::str::from_utf8(&side.bytes)
                        .map_err(|_| Crash::new("persistence: symlink target is not UTF-8"))?;
                    let id = store
                        .write_symlink(&RepoPathBuf::root(), target)
                        .await
                        .map_err(|e| Crash::new(format!("cannot write a symlink: {}", e)))?;
                    TreeValue::Symlink(id)
                } else {
                    let mut slice: &[u8] = side.bytes.as_ref();
                    let id = store
                        .write_file(&RepoPathBuf::root(), &mut slice)
                        .await
                        .map_err(|e| Crash::new(format!("cannot write a file: {}", e)))?;
                    TreeValue::File {
                        id,
                        executable: side.kind == BlobKind::Executable,
                        copy_id: CopyId::placeholder(),
                    }
                };
                values.push(Some(value));
            }
            let mut adds = Vec::new();
            let mut removes = Vec::new();
            for (i, v) in values.into_iter().enumerate() {
                if i % 2 == 0 {
                    adds.push(v);
                } else {
                    removes.push(v);
                }
            }
            Ok(ConflictTreeValue::new(adds, removes))
        }
    }
}

fn to_repo_path(path: &[String]) -> Result<RepoPathBuf, Crash> {
    let mut p = RepoPathBuf::root();
    for comp in path {
        let c = RepoPathComponentBuf::new(comp.clone())
            .map_err(|_| Crash::new(format!("invalid path component `{}`", comp)))?;
        p = p.join(&c);
    }
    Ok(p)
}

// ----------------------------------------------------------------------
// jj trees -> values
// ----------------------------------------------------------------------

/// blob contents keyed by file id, shared across the whole repo load: most
/// files are unchanged between commits, so inflating each blob once (instead
/// of once per commit that references it) keeps loading linear in the
/// repository's unique data rather than commits × files
#[derive(Clone, Default)]
struct BlobCache(Rc<RefCell<HashMap<FileId, Rc<Vec<u8>>>>>);

impl BlobCache {
    fn get(&self, id: &FileId) -> Option<Rc<Vec<u8>>> {
        self.0.borrow().get(id).cloned()
    }
    fn insert(&self, id: FileId, bytes: Rc<Vec<u8>>) {
        self.0.borrow_mut().insert(id, bytes);
    }
}

/// whole file *entries* ({path, content}) keyed by (path, blob identity),
/// shared across the whole repo load: an unchanged file is the same Value in
/// every commit, so it is built once and cloned (an Rc bump) thereafter
#[derive(Clone, Default)]
struct EntryCache(Rc<RefCell<HashMap<String, Value>>>);

impl EntryCache {
    fn get(&self, key: &str) -> Option<Value> {
        self.0.borrow().get(key).cloned()
    }
    fn insert(&self, key: String, entry: Value) {
        self.0.borrow_mut().insert(key, entry);
    }
}

async fn read_file_bytes(
    store: &Arc<Store>,
    path: &RepoPath,
    id: &FileId,
    cache: &BlobCache,
) -> Result<Rc<Vec<u8>>, Crash> {
    if let Some(bytes) = cache.get(id) {
        return Ok(bytes);
    }
    use futures::AsyncReadExt;
    let mut reader = store
        .read_file(path, id)
        .await
        .map_err(|e| Crash::new(format!("cannot read a file: {}", e)))?;
    let mut buf = Vec::new();
    reader
        .read_to_end(&mut buf)
        .await
        .map_err(|e| Crash::new(format!("cannot read a file: {}", e)))?;
    let buf = Rc::new(buf);
    cache.insert(id.clone(), buf.clone());
    Ok(buf)
}

/// one side of a conflict as jj stores it: absent (`None`, a deletion
/// conflict), or content with that side's own file type (§7.3)
async fn conflict_side(
    store: &Arc<Store>,
    path: &RepoPath,
    v: &Option<TreeValue>,
    cache: &BlobCache,
) -> Result<Option<ConflictSide>, Crash> {
    match v {
        None => Ok(None),
        Some(TreeValue::File { id, executable, .. }) => Ok(Some(ConflictSide {
            kind: if *executable {
                BlobKind::Executable
            } else {
                BlobKind::Regular
            },
            bytes: read_file_bytes(store, path, id, cache).await?,
            tree: None,
        })),
        Some(TreeValue::Symlink(id)) => {
            let target = store
                .read_symlink(path, id)
                .await
                .map_err(|e| Crash::new(format!("cannot read a symlink: {}", e)))?;
            Ok(Some(ConflictSide {
                kind: BlobKind::Symlink,
                bytes: Rc::new(target.into_bytes()),
                tree: None,
            }))
        }
        // a directory side, in a conflict between a file and a directory,
        // reads as empty content but keeps its tree, so writing the conflict
        // back cannot put an empty file where the directory was
        Some(TreeValue::Tree(id)) => Ok(Some(ConflictSide {
            kind: BlobKind::Regular,
            bytes: Rc::new(Vec::new()),
            tree: Some(id.hex()),
        })),
        // a submodule side reads as an empty file, as a resolved one does in
        // tree_value_to_blob (§12)
        Some(TreeValue::GitSubmodule(_)) => Ok(Some(ConflictSide::regular(&[]))),
    }
}

async fn merged_value_to_blob(
    store: &Arc<Store>,
    path: &RepoPath,
    value: &jj_lib::backend::MergedTreeValue,
    cache: &BlobCache,
) -> Result<Value, Crash> {
    if let Some(v) = value.as_resolved() {
        return match v {
            Some(tv) => tree_value_to_blob(store, path, tv).await,
            None => Ok(Value::Blob(Rc::new(BlobVal {
                kind: BlobKind::Regular,
                content: BlobContent::Resolved(Rc::new(Vec::new())),
            }))),
        };
    }
    // conflict sides, jj order: [add, (remove, add)*] (§7.3)
    let mut sides: Vec<Option<ConflictSide>> = Vec::new();
    for (i, add) in value.adds().enumerate() {
        sides.push(conflict_side(store, path, add, cache).await?);
        if let Some(remove) = value.get_remove(i) {
            sides.push(conflict_side(store, path, remove, cache).await?);
        }
    }
    Ok(Value::Blob(Rc::new(BlobVal {
        kind: BlobKind::Regular,
        content: BlobContent::Conflict(sides),
    })))
}

async fn tree_value_to_blob(
    store: &Arc<Store>,
    path: &RepoPath,
    value: &TreeValue,
) -> Result<Value, Crash> {
    match value {
        TreeValue::File { id, executable, .. } => {
            // lazy: the bytes are read from the store only if an expression
            // actually looks at them; equality and persistence use `id`
            let id_hex = id.hex();
            let store2 = store.clone();
            let path2 = path.to_owned();
            let id2 = id.clone();
            let lazy = LazyBlob::new(id_hex, move || {
                let mut reader = block_on(store2.read_file(&path2, &id2))
                    .map_err(|e| Crash::new(format!("cannot read a file: {}", e)))?;
                let mut buf = Vec::new();
                {
                    use futures::AsyncReadExt;
                    block_on(reader.read_to_end(&mut buf))
                        .map_err(|e| Crash::new(format!("cannot read a file: {}", e)))?;
                }
                Ok(Rc::new(buf))
            });
            Ok(Value::Blob(Rc::new(BlobVal {
                kind: if *executable {
                    BlobKind::Executable
                } else {
                    BlobKind::Regular
                },
                content: BlobContent::Lazy(Rc::new(lazy)),
            })))
        }
        TreeValue::Symlink(id) => {
            let target = store
                .read_symlink(path, id)
                .await
                .map_err(|e| Crash::new(format!("cannot read a symlink: {}", e)))?;
            Ok(Value::Blob(Rc::new(BlobVal {
                kind: BlobKind::Symlink,
                content: BlobContent::Resolved(Rc::new(target.into_bytes())),
            })))
        }
        // MergedTree::entries recurses into a resolved directory, and a
        // directory side of a conflict goes through conflict_side, so this is
        // a submodule, which §12 puts out of scope
        TreeValue::Tree(_) | TreeValue::GitSubmodule(_) => Ok(Value::Blob(Rc::new(BlobVal {
            kind: BlobKind::Regular,
            content: BlobContent::Resolved(Rc::new(Vec::new())),
        }))),
    }
}

async fn tree_to_files(
    store: &Arc<Store>,
    tree: &MergedTree,
    cache: &BlobCache,
    entries: &EntryCache,
) -> Result<Vec<Value>, Crash> {
    let mut out = Vec::new();
    for (path, value) in tree.entries() {
        let value = value.map_err(|e| Crash::new(format!("cannot read a tree entry: {}", e)))?;
        // cache key: the entry is fully determined by its path and the
        // identity of its (resolved) content; conflicts are not cached
        let key = match value.as_resolved() {
            Some(Some(TreeValue::File { id, executable, .. })) => Some(format!(
                "F\u{0}{}\u{0}{}\u{0}{}",
                path.as_internal_file_string(),
                id.hex(),
                executable
            )),
            // symlinks and directories are cheap; conflicts are rare — neither cached
            _ => None,
        };
        let entry = match key.as_ref().and_then(|k| entries.get(k)) {
            Some(e) => e,
            None => {
                let comps: Vec<String> = path
                    .components()
                    .map(|c| c.as_internal_str().to_string())
                    .collect();
                let content = merged_value_to_blob(store, &path, &value, cache).await?;
                let p = Value::list(comps.into_iter().map(Value::text).collect());
                let e = Value::record(&[("content", content), ("path", p)]);
                if let Some(k) = key {
                    entries.insert(k, e.clone());
                }
                e
            }
        };
        out.push(entry);
    }
    Ok(out)
}

// ----------------------------------------------------------------------
// value helpers
// ----------------------------------------------------------------------

fn commit_id_of(commit: &Value) -> Result<String, Crash> {
    match commit.field("id")? {
        Value::Id(i) => Ok(i.to_string()),
        v => Err(Crash::new(format!("expected an Id, got a {}", v.kind_name()))),
    }
}

fn hex_of(id: Option<&CommitId>) -> String {
    id.map(|i| i.hex()).unwrap_or_default()
}

fn truncate_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn new_id_set(repo: &Value) -> Result<BTreeSet<String>, Crash> {
    let mut out = BTreeSet::new();
    for c in crate::repo::all_commits(repo)? {
        out.insert(commit_id_of(&c)?);
    }
    Ok(out)
}

// ----------------------------------------------------------------------
// building the Repo value from the visible map (§7.2)
// ----------------------------------------------------------------------

fn build_repo_value(vis: &VisibleRepo, focus_change: &str) -> Result<Value, OpenError> {
    let root_rec = vis
        .commits
        .get(ROOT_ID)
        .ok_or_else(|| (2, "no root commit".to_string()))?;
    let store = root_rec.commit.store().clone();
    let cache = BlobCache::default();
    let entries = EntryCache::default();
    let mut subtree = build_subtree(vis, root_rec, &store, &cache, &entries)?;
    sort_subtree(&mut subtree, vis);
    let top = Value::record(&[
        ("children", subtree.field("children").map_err(|e| (2, e.msg))?),
        ("context", Value::list(vec![])),
        ("root", subtree.field("root").map_err(|e| (2, e.msg))?),
    ]);
    match crate::repo::by_id(&top, focus_change).map_err(|c| (2, c.msg))? {
        Some(v) => Ok(v),
        None => Err((2, "the working-copy commit is not visible".to_string())),
    }
}

fn build_subtree(
    vis: &VisibleRepo,
    rec: &StoredCommit,
    store: &Arc<Store>,
    cache: &BlobCache,
    entries: &EntryCache,
) -> Result<Value, OpenError> {
    // the files list is lazy (§7.2): most commits are never inspected, so
    // their file entries are materialized only on first `field("files")`.
    // Tagged with the change id, it tells the renderer when the O(1)
    // `has_conflict`/`is_empty` answers still describe the commit's files.
    let files_thunk = {
        let store = store.clone();
        let tree = rec.commit.tree();
        let cache = cache.clone();
        let entries = entries.clone();
        Value::Thunk(Rc::new(crate::value::ThunkVal::stored(rec.change_id.clone(), move || {
            let files = block_on(tree_to_files(&store, &tree, &cache, &entries))?;
            Ok(Value::list(files))
        })))
    };
    let labels: Vec<Value> = vis
        .labels
        .get(&rec.change_id)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(Value::text)
        .collect();
    let commit_v = Value::record(&[
        ("files", files_thunk),
        ("id", Value::Id(Rc::new(rec.change_id.clone()))),
        ("labels", Value::list(labels)),
        ("message", Value::text(rec.commit.description())),
    ]);
    let mut children = Vec::new();
    if let Some(kids) = vis.children.get(&rec.change_id) {
        for kid in kids {
            if let Some(krec) = vis.commits.get(kid) {
                children.push(build_subtree(vis, krec, store, cache, entries)?);
            }
        }
    }
    Ok(Value::record(&[("children", Value::list(children)), ("root", commit_v)]))
}

/// children ordered by committer timestamp asc, ties by change id asc (§7.2)
fn sort_subtree(subtree: &mut Value, vis: &VisibleRepo) {
    let mut kids: Vec<Value> = subtree
        .field("children")
        .and_then(|v| v.as_list().map(|l| l.to_vec()))
        .unwrap_or_default();
    for kid in kids.iter_mut() {
        sort_subtree(kid, vis);
    }
    kids.sort_by(|a, b| {
        let key = |v: &Value| -> (i64, String) {
            let id = v
                .field("root")
                .and_then(|r| r.field("id"))
                .ok()
                .and_then(|i| match i {
                    Value::Id(s) => Some(s.to_string()),
                    _ => None,
                })
                .unwrap_or_default();
            let t = vis.commits.get(&id).map(|r| r.committer_millis).unwrap_or(0);
            (t, id)
        };
        key(a).cmp(&key(b))
    });
    let root = subtree.field("root").expect("subtree has a root");
    *subtree = Value::record(&[("children", Value::list(kids)), ("root", root)]);
}
