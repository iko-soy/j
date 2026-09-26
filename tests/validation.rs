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
    ] {
        let new = focus_with_files(vec![entry_at(bad, "pwn")]);
        let e = validate_with(&old, &new).unwrap_err();
        assert!(e.contains("path component"), "{:?}: {}", bad, e);
    }
    // names that merely resemble them are ordinary files; jj's snapshot
    // tracks `.GIT` on a case-sensitive filesystem, so refusing it would
    // refuse the working directory itself
    for ok in [&["..."][..], &[".gitignore"][..], &[".jjconfig", ".git.d"][..], &[".GIT", "x"][..]] {
        let new = focus_with_files(vec![entry_at(ok, "fine")]);
        assert!(validate_with(&old, &new).is_ok(), "{:?}", ok);
    }
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
