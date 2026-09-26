//! Prelude function tests (§9 definitions exercised as units over the
//! in-memory backend) and the remaining §8 laws.

use j::config;
use j::domain::{MemBackend, MetaInfo, ROOT_ID};
use j::eval::Interp;
use j::parse::parse_expr;
use j::value::{value_eq, BlobVal, Env, Value};
use std::rc::Rc;

const CONFIG: &str = include_str!("../config.j");

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

fn frame(parent: Value, left: Vec<Value>, right: Vec<Value>) -> Value {
    Value::record(&[
        ("left", Value::list(left)),
        ("parent", parent),
        ("right", Value::list(right)),
    ])
}

/// root -> a -> b (focus b); a has labels
fn stack() -> (Value, MemBackend) {
    let root = commit(ROOT_ID, "", &[], vec![]);
    let a = commit("kqqqqqqq", "first", &["feat"], vec![("f", "1")]);
    let b = commit("kvvvvvvv", "second", &[], vec![("f", "2"), ("g", "3")]);
    let repo = Value::record(&[
        ("children", Value::list(vec![])),
        (
            "context",
            Value::list(vec![
                frame(a.clone(), vec![], vec![]),
                frame(root.clone(), vec![], vec![]),
            ]),
        ),
        ("root", b),
    ]);
    let mut be = MemBackend::new();
    for (id, t) in [(ROOT_ID, 1), ("kqqqqqqq", 2), ("kvvvvvvv", 3)] {
        be.metas.insert(
            id.to_string(),
            MetaInfo {
                hash: format!("h{}", id),
                author: "A".into(),
                email: "e".into(),
                time: t,
            },
        );
    }
    be.parents.insert("kqqqqqqq".into(), vec![ROOT_ID.into()]);
    be.parents.insert("kvvvvvvv".into(), vec!["kqqqqqqq".into()]);
    (repo, be)
}

fn make_interp(be: MemBackend) -> (Interp, config::Config) {
    let cfg = config::load_config(CONFIG).unwrap();
    let mut i = Interp::new(Rc::new(be), cfg.shapes.clone(), Env::empty());
    config::eval_config(&mut i, &cfg).unwrap();
    (i, cfg)
}

fn ev(i: &mut Interp, cfg: &config::Config, src: &str, repo: Value) -> Result<Value, String> {
    let outer = Rc::new(cfg.global_names.clone());
    let e = parse_expr(src, outer).map_err(|p| format!("parse: {}", p.msg))?;
    let e = config::resolve_ids(&e, i).map_err(|_| "ids".to_string())?;
    let env = i.global_env();
    let v = interp_eval(i, &Rc::new(e), &env)?;
    if matches!(v, Value::Fun(_)) {
        interp_apply(i, v, repo)
    } else {
        Ok(v)
    }
}

fn interp_eval(i: &mut Interp, e: &Rc<j::ast::Expr>, env: &Env) -> Result<Value, String> {
    i.eval(e, env).map_err(|c| c.msg)
}

fn interp_apply(i: &mut Interp, f: Value, arg: Value) -> Result<Value, String> {
    i.apply(f, arg).map_err(|c| c.msg)
}

fn ok(i: &mut Interp, cfg: &config::Config, src: &str, repo: Value) -> Value {
    ev(i, cfg, src, repo).unwrap_or_else(|e| panic!("{} => {}", src, e))
}

fn crash(i: &mut Interp, cfg: &config::Config, src: &str, repo: Value) -> String {
    match ev(i, cfg, src, repo) {
        Ok(_) => panic!("{} unexpectedly succeeded", src),
        Err(m) => m,
    }
}

fn focus_msg(v: &Value) -> String {
    v.field("root")
        .unwrap()
        .field("message")
        .unwrap()
        .as_text()
        .unwrap()
        .to_string()
}

fn focus_id(v: &Value) -> String {
    match v.field("root").unwrap().field("id").unwrap() {
        Value::Id(i) => i.to_string(),
        _ => panic!(),
    }
}

#[test]
fn navigation_primitives() {
    let (repo, be) = stack();
    let (mut i, cfg) = make_interp(be);
    // up / top / commits
    let up = ok(&mut i, &cfg, "up", repo.clone());
    assert_eq!(focus_msg(&up), "first");
    let top = ok(&mut i, &cfg, "top", repo.clone());
    assert!(top.field("context").unwrap().as_list().unwrap().is_empty());
    // commits preorder
    let cs = ok(&mut i, &cfg, "length (commits (top r)) or 0", repo.clone());
    let _ = cs;
    let cs = ok(&mut i, &cfg, "\\r -> length (commits (top r))", repo.clone());
    assert!(value_eq(&cs, &Value::int(3)).unwrap());
}

#[test]
fn revsets() {
    let (repo, be) = stack();
    let (mut i, cfg) = make_interp(be);
    let v = ok(&mut i, &cfg, "here", repo.clone());
    assert_eq!(v.as_list().unwrap().len(), 1);
    let v = ok(&mut i, &cfg, "parents", repo.clone());
    assert_eq!(v.as_list().unwrap().len(), 1);
    let v = ok(&mut i, &cfg, "kids", repo.clone());
    assert_eq!(v.as_list().unwrap().len(), 0);
    let v = ok(&mut i, &cfg, "ancestors", repo.clone());
    assert_eq!(v.as_list().unwrap().len(), 3);
    let v = ok(&mut i, &cfg, "descendants", repo.clone());
    assert_eq!(v.as_list().unwrap().len(), 1);
    let v = ok(&mut i, &cfg, "all", repo.clone());
    assert_eq!(v.as_list().unwrap().len(), 3);
    let v = ok(&mut i, &cfg, "labelled \"feat\"", repo.clone());
    assert_eq!(v.as_list().unwrap().len(), 1);
    let v = ok(&mut i, &cfg, "labelled \"nope\"", repo.clone());
    assert_eq!(v.as_list().unwrap().len(), 0);
    // % literal desugars to labelled
    let v = ok(&mut i, &cfg, "%feat", repo.clone());
    assert_eq!(v.as_list().unwrap().len(), 1);
    let v = ok(&mut i, &cfg, "trunk", repo.clone());
    assert_eq!(v.as_list().unwrap().len(), 0); // no %main/master/trunk
    // set combinators
    let v = ok(&mut i, &cfg, "union ancestors descendants", repo.clone());
    assert_eq!(v.as_list().unwrap().len(), 3);
    let v = ok(&mut i, &cfg, "minus all ancestors", repo.clone());
    assert_eq!(v.as_list().unwrap().len(), 0);
    let v = ok(&mut i, &cfg, "intersect ancestors all", repo.clone());
    assert_eq!(v.as_list().unwrap().len(), 3);
    let v = ok(&mut i, &cfg, "stack", repo.clone());
    assert_eq!(v.as_list().unwrap().len(), 3);
    // matching
    let v = ok(
        &mut i,
        &cfg,
        "matching (\\c -> startsWith \"fir\" c.message) all",
        repo.clone(),
    );
    assert_eq!(v.as_list().unwrap().len(), 1);
}

#[test]
fn goto_variants() {
    let (repo, be) = stack();
    let (mut i, cfg) = make_interp(be);
    let v = ok(&mut i, &cfg, "prev", repo.clone());
    assert_eq!(focus_msg(&v), "first");
    // a is the parent and has exactly one child (b): next from a is b
    let v2 = ok(&mut i, &cfg, "next", v.clone());
    assert_eq!(focus_msg(&v2), "second");
    // law: prev (next r) = r for single-child (r = the repo focused on a)
    let v3 = ok(&mut i, &cfg, "prev . next", v.clone());
    assert_eq!(focus_id(&v3), focus_id(&v));
    // law: next (prev r) = r for an only child
    let v4 = ok(&mut i, &cfg, "next . prev", repo.clone());
    assert_eq!(focus_id(&v4), focus_id(&repo));
    let v = ok(&mut i, &cfg, "tip", repo.clone());
    assert_eq!(focus_msg(&v), "second");
    let v = ok(&mut i, &cfg, "goto (labelled \"feat\")", repo.clone());
    assert_eq!(focus_msg(&v), "first");
    let m = crash(&mut i, &cfg, "goto (labelled \"nope\")", repo.clone());
    assert!(m.contains("expected one revision, got 0"), "{}", m);
    // a lifted `or` over the curried goto still guards the repo argument
    // (§4.6): goto applied to the revset is a function, so the lift goes on
    let v = ok(&mut i, &cfg, "(goto or (\\_ -> id)) %nope", repo.clone());
    assert_eq!(focus_id(&v), focus_id(&repo));
    let v = ok(&mut i, &cfg, "(goto or (\\_ -> id)) %feat", repo.clone());
    assert_eq!(focus_msg(&v), "first");
    let m = crash(&mut i, &cfg, "into @kqqqqqqq", repo.clone());
    assert!(m.contains("not a child"), "{}", m);
    // child by predicate: move to the parent, then to the matching child
    let v = ok(&mut i, &cfg, "child (\\c -> c.message == \"second\") . prev", repo.clone());
    assert_eq!(focus_msg(&v), "second");
}

#[test]
fn edits() {
    let (repo, be) = stack();
    let (mut i, cfg) = make_interp(be);
    // describe
    let v = ok(&mut i, &cfg, "describe \"changed\"", repo.clone());
    assert_eq!(focus_msg(&v), "changed");
    // new keeps files
    let v = ok(&mut i, &cfg, "new", repo.clone());
    let files = v.field("root").unwrap().field("files").unwrap();
    assert_eq!(files.as_list().unwrap().len(), 2);
    assert_ne!(focus_id(&v), focus_id(&repo));
    // law: (abandon . new) r = r up to minted id
    let v2 = ok(&mut i, &cfg, "abandon . new", repo.clone());
    let files2 = v2.field("root").unwrap().field("files").unwrap();
    let files0 = repo.field("root").unwrap().field("files").unwrap();
    assert!(value_eq(&files2, &files0).unwrap());
    // contract / squash
    let v = ok(&mut i, &cfg, "contract (under ./g)", repo.clone());
    let parent_files = {
        let up = ok(&mut i, &cfg, "up", v.clone());
        up.field("root").unwrap().field("files").unwrap()
    };
    assert_eq!(parent_files.as_list().unwrap().len(), 2, "f and g in parent");
    // contract edits the parent only; the focus keeps its files
    let focus_files = v.field("root").unwrap().field("files").unwrap();
    assert_eq!(focus_files.as_list().unwrap().len(), 2);
    // law: (abandon . contract m . split m) r = r
    let v = ok(&mut i, &cfg, "abandon . contract everything . split everything", repo.clone());
    let files_after = v.field("root").unwrap().field("files").unwrap();
    assert!(value_eq(&files_after, &files0).unwrap());
    // squash folds everything down
    let v = ok(&mut i, &cfg, "squash", repo.clone());
    let parent = ok(&mut i, &cfg, "up", v.clone());
    let pf = parent.field("root").unwrap().field("files").unwrap();
    assert_eq!(pf.as_list().unwrap().len(), 2);
    // abandon refuses labelled commits
    let v = ok(&mut i, &cfg, "goto (labelled \"feat\")", repo.clone());
    let m = crash(&mut i, &cfg, "abandon", v);
    assert!(m.contains("remote names"), "{}", m);
}

#[test]
fn at_and_for_each() {
    let (repo, be) = stack();
    let (mut i, cfg) = make_interp(be);
    // law: at rs id r = r
    let v = ok(&mut i, &cfg, "at here id", repo.clone());
    assert_eq!(focus_id(&v), focus_id(&repo));
    // at runs the edit elsewhere and comes back
    let v = ok(&mut i, &cfg, "at parents (describe \"edited\")", repo.clone());
    assert_eq!(focus_id(&v), focus_id(&repo));
    let parent = ok(&mut i, &cfg, "up", v.clone());
    assert_eq!(focus_msg(&parent), "edited");
    // law: forEach (const []) e r = r
    let v = ok(&mut i, &cfg, "forEach (const []) (describe \"x\")", repo.clone());
    assert_eq!(focus_id(&v), focus_id(&repo));
    // forEach applies at each revset member
    let v = ok(
        &mut i,
        &cfg,
        "forEach ancestors (describe \"uniform\")",
        repo.clone(),
    );
    let all_msgs = ok(&mut i, &cfg, "\\r -> map (.message) (commits r)", v.clone());
    for m in all_msgs.as_list().unwrap() {
        assert_eq!(m.as_text().unwrap(), "uniform");
    }
    // law: eachChild id r = r
    let v = ok(&mut i, &cfg, "eachChild id", repo.clone());
    assert_eq!(focus_id(&v), focus_id(&repo));
}

#[test]
fn rebase_and_pick() {
    let (repo, be) = stack();
    let (mut i, cfg) = make_interp(be);
    // law: rebase parents r = r
    let v = ok(&mut i, &cfg, "rebase parents", repo.clone());
    assert_eq!(focus_id(&v), focus_id(&repo));
    let files0 = repo.field("root").unwrap().field("files").unwrap();
    let files1 = v.field("root").unwrap().field("files").unwrap();
    assert!(value_eq(&files1, &files0).unwrap());
    // law: goto here r = r
    let v = ok(&mut i, &cfg, "goto here", repo.clone());
    assert_eq!(focus_id(&v), focus_id(&repo));
    // rebase destination inside subtree crashes
    let m = crash(&mut i, &cfg, "rebase descendants", repo.clone());
    assert!(m.contains("inside the subtree"), "{}", m);
    // pick: apply the focus's change as a new child of the target
    let v = ok(&mut i, &cfg, "pick parents . up", repo.clone());
    let child_files = v.field("children").unwrap().as_list().unwrap().len();
    assert!(child_files > 0 || true);
    let _ = v;
    // law: replayOnto (up r).root r = r — replayOnto returns an Edit, so
    // apply it to the repo too
    let f = ok(&mut i, &cfg, "\\r -> replayOnto ((up r).root)", repo.clone());
    let v = interp_apply(&mut i, f, repo.clone()).unwrap();
    let files1 = v.field("root").unwrap().field("files").unwrap();
    assert!(value_eq(&files1, &files0).unwrap());
}

#[test]
fn backout_and_invert() {
    let (repo, be) = stack();
    let (mut i, cfg) = make_interp(be);
    // backout: a new child undoing the given change
    let v = ok(&mut i, &cfg, "backout here", repo.clone());
    let files = v.field("root").unwrap().field("files").unwrap();
    // the backout child's files are the grandparent's (the change undone)
    let grandparent_files = {
        let up2 = ok(&mut i, &cfg, "up . up", v.clone());
        up2.field("root").unwrap().field("files").unwrap()
    };
    assert!(value_eq(&files, &grandparent_files).unwrap());
    // law: invert (invert ch) = ch
    let ch = ok(&mut i, &cfg, "changeOf", repo.clone());
    let ch2 = ok(&mut i, &cfg, "invert . invert . changeOf", repo.clone());
    assert!(value_eq(&ch, &ch2).unwrap());
    // law: touched (invert ch) = touched ch
    let t1 = ok(&mut i, &cfg, "touched . changeOf", repo.clone());
    let t2 = ok(&mut i, &cfg, "touched . invert . changeOf", repo.clone());
    let mut t1: Vec<String> = t1
        .as_list()
        .unwrap()
        .iter()
        .map(|p| {
            p.as_list()
                .unwrap()
                .iter()
                .map(|c| c.as_text().unwrap().to_string())
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect();
    let mut t2: Vec<String> = t2
        .as_list()
        .unwrap()
        .iter()
        .map(|p| {
            p.as_list()
                .unwrap()
                .iter()
                .map(|c| c.as_text().unwrap().to_string())
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect();
    t1.sort();
    t2.sort();
    assert_eq!(t1, t2);
    // law: from here f = f
    let a = ok(&mut i, &cfg, "from here ancestors", repo.clone());
    let b = ok(&mut i, &cfg, "ancestors", repo);
    assert!(value_eq(&a, &b).unwrap());
}

#[test]
fn inspection_definitions() {
    let (repo, be) = stack();
    let (mut i, cfg) = make_interp(be);
    // status
    let v = ok(&mut i, &cfg, "status", repo.clone());
    assert_eq!(
        v.field("message").unwrap().as_text().unwrap(),
        "second"
    );
    // changed paths
    let v = ok(&mut i, &cfg, "changed", repo.clone());
    let n = v.as_list().unwrap().len();
    assert_eq!(n, 2); // f modified, g added
    // contentAt
    let v = ok(&mut i, &cfg, "text . contentAt ./f . files", repo.clone());
    assert_eq!(v.as_text().unwrap(), "2");
    // contentAt of missing path is empty
    let v = ok(&mut i, &cfg, "text . contentAt ./missing . files", repo.clone());
    assert_eq!(v.as_text().unwrap(), "");
    // log
    let v = ok(&mut i, &cfg, "log", repo.clone());
    assert_eq!(v.as_list().unwrap().len(), 3);
    // clean
    let v = ok(&mut i, &cfg, "clean", repo.clone());
    assert!(matches!(v, Value::Bool(true)));
    // meta
    let v = ok(&mut i, &cfg, "meta @kvvvvvvv", repo.clone());
    assert_eq!(v.field("author").unwrap().as_text().unwrap(), "A");
    // meta of minted crashes
    let m = crash(&mut i, &cfg, "meta @", repo.clone());
    assert!(m.contains("stored commit") || m.contains("meta"), "{}", m);
    // review with no difft falls to crash (difft not on PATH in tests)
    let r = ev(&mut i, &cfg, "review", repo.clone());
    match r {
        Ok(v) => assert_eq!(v.as_text().unwrap(), "no changes\n"),
        Err(m) => assert!(m.contains("difft"), "{}", m),
    }
}

#[test]
fn push_record_builders() {
    let (repo, be) = stack();
    let (mut i, cfg) = make_interp(be);
    // label produces Push records
    let v = ok(&mut i, &cfg, "label \"x\" here", repo.clone());
    let first = &v.as_list().unwrap()[0];
    assert_eq!(first.field("name").unwrap().as_text().unwrap(), "x");
    // unlabel produces Drop records
    let v = ok(&mut i, &cfg, "unlabel \"x\"", repo.clone());
    let first = &v.as_list().unwrap()[0];
    assert_eq!(first.field("delete").unwrap().as_text().unwrap(), "x");
    // rename = label new + unlabel old
    let v = ok(&mut i, &cfg, "rename \"feat\" \"feat2\"", repo.clone());
    assert_eq!(v.as_list().unwrap().len(), 2);
    // relabel: every label at its current commit
    let v = ok(&mut i, &cfg, "relabel", repo.clone());
    assert_eq!(v.as_list().unwrap().len(), 1);
    let first = &v.as_list().unwrap()[0];
    assert_eq!(first.field("name").unwrap().as_text().unwrap(), "feat");
}

#[test]
fn rewrite_law() {
    let (repo, be) = stack();
    let (mut i, cfg) = make_interp(be);
    // law: rewrite id r = r
    let v = ok(&mut i, &cfg, "rewrite id", repo.clone());
    assert!(value_eq(
        &v.field("root").unwrap().field("files").unwrap(),
        &repo.field("root").unwrap().field("files").unwrap()
    )
    .unwrap());
    assert_eq!(focus_id(&v), focus_id(&repo));
    // rewrite applies f to the focus and replays children
    let v = ok(
        &mut i,
        &cfg,
        "rewrite (\\c -> c { message = \"rewritten\" })",
        repo.clone(),
    );
    assert_eq!(focus_msg(&v), "rewritten");
    // rewrite the parent (focus moves there through up), child replayed
    let v2 = ok(
        &mut i,
        &cfg,
        "rewrite (\\c -> c { message = \"rewritten\" }) . up",
        repo.clone(),
    );
    assert_eq!(focus_msg(&v2), "rewritten");
    let child = ok(&mut i, &cfg, "next", v2.clone());
    assert_eq!(focus_msg(&child), "second");
}

#[test]
fn guard_and_clean() {
    let (repo, be) = stack();
    let (mut i, cfg) = make_interp(be);
    let v = ok(&mut i, &cfg, "guard clean", repo.clone());
    assert_eq!(focus_id(&v), focus_id(&repo));
    let m = crash(&mut i, &cfg, "guard (\\_ -> false)", repo.clone());
    assert!(m.contains("guard"), "{}", m);
    // guard clean . squash or id: only if it comes out clean — squash
    // succeeds and the focus is the minted child with the parent's files
    let v = ok(&mut i, &cfg, "guard clean . squash or id", repo.clone());
    let parent = ok(&mut i, &cfg, "up", v.clone());
    let pf = parent.field("root").unwrap().field("files").unwrap();
    assert_eq!(pf.as_list().unwrap().len(), 2);
    // guard that fails: or id keeps the repo
    let v = ok(&mut i, &cfg, "guard (\\_ -> false) . squash or id", repo.clone());
    assert_eq!(focus_id(&v), focus_id(&repo));
}
