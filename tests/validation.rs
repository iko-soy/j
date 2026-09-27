//! Persistence validation tests (§7.5 steps 1–3, 6) and config validation (§6.2).

use j::config;
use j::domain::{MemBackend, MetaInfo, ROOT_ID};
use j::eval::Interp;
use j::value::{Env, Value};
use std::rc::Rc;

const CONFIG: &str = include_str!("../config.j");

fn commit(id: &str, msg: &str, labels: &[&str], files: Vec<Value>) -> Value {
    Value::record(&[
        ("files", Value::list(files)),
        ("message", Value::text(msg)),
        (
            "labels",
            Value::list(labels.iter().map(|l| Value::text(*l)).collect()),
        ),
        ("id", Value::Id(Rc::new(id.to_string()))),
    ])
}

fn entry(path: &str, content: &str) -> Value {
    Value::record(&[
        ("content", j::value::BlobVal::text_blob(content)),
        (
            "path",
            Value::list(path.split('/').map(Value::text).collect()),
        ),
    ])
}

fn subtree(root: Value, kids: Vec<Value>) -> Value {
    Value::record(&[
        ("children", Value::list(kids)),
        ("root", root),
    ])
}

/// root -> a, focused on a
fn two_commit_repo(labels_a: &[&str]) -> Value {
    let root = commit(ROOT_ID, "", &[], vec![]);
    let a = commit("kaaaaaaa", "a", labels_a, vec![]);
    let frame = Value::record(&[
        ("left", Value::list(vec![])),
        ("parent", root),
        ("right", Value::list(vec![])),
    ]);
    Value::record(&[
        ("children", Value::list(vec![])),
        ("context", Value::list(vec![frame])),
        ("root", a),
    ])
}

fn backend_with(ids: &[&str]) -> MemBackend {
    let mut b = MemBackend::new();
    for id in ids {
        b.metas.insert(
            id.to_string(),
            MetaInfo {
                hash: id.to_string(),
                author: "a".into(),
                email: "e".into(),
                time: 1,
            },
        );
    }
    b
}

fn make_interp(b: MemBackend) -> Interp {
    let cfg = config::load_config(CONFIG).unwrap();
    let mut i = Interp::new(Rc::new(b), cfg.shapes.clone(), Env::empty());
    config::eval_config(&mut i, &cfg).unwrap();
    i
}

fn validate_with(old: &Value, new: &Value) -> Result<(), String> {
    let mut i = make_interp(backend_with(&["kaaaaaaa", "kbbbbbbb"]));
    *i.old_repo.borrow_mut() = Some(old.clone());
    j::repo::validate_repo(&mut i, new)
        .map(|_| ())
        .map_err(|c| c.msg)
}

#[test]
fn valid_repo_passes() {
    let old = two_commit_repo(&[]);
    assert!(validate_with(&old, &old).is_ok());
}

#[test]
fn duplicate_ids_rejected() {
    let old = two_commit_repo(&[]);
    // attach a as a child of itself
    let root_a = old.field("root").unwrap();
    let dup = Value::record(&[
        ("children", Value::list(vec![subtree(root_a.clone(), vec![])])),
        ("context", old.field("context").unwrap()),
        ("root", root_a),
    ]);
    let e = validate_with(&old, &dup).unwrap_err();
    assert!(e.contains("more than once"), "{}", e);
}

#[test]
fn duplicate_paths_rejected() {
    let old = two_commit_repo(&[]);
    let bad_files = vec![entry("a", "1"), entry("a", "2")];
    let root = commit("kaaaaaaa", "a", &[], bad_files);
    let new = Value::record(&[
        ("children", old.field("children").unwrap()),
        ("context", old.field("context").unwrap()),
        ("root", root),
    ]);
    let e = validate_with(&old, &new).unwrap_err();
    assert!(e.contains("duplicate paths"), "{}", e);
}

/// `two_commit_repo` with the focus `a` holding `files`
fn focus_with_files(files: Vec<Value>) -> Value {
    let old = two_commit_repo(&[]);
    Value::record(&[
        ("children", old.field("children").unwrap()),
        ("context", old.field("context").unwrap()),
        ("root", commit("kaaaaaaa", "a", &[], files)),
    ])
}

fn entry_at(path: &[&str], content: &str) -> Value {
    Value::record(&[
        ("content", j::value::BlobVal::text_blob(content)),
        ("path", Value::list(path.iter().map(|c| Value::text(*c)).collect())),
    ])
}

#[test]
fn root_path_entry_rejected() {
    // `./` is the root, which cannot hold a file: persistence panicked in
    // jj's tree builder (exit 101) while validate let it through
    let old = two_commit_repo(&[]);
    let new = focus_with_files(vec![entry_at(&[], "x")]);
    let e = validate_with(&old, &new).unwrap_err();
    assert!(e.contains("root path"), "{}", e);
}

#[test]
fn file_and_directory_at_one_path_rejected() {
    // a jj tree cannot hold both; persisting kept `a/b` and silently dropped
    // the file `a`
    let old = two_commit_repo(&[]);
    let new = focus_with_files(vec![entry("a", "file"), entry("a/b", "nested")]);
    let e = validate_with(&old, &new).unwrap_err();
    assert!(e.contains("both a file and a directory"), "{}", e);
    let deep = focus_with_files(vec![entry("d/e", "1"), entry("d/e0", "2"), entry("d/e/f/g", "3")]);
    assert!(validate_with(&old, &deep).is_err());
    let fine = focus_with_files(vec![entry("a.txt", "1"), entry("a/b", "2"), entry("ab", "3")]);
    assert!(validate_with(&old, &fine).is_ok());
}

#[test]
fn unwritable_path_components_rejected() {
    // validate accepted every one of these. Persisting `.`, `..`, `.git` or
    // `.jj` recorded the operation before the checkout refused it, wedging
    // the repository; the rest crashed only when the tree was written.
    let old = two_commit_repo(&[]);
    // longer than ext4's name limit (NAME_MAX, 255 bytes), which the
    // checkout found only after the operation was recorded; bytes, not
    // characters, on the in-memory backend's filesystem (`name_fits`)
    let (long, wide, longest) = ("x".repeat(256), "é".repeat(128), "x".repeat(255));
    for bad in [
        &["..", "evil"][..],
        &["x", ".."][..],
        &["."][..],
        &[".git", "hooks", "post-checkout"][..],
        &["a", ".git"][..],
        &[".jj", "x"][..],
        &[""][..],
        &["a/b"][..],
        &["x\0y"][..],
        &[long.as_str()][..],
        &["d", wide.as_str(), "x"][..],
    ] {
        let new = focus_with_files(vec![entry_at(bad, "pwn")]);
        let e = validate_with(&old, &new).unwrap_err();
        assert!(e.contains("path component"), "{:?}: {}", bad, e);
    }
    // names that merely resemble them are ordinary files; jj's snapshot
    // tracks `.GIT` on a case-sensitive filesystem, so refusing it would
    // refuse the working directory itself
    for ok in [
        &["..."][..],
        &[".gitignore"][..],
        &[".jjconfig", ".git.d"][..],
        &[".GIT", "x"][..],
        &[longest.as_str()][..],
    ] {
        let new = focus_with_files(vec![entry_at(ok, "fine")]);
        assert!(validate_with(&old, &new).is_ok(), "{:?}", ok);
    }
}

#[test]
fn names_the_working_directory_holds_are_not_refused_for_length() {
    // 255 bytes is the name limit of ext4, but exFAT, NTFS and APFS count
    // 255 UTF-16 units or characters and hold these names. Once one was in
    // the working directory every persisting run was refused for it, `j id`
    // included, though the checkout never creates a name the directory
    // already holds (§7.5 step 1)
    let (wide, accented) = ("日".repeat(100), "é".repeat(128));
    let on_disk = vec![
        entry("a.txt", "x"),
        entry_at(&[&wide], "y"),
        entry_at(&[&accented, "x"], "z"),
    ];
    // the repository as loaded, and as the snapshot of the directory found it
    let loaded = two_commit_repo(&[]);
    let snapshot = focus_with_files(on_disk.clone());
    let mut i = make_interp(backend_with(&["kaaaaaaa", "kbbbbbbb"]));
    *i.old_repo.borrow_mut() = Some(loaded);
    *i.given_repo.borrow_mut() = Some(snapshot.clone());
    let mut check = |new: &Value| j::repo::validate_repo(&mut i, new).map(|_| ()).map_err(|c| c.msg);
    assert_eq!(check(&snapshot), Ok(()));
    let with = |extra: Value| {
        let mut files = on_disk.clone();
        files.push(extra);
        focus_with_files(files)
    };
    assert_eq!(check(&with(entry_at(&[&accented, "new"], "w"))), Ok(()));
    // `new`: a child of the snapshot holding the same names
    let frame = |parent: Value| {
        Value::record(&[
            ("left", Value::list(vec![])),
            ("parent", parent),
            ("right", Value::list(vec![])),
        ])
    };
    let child = Value::record(&[
        ("children", Value::list(vec![])),
        (
            "context",
            Value::list(vec![
                frame(snapshot.field("root").unwrap()),
                frame(commit(ROOT_ID, "", &[], vec![])),
            ]),
        ),
        ("root", commit("knnnnnnn", "", &[], on_disk.clone())),
    ]);
    assert_eq!(check(&child), Ok(()));
    // a long name the directory does not hold is asked of the backend,
    // which here holds 255 bytes
    let e = check(&with(entry_at(&["日".repeat(101).as_str()], "v"))).unwrap_err();
    assert!(e.contains("path component"), "{}", e);
    // `validate` with no snapshot reads the loaded focus
    assert_eq!(validate_with(&snapshot, &snapshot), Ok(()));
}

#[test]
fn immutable_commits_keep_stored_path_components() {
    // an immutable commit is left exactly as stored (§7.5 step 3), so a name
    // the history already holds must not stop every later persist
    let root = commit(ROOT_ID, "", &[], vec![]);
    let a = commit("kaaaaaaa", "a", &[], vec![entry(".jj/x", "vendored")]);
    let frame = |parent: Value| {
        Value::record(&[
            ("left", Value::list(vec![])),
            ("parent", parent),
            ("right", Value::list(vec![])),
        ])
    };
    let focus = |files: Vec<Value>| {
        Value::record(&[
            ("children", Value::list(vec![])),
            ("context", Value::list(vec![frame(a.clone()), frame(root.clone())])),
            ("root", commit("kbbbbbbb", "b", &[], files)),
        ])
    };
    // a is a merge, so immutable
    let mut b = backend_with(&["kaaaaaaa", "kbbbbbbb"]);
    b.parents.insert(
        "kaaaaaaa".to_string(),
        vec![ROOT_ID.to_string(), "kyyyyyyy".to_string()],
    );
    let mut i = make_interp(b);
    let old = focus(vec![]);
    *i.old_repo.borrow_mut() = Some(old.clone());
    assert!(j::repo::validate_repo(&mut i, &old).is_ok());
    // the mutable focus is still checked
    let bad = focus(vec![entry(".jj/x", "copied")]);
    let e = j::repo::validate_repo(&mut i, &bad).unwrap_err();
    assert!(e.msg.contains("path component"), "{}", e.msg);
}

#[test]
fn case_folding_filesystem_refuses_reserved_names_in_any_case() {
    // where the filesystem folds case, `.GIT` is the name `.git`, which jj's
    // checkout refuses at every depth, after the operation was recorded
    let old = two_commit_repo(&[]);
    let mut b = backend_with(&["kaaaaaaa", "kbbbbbbb"]);
    b.folds_case = true;
    let mut i = make_interp(b);
    *i.old_repo.borrow_mut() = Some(old.clone());
    for bad in [
        &[".GIT", "hooks", "post-checkout"][..],
        &["sub", ".Jj", "x"][..],
        &[".JJ"][..],
        &[".git"][..],
        &[".."][..],
    ] {
        let new = focus_with_files(vec![entry_at(bad, "pwn")]);
        let e = j::repo::validate_repo(&mut i, &new).unwrap_err();
        assert!(e.msg.contains("path component"), "{:?}: {}", bad, e.msg);
    }
    for ok in [&["..."][..], &[".Gitignore"][..], &[".JJconfig", ".GIT.d"][..], &["GIT"][..]] {
        let new = focus_with_files(vec![entry_at(ok, "fine")]);
        assert!(j::repo::validate_repo(&mut i, &new).is_ok(), "{:?}", ok);
    }
}

#[test]
fn unchanged_mutable_commits_keep_stored_path_components() {
    // git stores `.jj/x`, so a fetched branch can hold it; the branch is
    // mutable and labelled, so it cannot be abandoned. Refusing its names
    // stopped every persist in the clone, though persisting neither checks
    // it out nor writes any name of it jj has not already stored
    let root = commit(ROOT_ID, "", &[], vec![]);
    let feature_files = j::domain::lazy_files("kfffffff", vec![entry(".jj/x", "fixture")]);
    let with_files = |id: &str, msg: &str, labels: &[&str], files: Value| {
        let c = commit(id, msg, labels, vec![]);
        let Value::Record(m) = c else { unreachable!() };
        let mut m = (*m).clone();
        m.insert("files".to_string(), files);
        Value::Record(Rc::new(m))
    };
    let feature = with_files("kfffffff", "feat", &["feature"], feature_files.clone());
    // `focus` beside `other`, both children of the root
    let beside = |focus: Value, other: Value, kids: Vec<Value>| {
        Value::record(&[
            ("children", Value::list(kids)),
            (
                "context",
                Value::list(vec![Value::record(&[
                    ("left", Value::list(vec![])),
                    ("parent", root.clone()),
                    ("right", Value::list(vec![subtree(other, vec![])])),
                ])]),
            ),
            ("root", focus),
        ])
    };
    let a = commit("kaaaaaaa", "", &[], vec![]);
    let old = beside(a.clone(), feature.clone(), vec![]);
    let mut i = make_interp(backend_with(&["kaaaaaaa", "kfffffff"]));
    *i.old_repo.borrow_mut() = Some(old.clone());
    let mut check = |new: &Value| j::repo::validate_repo(&mut i, new).map(|_| ()).map_err(|c| c.msg);
    // left as stored, or rewritten with its stored files: accepted
    assert_eq!(check(&old), Ok(()));
    let described = beside(commit("kaaaaaaa", "hello", &[], vec![]), feature.clone(), vec![]);
    assert_eq!(check(&described), Ok(()));
    let renamed = with_files("kfffffff", "renamed", &["feature"], feature_files.clone());
    assert_eq!(check(&beside(a.clone(), renamed, vec![])), Ok(()));
    let relisted = commit("kfffffff", "feat", &["feature"], vec![entry(".jj/x", "fixture")]);
    assert_eq!(check(&beside(a.clone(), relisted, vec![])), Ok(()));
    // checked out, written with new files, or new: refused
    let refused = |r: Result<(), String>| {
        let e = r.unwrap_err();
        assert!(e.contains("path component"), "{}", e);
    };
    refused(check(&beside(feature.clone(), a.clone(), vec![])));
    let edited = commit(
        "kfffffff",
        "feat",
        &["feature"],
        vec![entry(".jj/x", "fixture"), entry("b", "more")],
    );
    refused(check(&beside(a.clone(), edited, vec![])));
    let copy = commit("knnnnnnn", "", &[], vec![entry(".jj/x", "fixture")]);
    refused(check(&beside(a.clone(), feature.clone(), vec![subtree(copy, vec![])])));
}

#[test]
fn labels_cannot_change() {
    let old = two_commit_repo(&["feat"]);
    let stripped = {
        let root = commit("kaaaaaaa", "a", &[], vec![]);
        Value::record(&[
            ("children", old.field("children").unwrap()),
            ("context", old.field("context").unwrap()),
            ("root", root),
        ])
    };
    let e = validate_with(&old, &stripped).unwrap_err();
    assert!(e.contains("labels"), "{}", e);
    // adding a label also fails
    let added = two_commit_repo(&["feat", "extra"]);
    let e = validate_with(&old, &added).unwrap_err();
    assert!(e.contains("labels"), "{}", e);
}

#[test]
fn focus_must_be_mutable() {
    // focus on the root commit
    let root_only = {
        let old = two_commit_repo(&[]);
        let ctx = old.field("context").unwrap();
        let frame = ctx.as_list().unwrap()[0].clone();
        Value::record(&[
            ("children", Value::list(vec![subtree(old.field("root").unwrap(), vec![])])),
            ("context", Value::list(vec![])),
            ("root", frame.field("parent").unwrap()),
        ])
    };
    let old = two_commit_repo(&[]);
    let e = validate_with(&old, &root_only).unwrap_err();
    assert!(e.contains("mutable"), "{}", e);
}

#[test]
fn immutable_message_change_rejected() {
    // root -> a (merge, immutable) -> b (focus); editing a's message is refused
    let root = commit(ROOT_ID, "", &[], vec![]);
    let a = commit("kaaaaaaa", "a", &[], vec![]);
    let b_commit = commit("kbbbbbbb", "b", &[], vec![]);
    let frame_root = Value::record(&[
        ("left", Value::list(vec![])),
        ("parent", root.clone()),
        ("right", Value::list(vec![])),
    ]);
    let frame_a = Value::record(&[
        ("left", Value::list(vec![])),
        ("parent", a.clone()),
        ("right", Value::list(vec![])),
    ]);
    let old = Value::record(&[
        ("children", Value::list(vec![])),
        ("context", Value::list(vec![frame_a, frame_root])),
        ("root", b_commit.clone()),
    ]);
    let mut b = backend_with(&["kaaaaaaa", "kbbbbbbb"]);
    b.parents.insert(
        "kaaaaaaa".to_string(),
        vec![ROOT_ID.to_string(), "kyyyyyyy".to_string()],
    );
    let cfg = config::load_config(CONFIG).unwrap();
    let mut i = Interp::new(Rc::new(b), cfg.shapes.clone(), Env::empty());
    config::eval_config(&mut i, &cfg).unwrap();
    *i.old_repo.borrow_mut() = Some(old.clone());
    // unchanged passes
    assert!(j::repo::validate_repo(&mut i, &old).is_ok());
    // changing a's message is refused
    let changed = {
        let frame_a2 = Value::record(&[
            ("left", Value::list(vec![])),
            ("parent", commit("kaaaaaaa", "different", &[], vec![])),
            ("right", Value::list(vec![])),
        ]);
        let frame_root2 = Value::record(&[
            ("left", Value::list(vec![])),
            ("parent", root.clone()),
            ("right", Value::list(vec![])),
        ]);
        Value::record(&[
            ("children", Value::list(vec![])),
            ("context", Value::list(vec![frame_a2, frame_root2])),
            ("root", b_commit),
        ])
    };
    let e = j::repo::validate_repo(&mut i, &changed).unwrap_err();
    assert!(e.msg.contains("immutable"), "{}", e.msg);
}

#[test]
fn immutable_files_in_another_order_are_unchanged() {
    // a snapshot stands for a tree, which has no order (§7.3): `contract m`
    // lists the entries m matches first, so moving nothing into an immutable
    // parent reordered its files, and the list comparison refused that as
    // "immutable (files changed)" — in `validate` and in persistence alike
    let root = commit(ROOT_ID, "", &[], vec![]);
    let with_a = |files: Vec<Value>| {
        let frame = |parent: Value| {
            Value::record(&[
                ("left", Value::list(vec![])),
                ("parent", parent),
                ("right", Value::list(vec![])),
            ])
        };
        Value::record(&[
            ("children", Value::list(vec![])),
            (
                "context",
                Value::list(vec![
                    frame(commit("kaaaaaaa", "a", &[], files)),
                    frame(root.clone()),
                ]),
            ),
            ("root", commit("kbbbbbbb", "b", &[], vec![])),
        ])
    };
    // a is a merge, so immutable
    let mut b = backend_with(&["kaaaaaaa", "kbbbbbbb"]);
    b.parents.insert(
        "kaaaaaaa".to_string(),
        vec![ROOT_ID.to_string(), "kyyyyyyy".to_string()],
    );
    let mut i = make_interp(b);
    let old = with_a(vec![entry("a.txt", "1"), entry("h/x", "2")]);
    *i.old_repo.borrow_mut() = Some(old.clone());
    let reordered = with_a(vec![entry("h/x", "2"), entry("a.txt", "1")]);
    assert!(
        j::repo::validate_repo(&mut i, &reordered).is_ok(),
        "{:?}",
        j::repo::validate_repo(&mut i, &reordered).err().map(|c| c.msg)
    );
    // a real change in any order is still refused
    for files in [
        vec![entry("h/x", "3"), entry("a.txt", "1")],
        vec![entry("a.txt", "1"), entry("h/x", "3")],
        vec![entry("h/x", "2")],
        vec![entry("h/x", "2"), entry("a.txt", "1"), entry("b.txt", "1")],
        vec![entry("a.txt", "1"), entry("b.txt", "1"), entry("h/x", "2")],
        vec![entry("h/y", "2"), entry("a.txt", "1")],
    ] {
        let e = j::repo::validate_repo(&mut i, &with_a(files)).unwrap_err();
        assert!(e.msg.contains("immutable (files changed)"), "{}", e.msg);
    }
}

#[test]
fn snapshots_compare_path_by_path() {
    let eq = |a: Vec<Value>, b: Vec<Value>| {
        j::repo::snapshot_eq(&Value::list(a), &Value::list(b)).unwrap()
    };
    let (a, b, c) = (entry("a", "1"), entry("b/c", "2"), entry("b.txt", "3"));
    assert!(eq(vec![a.clone(), b.clone(), c.clone()], vec![a.clone(), b.clone(), c.clone()]));
    assert!(eq(vec![a.clone(), b.clone(), c.clone()], vec![c.clone(), a.clone(), b.clone()]));
    assert!(eq(vec![c.clone(), b.clone(), a.clone()], vec![b.clone(), c.clone(), a.clone()]));
    assert!(eq(vec![], vec![]));
    // the same paths with other content, fewer or more paths, other paths
    let c2 = entry("b.txt", "4");
    assert!(!eq(vec![a.clone(), b.clone(), c.clone()], vec![a.clone(), b.clone(), c2.clone()]));
    assert!(!eq(vec![a.clone(), b.clone(), c.clone()], vec![c2, b.clone(), a.clone()]));
    assert!(!eq(vec![a.clone(), b.clone()], vec![a.clone(), b.clone(), c.clone()]));
    assert!(!eq(vec![a.clone(), c.clone()], vec![a.clone(), b.clone()]));
    assert!(!eq(vec![c.clone(), a.clone()], vec![a.clone(), b.clone()]));
    // a path listed twice is not a tree, and matches nothing else
    assert!(!eq(vec![a.clone(), a.clone()], vec![a.clone(), c.clone()]));
    assert!(!eq(vec![c.clone(), a.clone()], vec![a.clone(), a.clone()]));
}

#[test]
fn root_must_stay_on_top() {
    // covered implicitly by navigation invariants; validate shape only here
    let old = two_commit_repo(&[]);
    let not_repo = Value::record(&[("root", old.field("root").unwrap())]);
    let mut i = make_interp(backend_with(&[]));
    *i.old_repo.borrow_mut() = Some(old.clone());
    let e = j::repo::validate_repo(&mut i, &not_repo).unwrap_err();
    assert!(e.msg.contains("not a Repo"), "{}", e.msg);
}

// ------------------------------------------------------------------
// config validation (§6.2)
// ------------------------------------------------------------------

fn cfg_err(src: &str) -> String {
    match config::load_config(src) {
        Ok(_) => panic!("config unexpectedly valid"),
        Err(e) => e.message(),
    }
}

const MINIMAL: &str = r#"
user = { name = "A B", email = "a@b" }
immutable = \_ -> []
tree = \_ -> ""
labelled = \_ _ -> []
"#;

#[test]
fn minimal_config_valid() {
    config::load_config(MINIMAL).expect("minimal config");
}

#[test]
fn missing_required_definitions() {
    let e = cfg_err("immutable = \\_ -> []\ntree = \\_ -> \"\"\nlabelled = \\_ _ -> []\n");
    assert!(e.contains("user"), "{}", e);
    let e = cfg_err("user = { name = \"A\", email = \"a\" }\ntree = \\_ -> \"\"\nlabelled = \\_ _ -> []\n");
    assert!(e.contains("immutable"), "{}", e);
}

#[test]
fn double_definition_rejected() {
    let e = cfg_err(&format!("{}\nextra = 1\nextra = 2\n", MINIMAL));
    assert!(e.contains("twice"), "{}", e);
}

#[test]
fn builtin_redefinition_rejected() {
    let e = cfg_err(&format!("{}\nmap = 1\n", MINIMAL));
    assert!(e.contains("builtin"), "{}", e);
}

#[test]
fn local_shadowing_is_a_configuration_error() {
    // §4.2: rebinding a name of an enclosing lambda or let in config.j
    for (def, name) in [
        ("sh = \\x -> \\x -> x", "x"),
        ("sh = \\x x -> x", "x"),
        ("sh = let a = 1 in let a = 2 in a", "a"),
        ("sh = \\repo -> let repo = 1 in repo", "repo"),
    ] {
        let e = cfg_err(&format!("{}\n{}\n", MINIMAL, def));
        assert!(e.starts_with("line 7:"), "{}: {}", def, e);
        assert!(e.contains(&format!("`{}` is already bound", name)), "{}: {}", def, e);
    }
}

#[test]
fn config_binders_may_reuse_the_configs_own_top_level_names() {
    // §4.2's one exception, which the reference config's `\files ->` and
    // `\old new repo ->` rely on. Within the binder's scope the name means
    // the binder, for load order (§4.1) as for lookup: were `loop` in `sh`
    // the definition, `sh` and `loop` would be a cycle
    let i = eval_cfg(&format!(
        "{}\nfocus = 7\nparam = (\\focus -> focus) 1\nbound = let focus = 2 in focus\n\
         sh = let loop = 3 in loop\nloop = sh\n",
        MINIMAL
    ))
    .expect("config loads");
    for (name, want) in [("focus", 7), ("param", 1), ("bound", 2), ("loop", 3)] {
        let v = i.globals.lookup(name).expect(name);
        assert!(j::value::value_eq(&v, &Value::int(want)).unwrap(), "{}", name);
    }
    // a builtin's name stays reserved there, declared or not (§6.2)
    let e = cfg_err(&format!("{}\nsh = \\map -> map\n", MINIMAL));
    assert!(e.contains("`map` is already bound"), "{}", e);
    // and an expression may not bind a top-level name at all
    let cfg = config::load_config(&format!("{}\nfocus = 7\n", MINIMAL)).unwrap();
    let outer = Rc::new(cfg.global_names.clone());
    for src in ["\\focus -> focus", "let focus = 1 in focus"] {
        match j::parse::parse_expr(src, outer.clone()) {
            Ok(_) => panic!("{:?} must not parse", src),
            Err(e) => assert!(e.msg.contains("`focus` is already bound"), "{}: {}", src, e.msg),
        }
    }
}

#[test]
fn a_let_binding_not_yet_evaluated_is_not_its_top_level_namesake() {
    // §4.2: a config.j binder reusing a top-level name means the binder
    // throughout its scope. Here `a` applies `f`, whose body needs the
    // binding `bb`, which needs `a`: a crash, as on the command line. The
    // lookup used to pass the block's unfilled `bb` by and find the
    // definition, so `zz` was 1001
    for block in [
        "let a = f 1; f = \\x -> bb + x; bb = a in a",
        // no value cycle, but `bb` reaches back to `a` and is not evaluated
        // before the rest (§4.1): a crash is allowed, the definition is not
        "let a = g 1; g = \\x -> h x; h = \\x -> bb + x; bb = c.v; c = { v = 5, back = \\y -> a } in a",
    ] {
        let src = format!("{}\n(+) : Int -> Int -> Int\nbb = 1000\nzz = {}\n", MINIMAL, block);
        match eval_cfg(&src) {
            Ok(_) => panic!("{}: config loads", block),
            Err(e) => assert!(e.contains("unbound name `bb`"), "{}: {}", block, e),
        }
    }
    // evaluated before it is used, the binding is what the name means
    let i = eval_cfg(&format!(
        "{}\nf = \\x -> 1000 + x\nzz = let a = f 1; f = \\x -> x in a\n",
        MINIMAL
    ))
    .expect("config loads");
    let v = i.globals.lookup("zz").expect("zz");
    assert!(j::value::value_eq(&v, &Value::int(1)).unwrap());
}

#[test]
fn label_literal_depends_on_a_let_binding_named_labelled() {
    // §4.11: `%main` is `labelled "main"`, so in the scope of a config.j
    // binding `labelled` (§4.2) it is ordered after that binding (§4.1) and
    // applies it, wherever the block lists it. `a` used to be evaluated
    // first, before the binding: the top-level `labelled` gave `[]` where
    // `labelled "main"` in its place gave "main"
    for block in [
        "let a = %main; labelled = \\n r -> n in a 0",
        "let a = labelled \"main\"; labelled = \\n r -> n in a 0",
        "let labelled = \\n r -> n; a = %main in a 0",
        "let b = { f = (let c = 1 in %main) }; labelled = \\n r -> n in b.f 0",
        "let a = \\u -> %main; labelled = \\n r -> n in a 1 0",
    ] {
        let i = eval_cfg(&format!("{}\nzz = {}\n", MINIMAL, block))
            .unwrap_or_else(|e| panic!("{}: {}", block, e));
        let v = i.globals.lookup("zz").expect("zz");
        assert!(j::value::value_eq(&v, &Value::text("main")).unwrap(), "{}", block);
    }
}

#[test]
fn double_typedecl_rejected() {
    let e = cfg_err(&format!("{}\nPath = [Text]\nPath = [Text]\n", MINIMAL));
    assert!(e.contains("twice"), "{}", e);
}

#[test]
fn unknown_builtin_declaration_rejected() {
    let e = cfg_err(&format!("{}\nnotABuiltin : Int\n", MINIMAL));
    assert!(e.contains("not a builtin"), "{}", e);
}

#[test]
fn two_signatures_rejected() {
    let e = cfg_err(&format!("{}\nf : Int\nf : Text\nf = 1\n", MINIMAL));
    assert!(e.contains("two signatures"), "{}", e);
}

#[test]
fn signature_must_precede_definition() {
    let e = cfg_err(&format!(
        "{}\nother = 1\nf : Int\ng = 2\nf = 3\n",
        MINIMAL
    ));
    assert!(e.contains("immediately"), "{}", e);
}

#[test]
fn no_item_between_signature_and_definition() {
    // §3.4, §6.2.2: a typedecl or another signature is an item too; either
    // used to detach the signature silently and still attach it as `f`'s
    // contract
    for between in ["T = Int\n", "g : Int\ng = 1\n"] {
        let e = cfg_err(&format!("{}\nf : Int\n{}f = 2\n", MINIMAL, between));
        assert!(
            e.contains("the signature for `f` is not immediately followed"),
            "{:?}: {}",
            between,
            e
        );
    }
    // a builtin's signature has no definition to precede
    config::load_config(&format!(
        "{}\nlength : [a] -> Int\nT = Int\nnull : [a] -> Bool\nf : Int\nf = 1\n",
        MINIMAL
    ))
    .expect("builtin signatures may be followed by any item");
}

/// Load and evaluate a config over an empty backend.
fn eval_cfg(src: &str) -> Result<Interp, String> {
    let cfg = config::load_config(src).map_err(|e| e.message())?;
    let mut i = Interp::new(Rc::new(MemBackend::new()), cfg.shapes.clone(), Env::empty());
    config::eval_config(&mut i, &cfg).map_err(|c| c.msg)?;
    Ok(i)
}

#[test]
fn label_literal_depends_on_labelled() {
    // §4.11: `%main` is `labelled "main"`, so a definition using it outside a
    // lambda is evaluated after `labelled` (§4.1). A name sorting before
    // `labelled` used to be evaluated first and fail the whole config with
    // "`labelled` is not defined".
    for def in ["base = %main", "choices = [%main %master]", "zbase = %main"] {
        let i = eval_cfg(&format!("{}\n{}\n", MINIMAL, def))
            .unwrap_or_else(|e| panic!("{}: {}", def, e));
        let name = def.split(' ').next().unwrap();
        assert!(i.globals.lookup(name).is_some(), "{}", def);
    }
}

#[test]
fn load_order_follows_lambda_bodies() {
    // `opts` applies `mk` at load and `mk`'s body needs `lanes`, a reference
    // inside a lambda that §4.1 does not count: `opts` used to be evaluated
    // as soon as `mk` was, before `lanes` (which waits for `names`)
    let i = eval_cfg(&format!(
        "{}\nnames = 1\nlanes = [names]\nmk = \\d -> {{ lanes = lanes }}\nopts = mk 1\n",
        MINIMAL
    ))
    .expect("config loads");
    let opts = i.globals.lookup("opts").expect("opts");
    let want = Value::record(&[("lanes", Value::list(vec![Value::int(1)]))]);
    assert!(j::value::value_eq(&opts, &want).unwrap());
    // mutually referring definitions keep the order that references outside
    // lambdas give them: `b` applies `a`, whose body needs `y`, and `y` refers
    // back to `b`; `b` waits for `z`, so `y` is bound by then
    let i = eval_cfg(&format!(
        "{}\na = \\_ -> y\nb = a z\ny = \\_ -> b\nz = 1\n",
        MINIMAL
    ))
    .expect("config loads");
    assert!(i.globals.lookup("b").is_some());
    // among definitions that reach each other, one that refers to none of
    // them outside a lambda and only stores its lambdas that do cannot apply
    // their functions, so it goes first: `aout` applies `afmt`, whose body
    // needs the record, and the record's stored lambda refers back to
    // `aout`. The record used to be evaluated first only when its name
    // sorted first, and otherwise the config failed with an unbound name
    for (env, def) in [
        ("zenv", "{ lanes = 3, again = \\_ -> aout }"),
        ("aaenv", "{ lanes = 3, again = \\_ -> aout }"),
        ("zenv", "let n = 3 in { lanes = n, again = [(\\_ -> aout)] }"),
        ("zenv", "if true then base { lanes = 3, again = \\_ -> aout } else base"),
    ] {
        let src = format!(
            "{}\nbase = {{ lanes = 0, again = \\_ -> 0 }}\nafmt = \\x -> {env}.lanes\n\
             aout = afmt 1\n{env} = {def}\n",
            MINIMAL
        );
        let i = eval_cfg(&src).unwrap_or_else(|e| panic!("{}: {}", src, e));
        let aout = i.globals.lookup("aout").expect("aout");
        assert!(j::value::value_eq(&aout, &Value::int(3)).unwrap(), "{}", src);
    }
    for names in ["names", "anames"] {
        let src = format!(
            "{}\n{names} = 3\nenv = {{ lanes = {names}, header = \\_ -> aout }}\n\
             render = \\x -> env.lanes\naout = render 1\n",
            MINIMAL
        );
        let i = eval_cfg(&src).unwrap_or_else(|e| panic!("{}: {}", src, e));
        let aout = i.globals.lookup("aout").expect("aout");
        assert!(j::value::value_eq(&aout, &Value::int(3)).unwrap(), "{}", src);
    }
    // a lambda that is applied, not stored, leaves its definition in the
    // first order: `bm` reaches `ax` through the lambda it passes, so it must
    // still follow `ax`, which the first order puts before it
    let i = eval_cfg(&format!(
        "{}\naa = \\_ -> bm\nax = {{ f = aa }}\nbm = (\\f -> f 0) (\\_ -> ax)\n",
        MINIMAL
    ))
    .expect("config loads");
    assert!(i.globals.lookup("bm").is_some());
}

#[test]
fn user_must_be_wellformed() {
    let cfg = config::load_config("user = { name = \"\", email = \"a\" }\nimmutable = \\_ -> []\ntree = \\_ -> \"\"\nlabelled = \\_ _ -> []\n").unwrap();
    let mut i = Interp::new(Rc::new(MemBackend::new()), cfg.shapes.clone(), Env::empty());
    let r = config::eval_config(&mut i, &cfg);
    match r {
        Ok(_) => panic!("should fail"),
        Err(c) => assert!(c.msg.contains("user"), "{}", c.msg),
    }
    // immutable must be a function
    let cfg = config::load_config("user = { name = \"A\", email = \"a\" }\nimmutable = []\ntree = \\_ -> \"\"\nlabelled = \\_ _ -> []\n").unwrap();
    let mut i = Interp::new(Rc::new(MemBackend::new()), cfg.shapes.clone(), Env::empty());
    let r = config::eval_config(&mut i, &cfg);
    assert!(r.is_err());
}

#[test]
fn dependency_cycles_rejected() {
    let e = cfg_err(&format!("{}\na = b\nb = a\n", MINIMAL));
    assert!(e.contains("cycle"), "{}", e);
    // through lambdas is fine
    config::load_config(&format!("{}\nf = \\x -> g x\ng = \\x -> f x\n", MINIMAL))
        .expect("lambda cycles are allowed");
}

#[test]
fn id_literals_in_config_resolve() {
    let src = format!("{}\ntarget = @kqqqqqqq\n", MINIMAL);
    let cfg = config::load_config(&src).unwrap();
    let mut b = MemBackend::new();
    b.metas.insert(
        "kqqqqqqq".to_string(),
        j::domain::MetaInfo {
            hash: "h".into(),
            author: "a".into(),
            email: "e".into(),
            time: 0,
        },
    );
    let i = Interp::new(Rc::new(b), cfg.shapes.clone(), Env::empty());
    let mut resolved = config::load_config(&src).unwrap();
    let failures = config::resolve_config_ids(&mut resolved, &i);
    assert!(failures.is_empty());
    // resolving rewrites the definition: evaluating it must not hit the
    // literal again (it used to crash with `internal: unresolved id literal`,
    // and only when the definition was finally used)
    let mut i3 = Interp::new(
        Rc::new(MemBackend::new()),
        resolved.shapes.clone(),
        Env::empty(),
    );
    config::eval_config(&mut i3, &resolved).expect("resolved config evaluates");
    match i3.globals.lookup("target") {
        Some(Value::Id(id)) => assert_eq!(&*id, "kqqqqqqq"),
        other => panic!("target is {:?}", other),
    }
    // unknown id fails, naming the definition
    let i2 = Interp::new(Rc::new(MemBackend::new()), cfg.shapes.clone(), Env::empty());
    let mut unresolved = config::load_config(&src).unwrap();
    let failures = config::resolve_config_ids(&mut unresolved, &i2);
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].0, "target");
}

#[test]
fn parse_errors_reported_with_line() {
    let e = cfg_err("x = \n");
    assert!(e.contains("line"), "{}", e);
}

#[test]
fn eval_user_only() {
    let cfg = config::load_config(MINIMAL).unwrap();
    let mut i = Interp::new(Rc::new(MemBackend::new()), cfg.shapes.clone(), Env::empty());
    let (name, email) = config::eval_user_only(&mut i, &cfg).unwrap();
    assert_eq!(name, "A B");
    assert_eq!(email, "a@b");
}
