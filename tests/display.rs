//! Display tests (§5.1 examples) over the in-memory backend.

use j::config;
use j::domain::{MemBackend, MetaInfo, ROOT_ID};
use j::eval::Interp;
use j::parse::parse_expr;
use j::value::{BlobContent, BlobKind, BlobVal, Crash, Env, LazyBlob, ThunkVal, Value};
use std::rc::Rc;

const CONFIG: &str = include_str!("../config.j");

fn meta(id: &str, author: &str, time: i64) -> (String, MetaInfo) {
    (
        id.to_string(),
        MetaInfo {
            hash: format!("h{}", id),
            author: author.into(),
            email: "a@x".into(),
            time,
        },
    )
}

fn commit(id: &str, msg: &str, labels: &[&str], files: Vec<(&str, &str)>) -> Value {
    let files = Value::list(
        files
            .into_iter()
            .map(|(p, c)| {
                Value::record(&[
                    ("content", BlobVal::text_blob(c)),
                    ("path", Value::list(p.split('/').map(Value::text).collect())),
                ])
            })
            .collect(),
    );
    Value::record(&[
        ("files", files),
        ("message", Value::text(msg)),
        (
            "labels",
            Value::list(labels.iter().map(|l| Value::text(*l)).collect()),
        ),
        ("id", Value::Id(Rc::new(id.to_string()))),
    ])
}

/// a linear repo root -> a -> b (focus on b)
fn sample_repo() -> (Value, MemBackend) {
    let root = commit(ROOT_ID, "", &[], vec![]);
    let a = commit("kpqxaaaa", "add parser", &["main"], vec![("src/parser.rs", "fn parse() {}\n")]);
    let b = commit(
        "wqztbbbb",
        "wip",
        &["feature"],
        vec![
            ("src/parser.rs", "fn parse() { todo!() }\n"),
            ("src/lexer.rs", "fn lex() {}\n"),
        ],
    );
    let frame_a = Value::record(&[
        ("left", Value::list(vec![])),
        ("parent", a.clone()),
        ("right", Value::list(vec![])),
    ]);
    let frame_root = Value::record(&[
        ("left", Value::list(vec![])),
        ("parent", root.clone()),
        ("right", Value::list(vec![])),
    ]);
    let repo = Value::record(&[
        ("children", Value::list(vec![])),
        ("context", Value::list(vec![frame_a, frame_root])),
        ("root", b),
    ]);
    let mut be = MemBackend::new();
    for (id, m) in [
        meta(ROOT_ID, "Root", 1_700_000_000),
        meta("kpqxaaaa", "Montelot", 1_700_000_100),
        meta("wqztbbbb", "Montelot", 1_700_000_200),
    ] {
        be.metas.insert(id, m);
    }
    be.parents.insert("kpqxaaaa".into(), vec![ROOT_ID.into()]);
    be.parents.insert("wqztbbbb".into(), vec!["kpqxaaaa".into()]);
    (repo, be)
}

fn make_interp(be: MemBackend) -> (Interp, config::Config) {
    let cfg = config::load_config(CONFIG).unwrap();
    let mut i = Interp::new(Rc::new(be), cfg.shapes.clone(), Env::empty());
    config::eval_config(&mut i, &cfg).unwrap();
    (i, cfg)
}

fn eval_and_display(interp: &mut Interp, cfg: &config::Config, src: &str, repo: Value) -> String {
    let outer = Rc::new(cfg.global_names.clone());
    let e = parse_expr(src, outer).unwrap();
    let e = config::resolve_ids(&e, interp).unwrap();
    *interp.old_repo.borrow_mut() = Some(repo.clone());
    let env = interp.global_env();
    let v = interp.eval(&Rc::new(e), &env).unwrap();
    let v = if matches!(v, Value::Fun(_)) {
        interp.apply(v, repo).unwrap()
    } else {
        v
    };
    j::render::display(interp, &v, false).unwrap()
}

#[test]
fn status_example() {
    let (repo, be) = sample_repo();
    let (mut i, cfg) = make_interp(be);
    let out = eval_and_display(&mut i, &cfg, "status", repo);
    assert!(out.contains("src/lexer.rs"), "{}", out);
    assert!(out.contains("src/parser.rs"), "{}", out);
    assert!(out.contains("feature"), "{}", out);
    assert!(out.contains("wip"), "{}", out);
    assert!(out.contains("wqzt"), "{}", out);
}

#[test]
fn log_example() {
    let (repo, be) = sample_repo();
    let (mut i, cfg) = make_interp(be);
    let out = eval_and_display(&mut i, &cfg, "log", repo);
    assert!(out.contains("add parser"), "{}", out);
    assert!(out.contains("wip"), "{}", out);
    assert!(out.contains("✓"), "{}", out);
}

#[test]
fn files_example() {
    let (repo, be) = sample_repo();
    let (mut i, cfg) = make_interp(be);
    let out = eval_and_display(&mut i, &cfg, "files", repo);
    assert!(out.contains("src/lexer.rs"), "{}", out);
    assert!(out.contains("B"), "{}", out); // sizes
}

#[test]
fn clean_example() {
    let (repo, be) = sample_repo();
    let (mut i, cfg) = make_interp(be);
    let out = eval_and_display(&mut i, &cfg, "clean", repo);
    assert_eq!(out.trim(), "true");
}

#[test]
fn tree_example() {
    let (repo, be) = sample_repo();
    let (mut i, cfg) = make_interp(be);
    let out = eval_and_display(&mut i, &cfg, "tree", repo);
    // the root here is a single-child anchor, so it is dropped (no ⌂ row)
    assert!(!out.contains("⌂"), "{}", out);
    assert!(out.contains("add parser"), "{}", out);
    assert!(out.contains("wip"), "{}", out);
    assert!(out.contains("▶"), "{}", out);
    assert!(out.contains("◉"), "{}", out);
    assert!(out.contains("feature"), "{}", out);
    assert!(out.contains("main"), "{}", out);
}

#[test]
fn here_and_kids() {
    let (repo, be) = sample_repo();
    let (mut i, cfg) = make_interp(be);
    let out = eval_and_display(&mut i, &cfg, "here", repo.clone());
    assert!(out.contains("wqzt"), "{}", out);
    let out = eval_and_display(&mut i, &cfg, "kids", repo);
    assert!(out.contains("none"), "{}", out);
}

#[test]
fn map_path_files() {
    let (repo, be) = sample_repo();
    let (mut i, cfg) = make_interp(be);
    let out = eval_and_display(&mut i, &cfg, "map (.path) . files", repo);
    assert!(out.contains("src/lexer.rs"), "{}", out);
    assert!(out.contains("src/parser.rs"), "{}", out);
}

#[test]
fn text_of_blob() {
    let (repo, be) = sample_repo();
    let (mut i, cfg) = make_interp(be);
    let out = eval_and_display(&mut i, &cfg, "text . contentAt ./src/lexer.rs . files", repo);
    assert_eq!(out, "fn lex() {}\n");
}

#[test]
fn dry_run_tree_after_squash() {
    let (repo, be) = sample_repo();
    // strip labels so abandon is allowed
    let strip = |c: Value| -> Value {
        Value::record(&[
            ("files", c.field("files").unwrap()),
            ("id", c.field("id").unwrap()),
            ("labels", Value::list(vec![])),
            ("message", c.field("message").unwrap()),
        ])
    };
    let root = strip(repo.field("root").unwrap());
    let ctx: Vec<Value> = repo
        .field("context")
        .unwrap()
        .as_list()
        .unwrap()
        .iter()
        .map(|f| {
            Value::record(&[
                ("left", f.field("left").unwrap()),
                ("parent", strip(f.field("parent").unwrap())),
                ("right", f.field("right").unwrap()),
            ])
        })
        .collect();
    let repo = Value::record(&[
        ("children", repo.field("children").unwrap()),
        ("context", Value::list(ctx)),
        ("root", root),
    ]);
    let (mut i, cfg) = make_interp(be);
    // tree . squash is a Text: shows the result, persists nothing
    // squash folds the focus ("wip") into its parent; the parent's message
    // survives and the tree renders the result without persisting
    let out = eval_and_display(&mut i, &cfg, "tree . squash", repo);
    assert!(out.contains("add parser"), "{}", out);
    assert!(!out.contains("wip
"), "{}", out);
    // after a squash the focus is a new (minted) commit
    assert!(out.contains("▶"), "{}", out);
}

#[test]
fn show_of_focus() {
    let (repo, be) = sample_repo();
    let (mut i, cfg) = make_interp(be);
    let out = eval_and_display(&mut i, &cfg, "show . focus", repo);
    assert!(out.contains("message = \"wip\""), "{}", out);
    assert!(out.contains("@wqztbbbb"), "{}", out);
}

#[test]
fn functions_display_as_written() {
    // §5.1: a function displays as its name, its lambda source, or `f arg …`
    let (repo, be) = sample_repo();
    let (mut i, cfg) = make_interp(be);
    let out = eval_and_display(
        &mut i,
        &cfg,
        "{ f = (+ 1), g = \\x -> [(\\y -> y) x], h = (\\x y -> x) 1, k = map (1 +) }",
        repo,
    );
    let rows: Vec<&str> = out.lines().map(str::trim_end).collect();
    assert_eq!(
        rows,
        ["f  (+ 1)", "g  \\x -> [(\\y -> y) x]", "h  (\\x y -> x) 1", "k  map (1 +)"],
        "{}",
        out
    );
}

#[test]
fn conflicts_table() {
    // a repo where the focus has an unresolved file
    let (repo, be) = sample_repo();
    // add a conflicted file to the focus
    let root = repo.field("root").unwrap();
    let mut files: Vec<Value> = root.field("files").unwrap().as_list().unwrap().to_vec();
    files.push(Value::record(&[
        (
            "content",
            Value::Blob(Rc::new(j::value::BlobVal {
                kind: j::value::BlobKind::Regular,
                content: j::value::BlobContent::Conflict(vec![
                    Rc::new(b"ours\n".to_vec()),
                    Rc::new(b"base\n".to_vec()),
                    Rc::new(b"theirs\n".to_vec()),
                ]),
            })),
        ),
        ("path", Value::list(vec![Value::text("src/conflicted.rs")])),
    ]));
    let root2 = Value::record(&[
        ("files", Value::list(files)),
        ("message", root.field("message").unwrap()),
        ("labels", root.field("labels").unwrap()),
        ("id", root.field("id").unwrap()),
    ]);
    let repo2 = Value::record(&[
        ("children", repo.field("children").unwrap()),
        ("context", repo.field("context").unwrap()),
        ("root", root2),
    ]);
    let (mut i, cfg) = make_interp(be);
    let out = eval_and_display(&mut i, &cfg, "conflicts", repo2);
    assert!(out.contains("wqzt"), "{}", out);
    assert!(out.contains("⊗"), "{}", out);
    assert!(out.contains("wip"), "{}", out);
}

#[test]
fn entry_list_aligns_by_display_width() {
    // the size column was padded with `{:n$}`, which counts characters; a
    // wide (East Asian) path is two columns per character and pushed it out
    let root = commit(ROOT_ID, "", &[], vec![]);
    let focus = commit(
        "kpqxaaaa",
        "widths",
        &[],
        vec![
            ("ascii-name.txt", "x\n"),
            ("日本語.txt", "y\n"),
            ("café.txt", "z\n"),
        ],
    );
    let frame_root = Value::record(&[
        ("left", Value::list(vec![])),
        ("parent", root),
        ("right", Value::list(vec![])),
    ]);
    let repo = Value::record(&[
        ("children", Value::list(vec![])),
        ("context", Value::list(vec![frame_root])),
        ("root", focus),
    ]);
    let mut be = MemBackend::new();
    for (id, m) in [
        meta(ROOT_ID, "Root", 1_700_000_000),
        meta("kpqxaaaa", "M", 1_700_000_100),
    ] {
        be.metas.insert(id, m);
    }
    let (mut i, cfg) = make_interp(be);
    let out = eval_and_display(&mut i, &cfg, "files", repo);
    // every size cell must begin at the same display column
    let starts: Vec<usize> = out
        .lines()
        .filter(|l| l.ends_with(" B"))
        .map(|l| j::render::width(&l[..l.rfind("  ").unwrap()]))
        .collect();
    assert_eq!(starts.len(), 3, "{}", out);
    assert!(
        starts.windows(2).all(|w| w[0] == w[1]),
        "size column not aligned: {:?}\n{}",
        starts,
        out
    );
}

// ----------------------------------------------------------------------
// The jj backend differs from the default in-memory one in two ways that
// changed behaviour and were invisible to the suite: a commit's `files` is a
// thunk until asked for, and `has_conflict`/`is_empty` are answered from tree
// ids rather than by walking files. `MemBackend::answering` plus `lazy_files`
// reproduce both.
// ----------------------------------------------------------------------

fn lazy_commit(id: &str, msg: &str, files: Vec<(&str, &str)>) -> Value {
    let entries: Vec<Value> = files
        .into_iter()
        .map(|(p, c)| {
            Value::record(&[
                ("content", BlobVal::text_blob(c)),
                ("path", Value::list(p.split('/').map(Value::text).collect())),
            ])
        })
        .collect();
    Value::record(&[
        ("files", j::domain::lazy_files(entries)),
        ("id", Value::Id(Rc::new(id.to_string()))),
        ("labels", Value::list(vec![])),
        ("message", Value::text(msg)),
    ])
}

/// A linear repo of `n` commits above the root whose file lists are lazy,
/// focused on the last. It has to be long enough that a commit in the middle
/// has a parent nothing asked the files of — that is the case where emptiness
/// had nothing to fall back on.
fn lazy_repo_chain(n: usize) -> (Value, MemBackend) {
    let ids: Vec<String> = (0..n).map(|k| format!("kpqx{:04}", k)).collect();
    let mut be = MemBackend::answering();
    be.metas.insert(ROOT_ID.to_string(), meta(ROOT_ID, "Root", 1_700_000_000).1);
    be.parents.insert(ROOT_ID.to_string(), vec![]);
    let mut frames: Vec<Value> = vec![Value::record(&[
        ("left", Value::list(vec![])),
        ("parent", lazy_commit(ROOT_ID, "", vec![])),
        ("right", Value::list(vec![])),
    ])];
    let mut files: Vec<(String, String)> = Vec::new();
    let mut focus = None;
    for (k, id) in ids.iter().enumerate() {
        files.push((format!("src/f{}.rs", k), format!("fn f{}() {{}}\n", k)));
        let as_refs: Vec<(&str, &str)> =
            files.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
        let c = lazy_commit(id, &format!("commit {}", k), as_refs);
        be.metas.insert(id.clone(), meta(id, "M", 1_700_000_100 + k as i64).1);
        let parent = if k == 0 { ROOT_ID.to_string() } else { ids[k - 1].clone() };
        be.parents.insert(id.clone(), vec![parent]);
        be.empties.insert(id.clone(), false);
        if k + 1 == n {
            focus = Some(c);
        } else {
            frames.push(Value::record(&[
                ("left", Value::list(vec![])),
                ("parent", c),
                ("right", Value::list(vec![])),
            ]));
        }
    }
    frames.reverse();
    let repo = Value::record(&[
        ("children", Value::list(vec![])),
        ("context", Value::list(frames)),
        ("root", focus.unwrap()),
    ]);
    (repo, be)
}

fn lazy_repo() -> (Value, MemBackend) {
    lazy_repo_chain(2)
}

#[test]
fn lazy_file_lists_stay_invisible_to_the_language() {
    // `extract` walked past an unforced `files` thunk, so it found nothing on
    // a real repository while finding everything in memory
    let (repo, be) = lazy_repo();
    let (mut i, cfg) = make_interp(be);
    for (src, want) in [
        ("length (extract Entry repo)", "3"),
        ("length (extract Blob repo)", "3"),
        ("length (extract Commit repo)", "3"),
        ("length (files repo)", "2"),
    ] {
        let out = eval_and_display(
            &mut i,
            &cfg,
            &format!("(\\repo -> {})", src),
            repo.clone(),
        );
        assert_eq!(out.trim(), want, "{}", src);
    }
}

#[test]
fn tree_emptiness_uses_the_backend_when_files_are_lazy() {
    // `empty` fell back to a file count that is 0 whenever the list was never
    // materialised, so a backend that answers `is_empty` (and therefore never
    // forces the list) marked every commit empty
    // six commits: the middle ones have a parent whose files nothing needs
    let (repo, be) = lazy_repo_chain(6);
    let (mut i, cfg) = make_interp(be);
    let out = eval_and_display(&mut i, &cfg, "tree", repo);
    let rows: Vec<&str> = out.lines().filter(|l| l.contains("commit ")).collect();
    assert_eq!(rows.len(), 6, "{}", out);
    for row in rows {
        assert!(!row.contains('◌'), "wrongly marked empty: {:?}\n{}", row, out);
    }
}

// ----------------------------------------------------------------------
// A lazy part the store cannot read (a missing or corrupt object) must fail
// the same way every time it is used (§7.2). Forcing parked a stand-in —
// `false` for a `files` thunk, no bytes for a blob — while it ran and left it
// there when the read failed, so once a crash was caught by `or`, or
// swallowed by rendering, the next look at the same value saw the stand-in.
// ----------------------------------------------------------------------

const UNREADABLE: &str = "cannot read an object: gone";

/// root -> one commit (the focus) whose `files` is `files`; a fresh value per
/// call, since a failed force is exactly the state these tests look at
fn unreadable_repo(files: Value) -> (Value, MemBackend) {
    let focus = Value::record(&[
        ("files", files),
        ("id", Value::Id(Rc::new("kpqxaaaa".to_string()))),
        ("labels", Value::list(vec![])),
        (
            "message",
            Value::text("a message long enough that show breaks the commit over lines"),
        ),
    ]);
    let repo = Value::record(&[
        ("children", Value::list(vec![])),
        (
            "context",
            Value::list(vec![Value::record(&[
                ("left", Value::list(vec![])),
                ("parent", lazy_commit(ROOT_ID, "", vec![])),
                ("right", Value::list(vec![])),
            ])]),
        ),
        ("root", focus),
    ]);
    let mut be = MemBackend::answering();
    for (id, m) in [
        meta(ROOT_ID, "Root", 1_700_000_000),
        meta("kpqxaaaa", "M", 1_700_000_100),
    ] {
        be.metas.insert(id, m);
    }
    be.parents.insert(ROOT_ID.into(), vec![]);
    be.parents.insert("kpqxaaaa".into(), vec![ROOT_ID.into()]);
    be.empties.insert("kpqxaaaa".into(), false);
    (repo, be)
}

/// a `files` list whose tree cannot be read
fn unreadable_files() -> Value {
    Value::Thunk(Rc::new(ThunkVal::new(|| Err(Crash::new(UNREADABLE)))))
}

/// a file `f.txt` whose bytes cannot be read
fn unreadable_blob_files() -> Value {
    let blob = Value::Blob(Rc::new(BlobVal {
        kind: BlobKind::Regular,
        content: BlobContent::Lazy(Rc::new(LazyBlob::new("d93a6afb".into(), || {
            Err(Crash::new(UNREADABLE))
        }))),
    }));
    Value::list(vec![Value::record(&[
        ("content", blob),
        ("path", Value::list(vec![Value::text("f.txt")])),
    ])])
}

/// `src` applied to `repo` and displayed, or the message it crashed with
fn try_eval_and_display(
    interp: &mut Interp,
    cfg: &config::Config,
    src: &str,
    repo: Value,
) -> Result<String, String> {
    let outer = Rc::new(cfg.global_names.clone());
    let e = parse_expr(src, outer).unwrap();
    let e = config::resolve_ids(&e, interp).unwrap();
    *interp.old_repo.borrow_mut() = Some(repo.clone());
    let env = interp.global_env();
    let v = interp.eval(&Rc::new(e), &env).map_err(|c| c.msg)?;
    let v = if matches!(v, Value::Fun(_)) {
        interp.apply(v, repo).map_err(|c| c.msg)?
    } else {
        v
    };
    j::render::display(interp, &v, false).map_err(|c| c.msg)
}

#[test]
fn an_unreadable_file_list_crashes_every_time() {
    for src in [
        // caught once, then looked at again
        "length (repo.root.files or []) + length repo.root.files",
        "[(show repo.root.files or \"\") (show repo.root.files)]",
        "[(null (files repo) or true) (null (files repo))]",
    ] {
        let (repo, be) = unreadable_repo(unreadable_files());
        let (mut i, cfg) = make_interp(be);
        let got = try_eval_and_display(&mut i, &cfg, &format!("(\\repo -> {})", src), repo);
        match got {
            Err(msg) => assert!(msg.contains(UNREADABLE), "{}: crashed with {:?}", src, msg),
            Ok(out) => panic!("{}: the second look did not crash:\n{}", src, out),
        }
    }
    // `show` renders a record on one line first and again broken over lines
    // when that is too wide: the second rendering saw the stand-in `false`
    let (repo, be) = unreadable_repo(unreadable_files());
    let (mut i, cfg) = make_interp(be);
    if let Ok(out) = try_eval_and_display(&mut i, &cfg, "(\\repo -> show repo.root)", repo) {
        assert!(!out.contains("false"), "{}", out);
    }
}

#[test]
fn an_unreadable_blob_crashes_every_time() {
    for src in [
        // caught once, then looked at again
        "let b = contentAt [\"f.txt\"] (files repo) in [(text b or \"fallback\") (text b)]",
        "let b = contentAt [\"f.txt\"] (files repo) in [(text b or \"\") (show (b == blob \"\"))]",
        // the tree's size bar counts the lines of every touched file and
        // swallowed the failure, so a later `text` read the file as empty
        "[(tree repo) (text (contentAt [\"f.txt\"] (files repo)))]",
    ] {
        let (repo, be) = unreadable_repo(unreadable_blob_files());
        let (mut i, cfg) = make_interp(be);
        let got = try_eval_and_display(&mut i, &cfg, &format!("(\\repo -> {})", src), repo);
        match got {
            Err(msg) => assert!(msg.contains(UNREADABLE), "{}: crashed with {:?}", src, msg),
            Ok(out) => panic!("{}: the second look did not crash:\n{}", src, out),
        }
    }
    // the same pure expression twice in one run agrees with itself
    let (repo, be) = unreadable_repo(unreadable_blob_files());
    let (mut i, cfg) = make_interp(be);
    let src = "(\\repo -> [(show (files repo)) (show (files repo))])";
    match try_eval_and_display(&mut i, &cfg, src, repo) {
        Ok(out) => {
            let lines: Vec<&str> = out.lines().collect();
            assert_eq!(lines.len(), 2, "{}", out);
            assert_eq!(lines[0], lines[1], "{}", out);
        }
        Err(msg) => assert!(msg.contains(UNREADABLE), "crashed with {:?}", msg),
    }
}
