//! Indexed snapshot lookups used by the reference configuration.

use crate::value::{value_eq, BlobVal, Crash, ListVal, Value};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Weak;

struct SnapshotIndex {
    identity: (*const Vec<Value>, usize),
    storage: Weak<Vec<Value>>,
    // None means a non-path was encountered: use structural equality, as
    // the language's entryAt does, including errors in later entries.
    paths: Option<HashMap<Vec<String>, usize>>,
}

thread_local! {
    // `diffs` alternates the before/after snapshots. Weak keys and integer
    // positions keep neither snapshot nor its blobs alive after the caller.
    static SNAPSHOTS: RefCell<VecDeque<SnapshotIndex>> = const { RefCell::new(VecDeque::new()) };
}

fn path_key(path: &Value) -> Option<Vec<String>> {
    path.as_list().ok()?.iter()
        .map(|v| v.as_text().ok().map(str::to_owned))
        .collect()
}

fn indexed_entry(path: &[String], entries: &ListVal) -> Option<Option<usize>> {
    SNAPSHOTS.with(|cache| {
        let mut cache = cache.borrow_mut();
        let identity = entries.identity();
        if let Some(pos) = cache.iter().position(|c| {
            c.identity == identity && c.storage.strong_count() > 0
        }) {
            let entry = cache.remove(pos).unwrap();
            cache.push_front(entry);
        } else {
            let paths = (|| {
                let mut paths = HashMap::new();
                for (i, entry) in entries.iter().enumerate() {
                    let path = path_key(&entry.field("path").ok()?)?;
                    paths.entry(path).or_insert(i);
                }
                Some(paths)
            })();
            cache.push_front(SnapshotIndex {
                identity,
                storage: entries.weak_storage(),
                paths,
            });
            cache.truncate(2);
        }
        cache.front().unwrap().paths.as_ref()
            .map(|paths| paths.get(path).copied())
    })
}

/// `(head (filter (\e -> e.path == path) files)).content or blob ""`.
/// Index ordinary paths; preserve the full filter's errors and first-match
/// behavior for arbitrary values the shallow Snapshot contract also admits.
pub(crate) fn content_at(path: &Value, files: &Value) -> Result<Value, Crash> {
    let lookup = || -> Result<Value, Crash> {
        if let (Some(key), Value::List(entries)) = (path_key(path), files) {
            if let Some(found) = indexed_entry(&key, entries) {
                return found.ok_or_else(|| Crash::new("head: empty list"))
                    .and_then(|i| entries[i].field("content"));
            }
        }
        let mut first = None;
        for entry in files.as_list()? {
            if value_eq(&entry.field("path")?, path)? && first.is_none() {
                first = Some(entry);
            }
        }
        first.ok_or_else(|| Crash::new("head: empty list"))?.field("content")
    };
    Ok(lookup().unwrap_or_else(|_| BlobVal::text_blob("")))
}
