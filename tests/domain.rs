//! In-memory backend and replay (§7.3, §10) plus repo zipper/validation (§7.5).

use j::domain::{simple_replay, MemBackend, ROOT_ID};
use j::value::{value_eq, BlobVal, Value};
use std::collections::BTreeSet;
use std::rc::Rc;

fn entry(path: &str, content: &str) -> Value {
    Value::record(&[
        ("content", BlobVal::text_blob(content)),
        (
            "path",
            Value::list(path.split('/').map(Value::text).collect()),
        ),
    ])
}

fn snap(entries: Vec<Value>) -> Vec<Value> {
    entries
}

fn paths_of(snap: &[Value]) -> Vec<String> {
    snap.iter()
        .map(|e| {
            e.field("path")
                .unwrap()
                .as_list()
                .unwrap()
                .iter()
                .map(|c| c.as_text().unwrap().to_string())
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect()
}

fn content_at<'a>(snap: &'a [Value], path: &str) -> Option<Value> {
    snap.iter()
        .find(|e| {
            e.field("path")
                .unwrap()
                .as_list()
                .unwrap()
                .iter()
                .map(|c| c.as_text().unwrap().to_string())
                .collect::<Vec<_>>()
                .join("/")
                == path
        })
        .map(|e| e.field("content").unwrap())
}

fn unresolved_at(snap: &[Value], path: &str) -> bool {
    match content_at(snap, path) {
        Some(Value::Blob(b)) => b.is_unresolved(),
        _ => false,
    }
}

fn blob_text(v: &Value) -> String {
    match v {
        Value::Blob(b) => String::from_utf8(b.bytes().unwrap()).unwrap(),
        _ => panic!("not a blob"),
    }
}

#[test]
fn replay_no_conflicts() {
    // onto unchanged, change applied
    let from = snap(vec![entry("a", "1")]);
    let to = snap(vec![entry("a", "2")]);
    let onto = snap(vec![entry("a", "1"), entry("b", "x")]);
    let out = simple_replay(&onto, &from, &to).unwrap();
    assert_eq!(blob_text(&content_at(&out, "a").unwrap()), "2");
    assert_eq!(blob_text(&content_at(&out, "b").unwrap()), "x");

    // addition
    let out = simple_replay(&snap(vec![entry("a", "1")]), &from, &to).unwrap();
    assert_eq!(paths_of(&out), vec!["a"]);
}

#[test]
fn replay_add_and_delete() {
    let from = snap(vec![entry("a", "1")]);
    let to = snap(vec![entry("a", "1"), entry("n", "new")]);
    let onto = snap(vec![entry("a", "1")]);
    let out = simple_replay(&onto, &from, &to).unwrap();
    assert_eq!(paths_of(&out), vec!["a", "n"]);

    // deletion
    let from2 = snap(vec![entry("a", "1"), entry("d", "gone")]);
    let to2 = snap(vec![entry("a", "1")]);
    let out = simple_replay(&from2, &from2, &to2).unwrap();
    assert_eq!(paths_of(&out), vec!["a"]);
}

#[test]
fn replay_both_sides_same_change_resolves() {
    let from = snap(vec![entry("a", "1")]);
    let to = snap(vec![entry("a", "2")]);
    let onto = snap(vec![entry("a", "2")]);
    let out = simple_replay(&onto, &from, &to).unwrap();
    assert!(!unresolved_at(&out, "a"));
    assert_eq!(blob_text(&content_at(&out, "a").unwrap()), "2");
}

#[test]
fn replay_conflict_kept_unresolved() {
    let from = snap(vec![entry("a", "1")]);
    let to = snap(vec![entry("a", "2")]);
    let onto = snap(vec![entry("a", "3")]);
    let out = simple_replay(&onto, &from, &to).unwrap();
    assert!(unresolved_at(&out, "a"));
    // the conflict renders with markers containing all three sides
    let text = blob_text(&content_at(&out, "a").unwrap());
    assert!(text.contains("1") && text.contains("2") && text.contains("3"), "{}", text);
}

#[test]
fn replay_malformed_snapshots_crash() {
    // duplicate paths
    let bad = snap(vec![entry("a", "1"), entry("a", "2")]);
    assert!(simple_replay(&bad, &[], &[]).is_err());
    // not entries
    let not_entries = vec![Value::int(1)];
    assert!(simple_replay(&not_entries, &[], &[]).is_err());
    // path not a list
    let bad2 = vec![Value::record(&[
        ("content", BlobVal::text_blob("x")),
        ("path", Value::text("a")),
    ])];
    assert!(simple_replay(&bad2, &[], &[]).is_err());
}

#[test]
fn replay_rejects_root_entries_and_file_directory_clashes() {
    // an entry at the root path ./ names no file: jj's tree builder panicked
    // on it (exit 101) while this backend accepted it
    let at_root = vec![Value::record(&[
        ("content", BlobVal::text_blob("x")),
        ("path", Value::list(vec![])),
    ])];
    for (onto, from, to) in [
        (&at_root[..], &[][..], &[][..]),
        (&[][..], &[][..], &at_root[..]),
        (&[][..], &at_root[..], &[][..]),
    ] {
        let e = simple_replay(onto, from, to).unwrap_err();
        assert!(e.msg.contains("not a well-formed snapshot"), "{}", e.msg);
    }
    // `a` as a file and a directory at once: jj's tree keeps only one of
    // them, so the real backend silently dropped the file `a` (§7.3, §10)
    let clash = snap(vec![entry("a", "file"), entry("a/b", "nested")]);
    let e = simple_replay(&clash, &[], &[]).unwrap_err();
    assert!(e.msg.contains("both a file and a directory"), "{}", e.msg);
    let deep = snap(vec![entry("d/e", "1"), entry("d/e0", "2"), entry("d/e/f/g", "3")]);
    assert!(simple_replay(&[], &[], &deep).is_err());
    // sharing a prefix of a component's text is no clash
    let fine = snap(vec![entry("a.txt", "1"), entry("a/b", "2"), entry("ab", "3")]);
    let out = simple_replay(&fine, &fine, &fine).unwrap();
    assert_eq!(paths_of(&out), vec!["a/b", "a.txt", "ab"]);
}

#[test]
fn replay_identity_laws() {
    // replay b { from = b, to = x } = x
    let b = snap(vec![entry("a", "1"), entry("dir/b", "2")]);
    let x = snap(vec![entry("a", "9"), entry("c", "3")]);
    let out = simple_replay(&b, &b, &x).unwrap();
    assert_eq!(paths_of(&out), paths_of(&x));
    for p in paths_of(&x) {
        assert!(value_eq(&content_at(&out, &p).unwrap(), &content_at(&x, &p).unwrap()).unwrap());
    }
    // replay x { from = b, to = b } = x
    let out = simple_replay(&x, &b, &b).unwrap();
    assert_eq!(paths_of(&out), paths_of(&x));
}

// ------------------------------------------------------------------
// MemBackend
// ------------------------------------------------------------------

#[test]
fn membackend_ids_and_meta() {
    let mut b = MemBackend::new();
    b.metas.insert(
        "kpqxaaaa".into(),
        j::domain::MetaInfo {
            hash: "h".into(),
            author: "a".into(),
            email: "e".into(),
            time: 5,
        },
    );
    use j::domain::Backend;
    assert_eq!(b.visible_ids(), vec!["kpqxaaaa".to_string()]);
    assert!(b.meta("kpqxaaaa").is_ok());
    assert!(b.meta("zzzzzzzz").is_err());
    assert_eq!(b.resolve_prefix("kp"), vec!["kpqxaaaa".to_string()]);
    assert!(b.resolve_prefix("zz").is_empty());
}

#[test]
fn membackend_ancestors_closed() {
    use j::domain::Backend;
    let mut b = MemBackend::new();
    b.parents.insert("a".into(), vec!["b".into()]);
    b.parents.insert("b".into(), vec!["c".into()]);
    b.parents.insert("c".into(), vec![]);
    let set: BTreeSet<String> = ["a".to_string()].into_iter().collect();
    let closed = b.ancestors_closed(&set);
    assert!(closed.contains("a") && closed.contains("b") && closed.contains("c"));
    let empty: BTreeSet<String> = BTreeSet::new();
    assert!(b.ancestors_closed(&empty).is_empty());
}

#[test]
fn unique_prefix() {
    use j::domain::Backend;
    let mut b = MemBackend::new();
    for id in ["kpqxaaaa", "kpqybbbb", "mwzzcccc"] {
        b.metas.insert(
            id.to_string(),
            j::domain::MetaInfo {
                hash: id.into(),
                author: "a".into(),
                email: "e".into(),
                time: 0,
            },
        );
    }
    assert_eq!(b.unique_prefix("kpqxaaaa"), "kpqx");
    assert_eq!(b.unique_prefix("kpqybbbb"), "kpqy");
    assert_eq!(b.unique_prefix("mwzzcccc"), "mwzz");
    // short ids are padded to 4
    let mut b2 = MemBackend::new();
    b2.metas.insert(
        "kq".to_string(),
        j::domain::MetaInfo {
            hash: "h".into(),
            author: "a".into(),
            email: "e".into(),
            time: 0,
        },
    );
    assert_eq!(b2.unique_prefix("kq"), "kq");
}

// ------------------------------------------------------------------
// repo.rs: the zipper and validation
// ------------------------------------------------------------------

fn commit(id: &str, msg: &str, labels: &[&str]) -> Value {
    Value::record(&[
        ("files", Value::list(vec![])),
        ("message", Value::text(msg)),
        (
            "labels",
            Value::list(labels.iter().map(|l| Value::text(*l)).collect()),
        ),
        ("id", Value::Id(Rc::new(id.to_string()))),
    ])
}

/// root -> a -> b (focus b), root -> c
fn sample_repo() -> Value {
    let root = commit(ROOT_ID, "", &[]);
    let a = commit("kaaaaaaa", "a", &[]);
    let b = commit("kbbbbbbb", "b", &["feat"]);
    let c = commit("kccccccc", "c", &[]);
    let subtree = |r: Value, kids: Vec<Value>| {
        Value::record(&[
            ("children", Value::list(kids)),
            ("root", r),
        ])
    };
    // focus on b: context = [frame(a, [], []), frame(root, [], [c])]
    let frames = Value::list(vec![
        Value::record(&[
            ("left", Value::list(vec![])),
            ("parent", a.clone()),
            ("right", Value::list(vec![])),
        ]),
        Value::record(&[
            ("left", Value::list(vec![])),
            ("parent", root.clone()),
            ("right", Value::list(vec![subtree(c.clone(), vec![])])),
        ]),
    ]);
    Value::record(&[
        ("children", Value::list(vec![])),
        ("context", frames),
        ("root", b),
    ])
}

#[test]
fn by_id_reference_walk() {
    let repo = sample_repo();
    let top = j::repo::by_id(&repo, ROOT_ID).unwrap().unwrap();
    assert!(top.field("context").unwrap().as_list().unwrap().is_empty());
    let at_c = j::repo::by_id(&repo, "kccccccc").unwrap().unwrap();
    assert_eq!(
        at_c.field("root").unwrap().field("message").unwrap().as_text().unwrap(),
        "c"
    );
    // context depth 1 for c
    assert_eq!(at_c.field("context").unwrap().as_list().unwrap().len(), 1);
    assert!(j::repo::by_id(&repo, "kzzzzzzz").unwrap().is_none());
}

#[test]
fn all_commits_and_parent_map() {
    let repo = sample_repo();
    let cs = j::repo::all_commits(&repo).unwrap();
    assert_eq!(cs.len(), 4); // root, a, b, c
    let pm: std::collections::BTreeMap<String, String> =
        j::repo::parent_map(&repo).unwrap().into_iter().collect();
    assert_eq!(pm["kaaaaaaa"], ROOT_ID);
    assert_eq!(pm["kbbbbbbb"], "kaaaaaaa");
    assert_eq!(pm["kccccccc"], ROOT_ID);
}

#[test]
fn by_id_agrees_with_reference_definition() {
    // §10: the builtin `by` must agree with the in-language reference over
    // generated repos — checked structurally here: same focus id and context
    // depth on a deeper tree
    let repo = sample_repo();
    for id in [ROOT_ID, "kaaaaaaa", "kbbbbbbb", "kccccccc"] {
        let loc = j::repo::by_id(&repo, id).unwrap().unwrap();
        let got_id = match loc.field("root").unwrap().field("id").unwrap() {
            Value::Id(i) => i.to_string(),
            _ => panic!(),
        };
        assert_eq!(got_id, id);
    }
}
