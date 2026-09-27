//! Repo-zipper operations, the reference `by` walk (§10), and the
//! persistence validation shared by `validate` and the jj backend (§7.5).

use crate::domain::{Backend, ROOT_ID};
use crate::eval::Interp;
use crate::value::{value_eq, Crash, Value};
use std::collections::{BTreeMap, BTreeSet};

/// navigate: refocus `repo` on the commit with change id `id` (§10 reference).
///
/// Finding the commit and refocusing onto it are separate walks: refocusing
/// builds a new location — a record, with the siblings split around a hole —
/// so doing it for every node visited on the way cost an allocation per
/// commit. Only the commits on the path to the target need one, and there are
/// as many of those as the history is deep.
pub fn by_id(repo: &Value, id: &str) -> Result<Option<Value>, Crash> {
    let top = top_of(repo)?;
    if id_is(&top.field("root")?, id)? {
        return Ok(Some(top));
    }
    let mut path = Vec::new();
    let children = top.field("children")?;
    if !find_path(children.as_list()?, id, &mut path)? {
        return Ok(None);
    }
    // Descend the path collecting one frame per level and assemble the
    // location once. Refocusing step by step would rebuild the context at
    // every level, which is quadratic in the depth on its own.
    let mut root = top.field("root")?;
    let mut children = top.field("children")?;
    let mut frames: Vec<Value> = Vec::with_capacity(path.len());
    for idx in path {
        let kids = children.as_list()?;
        let child = kids[idx].clone();
        frames.push(Value::record(&[
            ("left", Value::list(kids[..idx].to_vec())),
            ("parent", root),
            ("right", Value::list(kids[idx + 1..].to_vec())),
        ]));
        root = child.field("root")?;
        children = child.field("children")?;
    }
    // innermost frame first: `up` reads context.first() as the parent
    frames.reverse();
    // `top_of` leaves no context above, but do not depend on that here
    frames.extend_from_slice(top.field("context")?.as_list()?);
    Ok(Some(Value::record(&[
        ("children", children),
        ("context", Value::list(frames)),
        ("root", root),
    ])))
}

/// indices of the children to descend through to reach `id`, if it is there
fn find_path(children: &[Value], id: &str, path: &mut Vec<usize>) -> Result<bool, Crash> {
    for (i, c) in children.iter().enumerate() {
        path.push(i);
        if id_is(&c.field("root")?, id)? {
            return Ok(true);
        }
        let grandchildren = c.field("children")?;
        if find_path(grandchildren.as_list()?, id, path)? {
            return Ok(true);
        }
        path.pop();
    }
    Ok(false)
}

/// compare a commit's id without copying it out
fn id_is(commit: &Value, id: &str) -> Result<bool, Crash> {
    match commit.field("id")? {
        Value::Id(i) => Ok(i.as_str() == id),
        v => Err(Crash::new(format!("expected an Id, got a {}", v.kind_name()))),
    }
}

fn id_of(commit: &Value) -> Result<String, Crash> {
    match commit.field("id")? {
        Value::Id(i) => Ok(i.to_string()),
        v => Err(Crash::new(format!("expected an Id, got a {}", v.kind_name()))),
    }
}

thread_local! {
    /// The last location climbed to the top, and the value it was climbed
    /// from. A revset walk applies `by` to the same repo once per id, and
    /// each climb rebuilds a location per level of depth; the answer only
    /// depends on the value, which cannot change. Holding the `Rc` keeps the
    /// address from being reused while it is the key.
    static TOP_MEMO: std::cell::RefCell<Option<(std::rc::Rc<crate::value::RecordMap>, Value)>> =
        const { std::cell::RefCell::new(None) };
}

/// move a location to the top of history
fn top_of(repo: &Value) -> Result<Value, Crash> {
    let key = match repo {
        Value::Record(m) => Some(m.clone()),
        _ => None,
    };
    if let Some(k) = &key {
        let hit = TOP_MEMO.with(|c| {
            c.borrow().as_ref().and_then(|(cached, top)| {
                std::rc::Rc::ptr_eq(cached, k).then(|| top.clone())
            })
        });
        if let Some(top) = hit {
            return Ok(top);
        }
    }
    let top = top_of_uncached(repo)?;
    if let Some(k) = key {
        TOP_MEMO.with(|c| *c.borrow_mut() = Some((k, top.clone())));
    }
    Ok(top)
}

fn top_of_uncached(repo: &Value) -> Result<Value, Crash> {
    let mut cur = repo.clone();
    loop {
        // ask whether the context is empty without copying it: this runs once
        // per level, and copying made climbing quadratic in the depth
        let ctx = cur.field("context")?;
        if ctx.as_list()?.is_empty() {
            return Ok(cur);
        }
        cur = up_of(&cur)?;
    }
}

fn up_of(repo: &Value) -> Result<Value, Crash> {
    let ctx = repo.field("context")?;
    let ctx = ctx.as_list()?;
    let frame = ctx
        .first()
        .ok_or_else(|| Crash::new("up: at the top of history"))?;
    let parent = frame.field("parent")?;
    let left = frame.field("left")?;
    let left = left.as_list()?;
    let right = frame.field("right")?;
    let right = right.as_list()?;
    let mut children: Vec<Value> = left.to_vec();
    children.push(Value::record(&[
        ("root", repo.field("root")?),
        ("children", repo.field("children")?),
    ]));
    children.extend_from_slice(right);
    // the remaining context shares the frames rather than copying them
    let rest = match repo.field("context")? {
        Value::List(xs) => Value::List(xs.skip(1)),
        v => return Err(Crash::new(format!("expected a list, got a {}", v.kind_name()))),
    };
    Ok(Value::record(&[
        ("children", Value::list(children)),
        ("context", rest),
        ("root", parent),
    ]))
}

fn refocus_child(repo: &Value, child: &Value) -> Result<Value, Crash> {
    let children = repo.field("children")?;
    let children = children.as_list()?;
    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut found = false;
    // match by the child's root id: ids are unique per repo (§7.5 validates
    // this at persist time), and a deep value_eq here would compare the whole
    // subtree — files included — making by_id quadratic in history size
    let want_id = id_of(&child.field("root")?).ok();
    for c in children.iter() {
        let same = match (&want_id, id_of(&c.field("root")?).ok()) {
            (Some(w), Some(cid)) => *w == cid,
            _ => value_eq(c, child)?,
        };
        if !found && same {
            found = true;
        } else if !found {
            left.push(c.clone());
        } else {
            right.push(c.clone());
        }
    }
    let frame = Value::record(&[
        ("left", Value::list(left)),
        ("parent", repo.field("root")?),
        ("right", Value::list(right)),
    ]);
    let mut ctx = repo.field("context")?.as_list()?.to_vec();
    ctx.insert(0, frame);
    Ok(Value::record(&[
        ("children", child.field("children")?),
        ("context", Value::list(ctx)),
        ("root", child.field("root")?),
    ]))
}

// ----------------------------------------------------------------------
// tree traversal helpers over Repo values
// ----------------------------------------------------------------------

/// all commits in the whole history (preorder from the top)
pub fn all_commits(repo: &Value) -> Result<Vec<Value>, Crash> {
    let top = top_of(repo)?;
    let mut out = Vec::new();
    let children = top.field("children")?;
    collect_commits(&top.field("root")?, children.as_list()?, &mut out)?;
    Ok(out)
}

fn collect_commits(root: &Value, children: &[Value], out: &mut Vec<Value>) -> Result<(), Crash> {
    out.push(root.clone());
    for c in children {
        let gc = c.field("children")?;
        collect_commits(&c.field("root")?, gc.as_list()?, out)?;
    }
    Ok(())
}

/// parent change id of every commit (first-parent tree), as (child_id, parent_id or "")
pub fn parent_map(repo: &Value) -> Result<Vec<(String, String)>, Crash> {
    let top = top_of(repo)?;
    let mut out = Vec::new();
    parent_map_rec(&top, &mut out)?;
    Ok(out)
}

fn parent_map_rec(loc: &Value, out: &mut Vec<(String, String)>) -> Result<(), Crash> {
    let pid = id_of(&loc.field("root")?)?;
    let children = loc.field("children")?;
    for c in children.as_list()?.iter() {
        out.push((id_of(&c.field("root")?)?, pid.clone()));
        parent_map_rec(&refocus_child(loc, c)?, out)?;
    }
    Ok(())
}

// ----------------------------------------------------------------------
// equality up to the order of snapshot entries (§7.3)
// ----------------------------------------------------------------------

/// Whether two snapshots hold the same entries, in whatever order. A
/// snapshot stands for a tree, which has no order (§7.3): `select` lists the
/// entries a fileset matches first, so a `contract m` that moves nothing
/// still reorders the parent's files, and comparing them as lists called
/// that a change. Entries compare with `value_eq`, so lazy blobs still
/// compare by content id without being read.
pub fn snapshot_eq(a: &Value, b: &Value) -> Result<bool, Crash> {
    let (a, b) = (a.forced()?, b.forced()?);
    // not lists: no snapshots, left to validation to name (§7.5 step 1)
    let (Value::List(xs), Value::List(ys)) = (&a, &b) else {
        return value_eq(&a, &b);
    };
    if xs.len() != ys.len() {
        return Ok(false);
    }
    // the usual case, both in path order: one pass, nothing allocated
    let mut k = 0;
    while k < xs.len() && value_eq(&xs[k], &ys[k])? {
        k += 1;
    }
    if k == xs.len() {
        return Ok(true);
    }
    // a tree has one listing in path order, the one it is loaded in and
    // `replay` returns, so two such listings that differ are different trees
    if in_path_order(xs) && in_path_order(ys) {
        return Ok(false);
    }
    // the rest by path; an entry with no readable path, or a path listed
    // twice, is not a tree, and compares unequal as it would as a list
    let mut rest: BTreeMap<Vec<String>, &Value> = BTreeMap::new();
    for x in &xs[k..] {
        let Some(p) = entry_path(x) else {
            return Ok(false);
        };
        if rest.insert(p, x).is_some() {
            return Ok(false);
        }
    }
    for y in &ys[k..] {
        match entry_path(y).and_then(|p| rest.remove(&p)) {
            Some(x) if value_eq(x, y)? => {}
            _ => return Ok(false),
        }
    }
    Ok(true)
}

fn entry_path(entry: &Value) -> Option<Vec<String>> {
    let path = entry.field("path").ok()?;
    let comps = path.as_list().ok()?;
    comps.iter().map(|c| c.as_text().ok().map(str::to_string)).collect()
}

/// Every entry's path strictly after the one before, component by component
/// (`a/b` < `a.txt`); false if a path is unreadable.
fn in_path_order(entries: &[Value]) -> bool {
    let before = |a: &Value, b: &Value| -> Option<bool> {
        let (pa, pb) = (a.field("path").ok()?, b.field("path").ok()?);
        let (ca, cb) = (pa.as_list().ok()?, pb.as_list().ok()?);
        for (x, y) in ca.iter().zip(cb.iter()) {
            match x.as_text().ok()?.cmp(y.as_text().ok()?) {
                std::cmp::Ordering::Equal => {}
                o => return Some(o.is_lt()),
            }
        }
        Some(ca.len() < cb.len())
    };
    entries.windows(2).all(|w| before(&w[0], &w[1]) == Some(true))
}

/// Whether `b` is the repository `a` (§1.2 step 8): equal as values, except
/// that each commit's `files` compare as snapshots (`snapshot_eq`), so a
/// result that only reorders entries persists nothing. A record `b` shares
/// with `a` is equal without being walked.
pub fn same_repo(a: &Value, b: &Value) -> Result<bool, Crash> {
    match (a, b) {
        (Value::Record(x), Value::Record(y)) => {
            if std::rc::Rc::ptr_eq(x, y) {
                return Ok(true);
            }
            if x.len() != y.len() {
                return Ok(false);
            }
            for ((k1, v1), (k2, v2)) in x.iter().zip(y.iter()) {
                // only a Commit has a `files` field in a Repo value
                let same = k1 == k2
                    && if k1 == "files" {
                        snapshot_eq(v1, v2)?
                    } else {
                        same_repo(v1, v2)?
                    };
                if !same {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        (Value::List(xs), Value::List(ys)) => {
            if xs.len() != ys.len() {
                return Ok(false);
            }
            for (u, v) in xs.iter().zip(ys.iter()) {
                if !same_repo(u, v)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        _ => value_eq(a, b),
    }
}

// ----------------------------------------------------------------------
// validation (§7.5 steps 1–3 and 6), shared by `validate` and persistence
// ----------------------------------------------------------------------

#[derive(Debug)]
pub struct Validated {
    pub immutable: BTreeSet<String>,
}

pub fn validate_repo(i: &mut Interp, new: &Value) -> Result<Validated, Crash> {
    // shape: new must be a Repo with a Commit root (§4.4)
    check_repo_shape(new)?;
    let mut seen = BTreeSet::new();
    validate_ids_and_snapshots(new, &mut seen)?;
    // immutability needs `old`; the builtin validate uses the repo loaded at
    // startup, carried in the interpreter by the CLI
    let old = i.old_repo.borrow().clone();
    let immutable = match &old {
        Some(old) => {
            validate_labels(old, new)?;
            compute_immutable(i, old)?
        }
        None => BTreeSet::new(),
    };
    if let Some(old) = &old {
        validate_immutable(old, new, &immutable)?;
    }
    let given = i.given_repo.borrow().clone().or_else(|| old.clone());
    validate_path_names(old.as_ref(), given.as_ref(), new, &immutable, i.backend.as_ref())?;
    // focus mutable (§7.5 step 6)
    let focus_id = id_of(&new.field("root")?)?;
    if focus_id == ROOT_ID || immutable.contains(&focus_id) {
        return Err(Crash::new(
            "the focus must be a mutable commit; compose with new",
        ));
    }
    Ok(Validated { immutable })
}

fn check_repo_shape(v: &Value) -> Result<(), Crash> {
    let fields = v
        .field_set()
        .ok_or_else(|| Crash::new("persistence: the value is not a Repo"))?;
    let want: BTreeSet<String> = ["children", "context", "root"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if fields != want {
        return Err(Crash::new("persistence: the value is not a Repo"));
    }
    let root_fields = v
        .field("root")?
        .field_set()
        .ok_or_else(|| Crash::new("persistence: the root is not a Commit"))?;
    let want_root: BTreeSet<String> = ["files", "id", "labels", "message"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if root_fields != want_root {
        return Err(Crash::new("persistence: the root is not a Commit"));
    }
    Ok(())
}

fn validate_ids_and_snapshots(repo: &Value, seen: &mut BTreeSet<String>) -> Result<(), Crash> {
    let top = top_of(repo)?;
    validate_tree(&top, seen)
}

fn validate_tree(loc: &Value, seen: &mut BTreeSet<String>) -> Result<(), Crash> {
    let root = loc.field("root")?;
    let id = id_of(&root)?;
    if !seen.insert(id.clone()) {
        return Err(Crash::new(format!(
            "persistence: id {} occurs more than once",
            id
        )));
    }
    let files = root.field("files")?;
    let files = files.as_list()?;
    let mut paths = BTreeSet::new();
    for e in files {
        let p = e.field("path")?;
        let p = p.as_list()?;
        let key: Vec<String> = p
            .iter()
            .map(|c| c.as_text().map(|s| s.to_string()))
            .collect::<Result<_, _>>()?;
        if key.is_empty() {
            return Err(Crash::new("persistence: a snapshot has an entry at the root path ./"));
        }
        if !paths.insert(key) {
            return Err(Crash::new("persistence: a snapshot has duplicate paths"));
        }
    }
    if let Some(p) = crate::domain::file_and_directory(&paths) {
        return Err(Crash::new(format!(
            "persistence: a snapshot has `{}` as both a file and a directory",
            p.join("/")
        )));
    }
    let children = loc.field("children")?;
    for c in children.as_list()?.iter() {
        validate_tree(&refocus_child(loc, c)?, seen)?;
    }
    Ok(())
}

/// The name limit of ext4, xfs, btrfs and tmpfs (NAME_MAX), in bytes.
/// exFAT, NTFS, HFS+ and APFS count 255 UTF-16 units or characters instead,
/// so they hold longer names, and pathconf answers 255 on each: whether a
/// longer name fits is asked of the backend (`Backend::name_fits`). A
/// filesystem with a lower limit (encfs, eCryptfs) refuses a shorter name
/// at checkout, which then records nothing (§7.5 step 7).
pub const NAME_MAX: usize = 255;

/// Whether a checkout cannot create the path component `comp` (§7.5 step
/// 1). jj stores any name that is not empty and has no `/`, but its
/// checkout refuses `.`, `..`, `.git` and `.jj`, and the filesystem a name
/// it cannot hold, part way through writing the focus; a git tree cannot
/// hold a NUL. Where the filesystem folds case, `.GIT` is the file `.git`,
/// which the checkout refuses by its file identity or, where that does not
/// show it, writes into. `fits` says whether a name longer than `NAME_MAX`
/// bytes is one the filesystem holds.
pub fn checkout_refuses(comp: &str, folds_case: bool, fits: impl FnOnce(&str) -> bool) -> bool {
    matches!(comp, "" | "." | ".." | ".git" | ".jj")
        || comp.contains(['/', '\0'])
        || (folds_case && (comp.eq_ignore_ascii_case(".git") || comp.eq_ignore_ascii_case(".jj")))
        || (comp.len() > NAME_MAX && !fits(comp))
}

/// Path components (§7.5 step 1) of every commit persisting writes anew or
/// checks out must be ones a checkout can create (`checkout_refuses`).
///
/// A commit that keeps the files it has in `old` is checked only as the
/// focus: persisting at most rewrites it with the tree jj already stored and
/// never checks it out, and a fetched branch can hold `.jj`, so refusing its
/// names would stop every persist for a commit the script did not touch.
/// Immutable commits are such commits (step 3).
///
/// A name longer than `NAME_MAX` is one the filesystem holds if the
/// working directory holds it, as the focus of `given` (its snapshot, §7.4)
/// shows: the checkout, which diffs from what is on disk, does not create
/// such a file again. Any other is asked of the backend.
fn validate_path_names(
    old: Option<&Value>,
    given: Option<&Value>,
    new: &Value,
    immutable: &BTreeSet<String>,
    backend: &dyn Backend,
) -> Result<(), Crash> {
    let folds_case = backend.folds_case();
    // the long names the working directory holds, read on first need: the
    // snapshot's file list is the whole tree
    let mut held: Option<BTreeSet<String>> = None;
    let focus = id_of(&new.field("root")?)?;
    let stored: BTreeMap<String, Value> = match old {
        Some(old) => all_commits(old)?
            .into_iter()
            .map(|c| id_of(&c).map(|id| (id, c)))
            .collect::<Result<_, _>>()?,
        None => BTreeMap::new(),
    };
    for c in all_commits(new)? {
        let id = id_of(&c)?;
        if immutable.contains(&id) {
            continue;
        }
        if id != focus {
            if let Some(o) = stored.get(&id) {
                if same_files(o, &c)? {
                    continue;
                }
            }
        }
        let files = c.field("files")?;
        for e in files.as_list()? {
            let path = e.field("path")?;
            for comp in path.as_list()? {
                let comp = comp.as_text()?;
                if comp.len() > NAME_MAX && held.is_none() {
                    held = Some(match given {
                        Some(given) => long_names(&given.field("root")?.field("files")?)?,
                        None => BTreeSet::new(),
                    });
                }
                let on_disk = |c: &str| held.as_ref().is_some_and(|h| h.contains(c));
                if checkout_refuses(comp, folds_case, |c| on_disk(c) || backend.name_fits(c)) {
                    return Err(Crash::new(format!(
                        "persistence: a snapshot has the path component {:?}, which a checkout cannot create",
                        comp
                    )));
                }
            }
        }
    }
    Ok(())
}

/// The path components of snapshot `files` longer than `NAME_MAX` bytes.
fn long_names(files: &Value) -> Result<BTreeSet<String>, Crash> {
    let mut out = BTreeSet::new();
    for e in files.as_list()? {
        for comp in e.field("path")?.as_list()? {
            let comp = comp.as_text()?;
            if comp.len() > NAME_MAX {
                out.insert(comp.to_string());
            }
        }
    }
    Ok(out)
}

/// Whether commit `new` holds the files commit `old` does. A commit record,
/// or a lazy `files`, the two share is the same without the tree being read.
fn same_files(old: &Value, new: &Value) -> Result<bool, Crash> {
    if let (Value::Record(a), Value::Record(b)) = (old, new) {
        if std::rc::Rc::ptr_eq(a, b) {
            return Ok(true);
        }
        if let (Some(Value::Thunk(x)), Some(Value::Thunk(y))) = (a.get("files"), b.get("files")) {
            if std::rc::Rc::ptr_eq(x, y) {
                return Ok(true);
            }
        }
    }
    snapshot_eq(&old.field("files")?, &new.field("files")?)
}

fn label_set(repo: &Value) -> Result<BTreeSet<(String, String)>, Crash> {
    let mut out = BTreeSet::new();
    for c in all_commits(repo)? {
        let id = id_of(&c)?;
        for l in c.field("labels").and_then(|v| v.as_list().map(|x| x.to_vec()))?.iter() {
            out.insert((id.clone(), l.as_text()?.to_string()));
        }
    }
    Ok(out)
}

fn validate_labels(old: &Value, new: &Value) -> Result<(), Crash> {
    let a = label_set(old)?;
    let b = label_set(new)?;
    if a != b {
        return Err(Crash::new(
            "persistence: labels are read-only; the set of (id, label) pairs changed",
        ));
    }
    Ok(())
}

/// The immutable set: the config's `immutable` revset against `old`, plus
/// every merge commit and its ancestors (§7.5 step 3).
pub fn compute_immutable(i: &mut Interp, old: &Value) -> Result<BTreeSet<String>, Crash> {
    let v = i.apply_cached_revset("immutable", old)?;
    let mut set = BTreeSet::new();
    for idv in v.as_list()?.iter() {
        match idv {
            Value::Id(id) => {
                set.insert(id.to_string());
            }
            _ => return Err(Crash::new("immutable: revset returned a non-Id")),
        }
    }
    // merge commits and their ancestors
    let mut merges = BTreeSet::new();
    for c in all_commits(old)? {
        let id = id_of(&c)?;
        if i.backend.is_merge(&id) {
            merges.insert(id);
        }
    }
    let closed = i.backend.ancestors_closed(&merges);
    set.extend(closed);
    Ok(set)
}

fn validate_immutable(old: &Value, new: &Value, immutable: &BTreeSet<String>) -> Result<(), Crash> {
    if immutable.is_empty() {
        return Ok(());
    }
    let old_parents: BTreeMap<String, String> = parent_map(old)?.into_iter().collect();
    let new_parents: BTreeMap<String, String> = parent_map(new)?.into_iter().collect();
    let new_commits: BTreeMap<String, Value> = all_commits(new)?
        .into_iter()
        .map(|c| id_of(&c).map(|id| (id, c)))
        .collect::<Result<_, _>>()?;
    for c in all_commits(old)? {
        let id = id_of(&c)?;
        if !immutable.contains(&id) {
            continue;
        }
        let nc = new_commits.get(&id).ok_or_else(|| {
            Crash::new(format!("persistence: commit {} is immutable", id))
        })?;
        let same_parent = old_parents.get(&id).cloned().unwrap_or_default()
            == new_parents.get(&id).cloned().unwrap_or_default();
        if !same_parent {
            return Err(Crash::new(format!(
                "persistence: commit {} is immutable (parent changed)",
                id
            )));
        }
        if !snapshot_eq(&c.field("files")?, &nc.field("files")?)? {
            return Err(Crash::new(format!(
                "persistence: commit {} is immutable (files changed)",
                id
            )));
        }
        if !value_eq(&c.field("message")?, &nc.field("message")?)? {
            return Err(Crash::new(format!(
                "persistence: commit {} is immutable (message changed)",
                id
            )));
        }
    }
    Ok(())
}
