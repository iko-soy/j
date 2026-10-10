//! Indexed lookups preserve the reference config's behavior on arbitrary
//! values admitted by its shallow contracts, as well as ordinary snapshots.

use j::config;
use j::domain::{MemBackend, ROOT_ID};
use j::eval::Interp;
use j::value::{value_eq, BlobContent, BlobKind, BlobVal, Crash, Env, LazyBlob, Value};
use std::cell::Cell;
use std::rc::Rc;

const CONFIG: &str = include_str!("../config.j");

fn interpreter(src: &str) -> Interp {
    let cfg = config::load_config(src).unwrap();
    let mut i = Interp::new(Rc::new(MemBackend::new()), cfg.shapes.clone(), Env::empty());
    config::eval_config(&mut i, &cfg).unwrap();
    i
}

fn previous_config() -> String {
    CONFIG
        .lines()
        .filter(|line| !line.starts_with("lookupCommit :") && !line.starts_with("lookupContent :"))
        .collect::<Vec<_>>()
        .join("\n")
        .replace("commitAt = lookupCommit", "commitAt = \\i repo -> (by i repo).root")
        .replace(
            "contentAt = lookupContent",
            "contentAt = \\p s -> (head (entryAt p s)).content or blob \"\"",
        )
}

fn call(i: &mut Interp, name: &str, a: Value, b: Value) -> Result<Value, Crash> {
    let f = i.global_env().lookup(name).unwrap();
    let f = i.apply(f, a)?;
    i.apply(f, b)
}

fn path(name: &str) -> Value {
    Value::list(vec![Value::text(name)])
}

fn entry(path: Value, content: Value) -> Value {
    Value::record(&[("path", path), ("content", content)])
}

fn file(name: &str, content: &str) -> Value {
    entry(path(name), BlobVal::text_blob(content))
}

fn assert_same(actual: Result<Value, Crash>, expected: Result<Value, Crash>, case: &str) {
    match (actual, expected) {
        (Ok(a), Ok(b)) => assert!(value_eq(&a, &b).unwrap(), "{case}: {a:?} != {b:?}"),
        (Err(a), Err(b)) => assert_eq!(a.msg, b.msg, "{case}"),
        (a, b) => panic!("{case}: {a:?} != {b:?}"),
    }
}

#[test]
fn indexed_content_matches_original_for_duplicates_and_malformed_entries() {
    let mut indexed = interpreter(CONFIG);
    let mut original = interpreter(&previous_config());
    let function = indexed.global_env().lookup("id").unwrap();
    let function_path = Value::list(vec![function]);
    let cases = vec![
        ("first duplicate", path("a"), vec![file("a", "first"), file("a", "second")]),
        ("absent", path("missing"), vec![file("a", "first")]),
        (
            "first duplicate has no content",
            path("a"),
            vec![Value::record(&[("path", path("a"))]), file("a", "second")],
        ),
        (
            "first duplicate has wrong content kind",
            path("a"),
            vec![entry(path("a"), Value::int(1)), file("a", "second")],
        ),
        (
            "irrelevant content is not read",
            path("a"),
            vec![Value::record(&[("path", path("b"))]), file("a", "first")],
        ),
        (
            "later missing path still crashes the filter",
            path("a"),
            vec![file("a", "first"), Value::record(&[("content", BlobVal::text_blob("bad"))])],
        ),
        ("later non-record", path("a"), vec![file("a", "first"), Value::int(7)]),
        (
            "non-text path still compares structurally",
            Value::list(vec![Value::int(1)]),
            vec![entry(Value::list(vec![Value::int(1)]), BlobVal::text_blob("integer path"))],
        ),
        (
            "later path of another kind compares false",
            path("a"),
            vec![file("a", "first"), entry(Value::int(7), BlobVal::text_blob("unused"))],
        ),
        (
            "function path equality crashes",
            function_path.clone(),
            vec![entry(function_path.clone(), BlobVal::text_blob("unused"))],
        ),
        (
            "function path with different length is unequal",
            Value::list(vec![]),
            vec![entry(function_path.clone(), BlobVal::text_blob("unused"))],
        ),
        (
            "function and text path components are unequal",
            path("a"),
            vec![file("a", "first"), entry(function_path, BlobVal::text_blob("unused"))],
        ),
    ];
    for (case, path, entries) in cases {
        let files = Value::list(entries);
        let expected = call(&mut original, "contentAt", path.clone(), files.clone());
        // The second call exercises cached valid and malformed snapshots.
        for _ in 0..2 {
            assert_same(
                call(&mut indexed, "contentAt", path.clone(), files.clone()),
                expected.clone(),
                case,
            );
        }
    }
}

#[test]
fn content_index_distinguishes_shared_tail_and_survives_eviction() {
    let mut i = interpreter(CONFIG);
    let whole = Value::list(vec![file("a", "first"), file("a", "second")]);
    let Value::List(entries) = &whole else { unreachable!() };
    let tail = Value::List(entries.skip(1));
    let third = Value::list(vec![file("a", "third")]);
    let fourth = Value::list(vec![file("a", "fourth")]);
    for (files, expected) in [
        (&whole, "first"), (&tail, "second"), (&whole, "first"),
        (&third, "third"), (&fourth, "fourth"), (&tail, "second"), (&whole, "first"),
    ] {
        let got = call(&mut i, "contentAt", path("a"), files.clone()).unwrap();
        assert!(value_eq(&got, &BlobVal::text_blob(expected)).unwrap());
    }
}

#[test]
fn content_index_neither_reads_blob_bytes_nor_retains_snapshot() {
    let mut i = interpreter(CONFIG);
    let reads = Rc::new(Cell::new(0));
    let observed = reads.clone();
    let blob = Rc::new(BlobVal {
        kind: BlobKind::Regular,
        content: BlobContent::Lazy(Rc::new(LazyBlob::new("lazy-content".into(), move || {
            observed.set(observed.get() + 1);
            Err(Crash::new("bytes must not be read by contentAt"))
        }))),
    });
    let weak = Rc::downgrade(&blob);
    let files = Value::list(vec![entry(path("a"), Value::Blob(blob))]);
    for _ in 0..2 {
        let got = call(&mut i, "contentAt", path("a"), files.clone()).unwrap();
        assert!(matches!(got, Value::Blob(_)));
    }
    assert_eq!(reads.get(), 0);
    drop(files);
    assert!(weak.upgrade().is_none(), "the index retained the snapshot's blob");
}

fn commit(id: &str, message: &str) -> Value {
    Value::record(&[
        ("id", Value::Id(Rc::new(id.into()))),
        ("message", Value::text(message)),
        ("labels", Value::list(vec![])),
        ("files", Value::list(vec![])),
    ])
}

fn node(root: Value, children: Vec<Value>) -> Value {
    Value::record(&[("root", root), ("children", Value::list(children))])
}

fn repo(root: Value, children: Value) -> Value {
    Value::record(&[("root", root), ("children", children), ("context", Value::list(vec![]))])
}

fn lookup(i: &mut Interp, id: &str, repo: &Value) -> Result<Value, Crash> {
    call(i, "commitAt", Value::Id(Rc::new(id.into())), repo.clone())
}

#[test]
fn commit_index_matches_navigation_from_every_focus_and_after_edits() {
    let mut indexed = interpreter(CONFIG);
    let mut original = interpreter(&previous_config());
    let whole = repo(
        commit(ROOT_ID, "root"),
        Value::list(vec![
            node(commit("kaaaaaaa", "a"), vec![node(commit("kccccccc", "c"), vec![])]),
            node(commit("kbbbbbbb", "b"), vec![]),
        ]),
    );
    let ids = [ROOT_ID, "kaaaaaaa", "kbbbbbbb", "kccccccc"];
    for focus in ids {
        let focused = j::repo::by_id(&whole, focus).unwrap().unwrap();
        for id in ids.into_iter().chain(std::iter::once("kmissing")) {
            for _ in 0..2 {
                assert_same(lookup(&mut indexed, id, &focused), lookup(&mut original, id, &focused), id);
            }
        }
    }
    let edited = repo(commit(ROOT_ID, "edited root"), whole.field("children").unwrap());
    for current in [&whole, &edited, &whole] {
        assert_same(
            lookup(&mut indexed, ROOT_ID, current),
            lookup(&mut original, ROOT_ID, current),
            "record identity after edit",
        );
    }
}

#[test]
fn commit_index_preserves_first_duplicate_and_early_hits_in_malformed_trees() {
    let mut indexed = interpreter(CONFIG);
    let mut original = interpreter(&previous_config());
    let cases = [
        repo(commit(ROOT_ID, "root"), Value::list(vec![
            node(commit("kaaaaaaa", "first"), vec![node(commit("kaaaaaaa", "nested duplicate"), vec![])]),
            node(commit("kaaaaaaa", "second"), vec![]),
        ])),
        repo(commit(ROOT_ID, "root"), Value::list(vec![node(commit("kaaaaaaa", "a"), vec![]), Value::int(7)])),
        repo(commit(ROOT_ID, "root"), Value::int(7)),
        repo(commit(ROOT_ID, "root"), Value::list(vec![
            Value::record(&[("root", commit("kaaaaaaa", "a")), ("children", Value::int(7))]),
            node(commit("kbbbbbbb", "b"), vec![]),
        ])),
    ];
    for current in cases {
        for id in [ROOT_ID, "kaaaaaaa", "kbbbbbbb", "kmissing"] {
            for _ in 0..2 {
                assert_same(lookup(&mut indexed, id, &current), lookup(&mut original, id, &current), id);
            }
        }
    }
}

#[test]
fn existing_configs_keep_their_original_lookup_definitions() {
    let mut i = interpreter(&previous_config());
    assert!(i.global_env().lookup("lookupCommit").is_none());
    assert!(i.global_env().lookup("lookupContent").is_none());
    let original_repo = repo(commit(ROOT_ID, "old config"), Value::list(vec![]));
    let got = lookup(&mut i, ROOT_ID, &original_repo).unwrap();
    assert_eq!(got.field("message").unwrap().as_text().unwrap(), "old config");
    let got = call(&mut i, "contentAt", path("a"), Value::list(vec![file("a", "old config")])).unwrap();
    assert!(value_eq(&got, &BlobVal::text_blob("old config")).unwrap());
}
