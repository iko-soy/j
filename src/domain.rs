//! Domain abstraction: everything repository-shaped the interpreter needs
//! (§7, §10). Two implementations: an in-memory backend for tests, and the
//! jj-lib backend.

use crate::value::{BlobContent, BlobKind, BlobVal, Crash, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

#[derive(Clone, Debug)]
pub struct MetaInfo {
    pub hash: String,
    pub author: String,
    pub email: String,
    /// seconds since unix epoch
    pub time: i64,
}

/// The result of resolving an id prefix (§4.10).
pub enum IdResolution {
    Ok(String),
    NoMatch,
    Ambiguous(Vec<String>),
}

/// Immutable-set computation needs this from the backend (§7.5).
pub struct ImmutabilityInfo {
    /// change ids that are merges (2+ parents) plus all their ancestors
    pub merge_closed: BTreeSet<String>,
}

pub trait Backend {
    /// All visible change ids (full), for resolving `@prefix` literals.
    fn visible_ids(&self) -> Vec<String>;
    /// All visible ids beginning with `prefix`.
    fn resolve_prefix(&self, prefix: &str) -> Vec<String> {
        self.visible_ids()
            .into_iter()
            .filter(|id| id.starts_with(prefix))
            .collect()
    }
    /// Metadata for a stored commit, by change id.
    fn meta(&self, id: &str) -> Result<MetaInfo, Crash>;
    /// True if the change id denotes a merge commit (2+ parents).
    fn is_merge(&self, id: &str) -> bool;
    /// True if the commit's tree contains a conflict, if the backend can say
    /// without reading files (jj: O(1)); None means "unknown, compare files".
    fn has_conflict(&self, _id: &str) -> Option<bool> {
        None
    }
    /// True if the commit's tree equals its first parent's tree, if the
    /// backend can say without reading files; None means "compare files".
    fn is_empty(&self, _id: &str) -> Option<bool> {
        None
    }
    /// All ancestors (inclusive) of the given change ids.
    fn ancestors_closed(&self, ids: &BTreeSet<String>) -> BTreeSet<String>;

    /// jj tree merge (§7.3): replay the change from->to onto `onto`.
    fn replay(
        &self,
        onto: &[Value],
        from: &[Value],
        to: &[Value],
    ) -> Result<Vec<Value>, Crash>;

    /// shortest prefix of `id` unique among visible ids, min length 4
    fn unique_prefix(&self, id: &str) -> String {
        let others: Vec<String> = self.visible_ids();
        let mut n = 4.min(id.len());
        while n < id.len() {
            let p = &id[..n];
            if others.iter().filter(|o| o.starts_with(p) && *o != id).count() == 0 {
                return p.to_string();
            }
            n += 1;
        }
        id.to_string()
    }
}

// ----------------------------------------------------------------------
// snapshot helpers shared by backends
// ----------------------------------------------------------------------

/// entries as a path->value map; crashes on malformed snapshots (§7.3)
pub fn snapshot_map(entries: &[Value]) -> Result<BTreeMap<Vec<String>, Value>, Crash> {
    let mut m = BTreeMap::new();
    for e in entries {
        let path_v = e.field("path").map_err(|_| {
            Crash::new("replay: not a well-formed snapshot (entry without `path`)")
        })?;
        let content = e.field("content").map_err(|_| {
            Crash::new("replay: not a well-formed snapshot (entry without `content`)")
        })?;
        if !matches!(content, Value::Blob(_)) {
            return Err(Crash::new(
                "replay: not a well-formed snapshot (content is not a Blob)",
            ));
        }
        let path = path_v
            .as_list()
            .map_err(|_| Crash::new("replay: not a well-formed snapshot (path not a list)"))?
            .iter()
            .map(|c| {
                c.as_text()
                    .map(|s| s.to_string())
                    .map_err(|_| Crash::new("replay: not a well-formed snapshot (path component not Text)"))
            })
            .collect::<Result<Vec<String>, Crash>>()?;
        // `./` is the root directory, which no entry can be
        if path.is_empty() {
            return Err(Crash::new(
                "replay: not a well-formed snapshot (an entry at the root path ./)",
            ));
        }
        if m.insert(path, content).is_some() {
            return Err(Crash::new("replay: not a well-formed snapshot (duplicate paths)"));
        }
    }
    if let Some(p) = file_and_directory(m.keys()) {
        return Err(Crash::new(format!(
            "replay: not a well-formed snapshot (`{}` is both a file and a directory)",
            p.join("/")
        )));
    }
    Ok(m)
}

/// The first of `sorted` paths that is also a directory of another: a tree
/// holds `a` as a file or as a directory, never both (§7.3). A path's
/// descendants sort directly after it (`a` < `a/b` < `a.txt`), so comparing
/// each path with the next finds every clash.
pub fn file_and_directory<'a>(
    sorted: impl IntoIterator<Item = &'a Vec<String>>,
) -> Option<&'a [String]> {
    let mut prev: Option<&'a Vec<String>> = None;
    for p in sorted {
        if let Some(q) = prev {
            if p.len() > q.len() && p.starts_with(q) {
                return Some(q);
            }
        }
        prev = Some(p);
    }
    None
}

pub fn map_to_snapshot(m: BTreeMap<Vec<String>, Value>) -> Vec<Value> {
    m.into_iter()
        .map(|(path, content)| {
            let p = Value::list(path.into_iter().map(Value::text).collect());
            Value::record(&[("content", content), ("path", p)])
        })
        .collect()
}

fn blob_eq(a: &Value, b: &Value) -> Result<bool, Crash> {
    Ok(match (a, b) {
        (Value::Blob(x), Value::Blob(y)) => {
            x.kind == y.kind
                && match (&x.content, &y.content) {
                    (BlobContent::Resolved(p), BlobContent::Resolved(q)) => p == q,
                    (BlobContent::Lazy(p), BlobContent::Lazy(q)) => p.id == q.id,
                    (BlobContent::Lazy(p), BlobContent::Resolved(q))
                    | (BlobContent::Resolved(q), BlobContent::Lazy(p)) => p.force()? == *q,
                    (BlobContent::Conflict(p), BlobContent::Conflict(q)) => p == q,
                    _ => false,
                }
        }
        _ => false,
    })
}

fn empty_blob() -> Value {
    Value::Blob(Rc::new(BlobVal {
        kind: BlobKind::Regular,
        content: BlobContent::Resolved(Rc::new(Vec::new())),
    }))
}

fn conflict_blob(sides: Vec<Rc<Vec<u8>>>) -> Value {
    Value::Blob(Rc::new(BlobVal {
        kind: BlobKind::Regular,
        content: BlobContent::Conflict(sides),
    }))
}

/// A simple per-path three-way merge usable by the in-memory backend and as
/// a reference. A path resolves when at most one side changed it, or both
/// made the same change (§10); otherwise a conflict blob is produced.
pub fn simple_replay(
    onto: &[Value],
    from: &[Value],
    to: &[Value],
) -> Result<Vec<Value>, Crash> {
    let onto = snapshot_map(onto)?;
    let from = snapshot_map(from)?;
    let to = snapshot_map(to)?;
    let mut paths: BTreeSet<Vec<String>> = BTreeSet::new();
    paths.extend(onto.keys().cloned());
    paths.extend(from.keys().cloned());
    paths.extend(to.keys().cloned());
    let absent = empty_blob();
    let mut out = BTreeMap::new();
    for p in paths {
        let o = onto.get(&p).cloned().unwrap_or_else(|| absent.clone());
        let f = from.get(&p).cloned().unwrap_or_else(|| absent.clone());
        let t = to.get(&p).cloned().unwrap_or_else(|| absent.clone());
        let o_present = onto.contains_key(&p);
        let f_present = from.contains_key(&p);
        let t_present = to.contains_key(&p);
        let changed_to = !(f_present == t_present && (!f_present || blob_eq(&f, &t)?));
        let changed_onto = !(f_present == o_present && (!f_present || blob_eq(&f, &o)?));
        match (changed_to, changed_onto) {
            (false, false) => {
                if o_present {
                    out.insert(p, o);
                }
            }
            (true, false) => {
                if t_present {
                    out.insert(p, t);
                }
            }
            (false, true) => {
                if o_present {
                    out.insert(p, o);
                }
            }
            (true, true) => {
                if t_present && o_present && blob_eq(&t, &o)? {
                    out.insert(p, t);
                } else if !t_present && !o_present {
                    // both deleted
                } else {
                    // conflict: sides [to, from, onto] (add, remove, add)
                    let get_bytes = |v: &Value, present: bool| -> Result<Rc<Vec<u8>>, Crash> {
                        if !present {
                            return Ok(Rc::new(Vec::new()));
                        }
                        match v {
                            Value::Blob(b) => Ok(Rc::new(b.bytes()?)),
                            _ => Ok(Rc::new(Vec::new())),
                        }
                    };
                    out.insert(
                        p,
                        conflict_blob(vec![
                            get_bytes(&t, t_present)?,
                            get_bytes(&f, f_present)?,
                            get_bytes(&o, o_present)?,
                        ]),
                    );
                }
            }
        }
    }
    Ok(map_to_snapshot(out))
}

// ----------------------------------------------------------------------
// In-memory backend (§10)
// ----------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct MemBackend {
    /// id -> meta
    pub metas: HashMap<String, MetaInfo>,
    /// id -> parent change ids (first parent first)
    pub parents: HashMap<String, Vec<String>>,
    /// Answer `has_conflict` and `is_empty` from the two maps below instead of
    /// returning None, the way the jj backend answers them from tree ids. A
    /// backend that answers them takes different paths through rendering and
    /// never forces a commit's file list, so tests that only ever used the
    /// None-answering default could not reach those paths at all.
    pub answers_tree_queries: bool,
    /// id -> the commit's tree holds a conflict
    pub conflicts: HashMap<String, bool>,
    /// id -> the commit's tree equals its first parent's
    pub empties: HashMap<String, bool>,
}

impl MemBackend {
    pub fn new() -> Self {
        MemBackend::default()
    }

    /// A backend that answers the tree queries, like the jj backend does.
    pub fn answering() -> Self {
        MemBackend {
            answers_tree_queries: true,
            ..MemBackend::default()
        }
    }
}

/// A snapshot that is only materialised on first use, the way the jj backend
/// builds a commit's `files` (§7.2). The laziness must stay invisible to the
/// language, which is exactly what is easy to get wrong, so tests need to be
/// able to build repos this way.
pub fn lazy_files(entries: Vec<Value>) -> Value {
    Value::Thunk(Rc::new(crate::value::ThunkVal::new(move || {
        Ok(Value::list(entries))
    })))
}

impl Backend for MemBackend {
    fn visible_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.metas.keys().cloned().collect();
        ids.sort();
        ids
    }
    fn meta(&self, id: &str) -> Result<MetaInfo, Crash> {
        self.metas
            .get(id)
            .cloned()
            .ok_or_else(|| Crash::new(format!("meta: no stored commit for `{}`", id)))
    }
    fn is_merge(&self, id: &str) -> bool {
        self.parents.get(id).map(|p| p.len() >= 2).unwrap_or(false)
    }
    fn has_conflict(&self, id: &str) -> Option<bool> {
        if !self.answers_tree_queries {
            return None;
        }
        Some(self.conflicts.get(id).copied().unwrap_or(false))
    }
    fn is_empty(&self, id: &str) -> Option<bool> {
        if !self.answers_tree_queries {
            return None;
        }
        // like jj: only answerable against a recorded first parent
        self.parents.get(id)?.first()?;
        Some(self.empties.get(id).copied().unwrap_or(false))
    }
    fn ancestors_closed(&self, ids: &BTreeSet<String>) -> BTreeSet<String> {
        let mut seen = BTreeSet::new();
        let mut stack: Vec<String> = ids.iter().cloned().collect();
        while let Some(id) = stack.pop() {
            if seen.insert(id.clone()) {
                if let Some(ps) = self.parents.get(&id) {
                    for p in ps {
                        stack.push(p.clone());
                    }
                }
            }
        }
        seen
    }
    fn replay(&self, onto: &[Value], from: &[Value], to: &[Value]) -> Result<Vec<Value>, Crash> {
        simple_replay(onto, from, to)
    }
}

/// The root commit's change id in jj repositories.
pub const ROOT_ID: &str = "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz";
