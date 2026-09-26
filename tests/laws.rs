//! Property tests: the laws of §8 against the in-memory backend (§10), using
//! the reference config.j unmodified.

use j::config;
use j::domain::{MemBackend, MetaInfo, ROOT_ID};
use j::eval::Interp;
use j::parse::parse_expr;
use j::value::{value_eq, Env, Value};
use proptest::prelude::*;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

const CONFIG: &str = include_str!("../config.j");

// ---------------------------------------------------------------------
// random Repo generation (bounded depth and width, random labels, random
// file edits) — §10
// ---------------------------------------------------------------------

#[derive(Debug, Clone)]
struct GCommit {
    id: String,
    message: String,
    labels: Vec<String>,
    files: Vec<(Vec<String>, String)>,
}

#[derive(Debug, Clone)]
struct GTree {
    root: GCommit,
    children: Vec<GTree>,
}

fn commit_strategy(id: String) -> impl Strategy<Value = GCommit> {
    (
        Just(id),
        "[a-z ]{0,12}",
        proptest::collection::vec("[a-z]{3,6}", 0..=1),
        proptest::collection::vec(
            (proptest::collection::vec("[a-z]{2,5}", 1..=2usize), "[a-z\n]{0,20}"),
            0..=3usize,
        ),
    )
        .prop_map(|(id, message, labels, files)| {
            // a well-formed snapshot (§7.3): unique paths, none both a file
            // and a directory
            let mut seen: BTreeSet<Vec<String>> = BTreeSet::new();
            let files: Vec<(Vec<String>, String)> = files
                .into_iter()
                .filter(|(p, _)| {
                    !seen.iter().any(|q| q.starts_with(p) || p.starts_with(q))
                        && seen.insert(p.clone())
                })
                .collect();
            GCommit {
                id,
                message,
                labels,
                files,
            }
        })
}

fn tree_strategy(ids: Rc<Vec<String>>) -> BoxedStrategy<GTree> {
    let leaf_ids = ids.clone();
    let strat = (0..ids.len()).prop_flat_map(move |_| {
        let ids = leaf_ids.clone();
        tree_rec(ids, 0)
    });
    strat.boxed()
}

fn tree_rec(ids: Rc<Vec<String>>, depth: usize) -> BoxedStrategy<GTree> {
    if ids.is_empty() {
        // shouldn't happen; produce a trivial tree with a fixed id
        return commit_strategy("kzzzzzzz".to_string())
            .prop_map(|root| GTree {
                root,
                children: vec![],
            })
            .boxed();
    }
    let head = ids[0].clone();
    let rest: Vec<String> = ids[1..].to_vec();
    let max_kids = if depth >= 2 || rest.is_empty() {
        0
    } else {
        2.min(rest.len())
    };
    (0..=max_kids)
        .prop_flat_map(move |nkids| {
            let rest = rest.clone();
            let head = head.clone();
            // split `rest` into nkids non-empty chunks
            split_strategy(rest, nkids).prop_flat_map(move |chunks| {
                let head = head.clone();
                let child_strats: Vec<BoxedStrategy<GTree>> = chunks
                    .into_iter()
                    .map(|c| tree_rec(Rc::new(c), depth + 1))
                    .collect();
                (commit_strategy(head.clone()), child_strats)
                    .prop_map(|(root, children)| GTree { root, children })
                    .boxed()
            })
        })
        .boxed()
}

fn split_strategy(
    ids: Vec<String>,
    nkids: usize,
) -> BoxedStrategy<Vec<Vec<String>>> {
    if nkids == 0 || ids.is_empty() {
        return Just(Vec::new()).boxed();
    }
    // deterministic split into nkids chunks
    let chunk = ids.len().div_ceil(nkids);
    let chunks: Vec<Vec<String>> = ids.chunks(chunk).map(|c| c.to_vec()).collect();
    Just(chunks).boxed()
}

fn gcommit_to_value(c: &GCommit) -> Value {
    let files = Value::list(
        c.files
            .iter()
            .map(|(p, content)| {
                Value::record(&[
                    ("content", j::value::BlobVal::text_blob(content)),
                    (
                        "path",
                        Value::list(p.iter().map(|s| Value::text(s.clone())).collect()),
                    ),
                ])
            })
            .collect(),
    );
    Value::record(&[
        ("files", files),
        ("message", Value::text(c.message.clone())),
        (
            "labels",
            Value::list(c.labels.iter().map(|l| Value::text(l.clone())).collect()),
        ),
        ("id", Value::Id(Rc::new(c.id.clone()))),
    ])
}

fn gtree_to_subtree(t: &GTree) -> Value {
    Value::record(&[
        ("root", gcommit_to_value(&t.root)),
        (
            "children",
            Value::list(t.children.iter().map(gtree_to_subtree).collect()),
        ),
    ])
}

/// Build a Repo value focused on the commit at `focus_path` (indices of
/// children from the top).
fn repo_value(tree: &GTree, focus_path: &[usize]) -> Value {
    // navigate to the focus, building context frames
    let mut frames: Vec<Value> = Vec::new();
    let mut cur = tree;
    for &idx in focus_path {
        let left: Vec<Value> = cur.children[..idx].iter().map(gtree_to_subtree).collect();
        let right: Vec<Value> = cur.children[idx + 1..].iter().map(gtree_to_subtree).collect();
        frames.push(Value::record(&[
            ("left", Value::list(left)),
            ("parent", gcommit_to_value(&cur.root)),
            ("right", Value::list(right)),
        ]));
        cur = &cur.children[idx];
    }
    frames.reverse();
    Value::record(&[
        ("children", Value::list(cur.children.iter().map(gtree_to_subtree).collect())),
        ("context", Value::list(frames)),
        ("root", gcommit_to_value(&cur.root)),
    ])
}

fn make_backend(tree: &GTree) -> MemBackend {
    let mut b = MemBackend::new();
    fn walk(t: &GTree, parent: Option<&str>, b: &mut MemBackend) {
        b.metas.insert(
            t.root.id.clone(),
            MetaInfo {
                hash: format!("hash-{}", t.root.id),
                author: "A U Thor".into(),
                email: "a@x".into(),
                time: 1_700_000_000,
            },
        );
        b.parents.insert(
            t.root.id.clone(),
            parent.map(|p| vec![p.to_string()]).unwrap_or_default(),
        );
        for c in &t.children {
            walk(c, Some(&t.root.id), b);
        }
    }
    walk(tree, None, &mut b);
    b
}

fn make_interp(backend: MemBackend) -> (Interp, config::Config) {
    let cfg = config::load_config(CONFIG).expect("config");
    let mut i = Interp::new(Rc::new(backend), cfg.shapes.clone(), Env::empty());
    config::eval_config(&mut i, &cfg).expect("eval config");
    (i, cfg)
}

fn eval_fn(
    interp: &mut Interp,
    cfg: &config::Config,
    src: &str,
    repo: Value,
) -> Result<Value, String> {
    let outer = Rc::new(cfg.global_names.clone());
    let e = parse_expr(src, outer).map_err(|p| format!("parse {}: {}", src, p.msg))?;
    let e = config::resolve_ids(&e, interp).map_err(|f| format!("id resolution: {:?}", f))?;
    let e = Rc::new(e);
    let env = interp.global_env();
    let v = interp.eval(&e, &env).map_err(|c| format!("eval {}: {}", src, c.msg))?;
    if matches!(v, Value::Fun(_)) {
        interp
            .apply(v, repo)
            .map_err(|c| format!("apply {}: {}", src, c.msg))
    } else {
        Ok(v)
    }
}

/// Compare two repos up to minted ids: same shape, same non-minted ids,
/// same files/messages/labels.
fn repos_match(a: &Value, b: &Value, backend_ids: &BTreeSet<String>) -> bool {
    let ca = match j::repo::all_commits(a) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let cb = match j::repo::all_commits(b) {
        Ok(c) => c,
        Err(_) => return false,
    };
    if ca.len() != cb.len() {
        return false;
    }
    let pa = j::repo::parent_map(a).unwrap_or_default();
    let pb = j::repo::parent_map(b).unwrap_or_default();
    let to_map = |cs: &[Value], ps: Vec<(String, String)>| -> HashMap<String, (String, Value, Value, Value)> {
        let pm: HashMap<String, String> = ps.into_iter().collect();
        cs.iter()
            .map(|c| {
                let id = c.field("id").unwrap();
                let id = match id {
                    Value::Id(i) => i.to_string(),
                    _ => unreachable!(),
                };
                (
                    id.clone(),
                    (
                        pm.get(&id).cloned().unwrap_or_default(),
                        c.field("files").unwrap(),
                        c.field("message").unwrap(),
                        c.field("labels").unwrap(),
                    ),
                )
            })
            .collect()
    };
    let ma = to_map(&ca, pa);
    let mb = to_map(&cb, pb);
    // match commits by (parent, files, message, labels), ignoring minted ids
    let mut unmatched: Vec<(String, Value, Value, Value)> = mb.values().cloned().collect();
    for (ida, va) in &ma {
        if backend_ids.contains(ida) {
            // stored id: must exist in b with same content
            match mb.get(ida) {
                Some(vb) => {
                    let pos = unmatched.iter().position(|x| {
                        x.0 == vb.0
                            && value_eq(&x.1, &vb.1).unwrap_or(false)
                            && value_eq(&x.2, &vb.2).unwrap_or(false)
                            && value_eq(&x.3, &vb.3).unwrap_or(false)
                    });
                    match pos {
                        Some(i) => {
                            unmatched.remove(i);
                        }
                        None => return false,
                    }
                }
                None => return false,
            }
        } else {
            // minted: match any unmatched with same content
            let pos = unmatched.iter().position(|x| {
                value_eq(&x.1, &va.1).unwrap_or(false)
                    && value_eq(&x.2, &va.2).unwrap_or(false)
                    && value_eq(&x.3, &va.3).unwrap_or(false)
            });
            match pos {
                Some(i) => {
                    unmatched.remove(i);
                }
                None => return false,
            }
        }
    }
    unmatched.is_empty()
}

fn arb_repo() -> impl Strategy<Value = (GTree, Vec<usize>, MemBackend)> {
    // a root id pool, tree, then a random focus path that exists
    proptest::collection::vec("[k-z]{8}", 1..=6usize)
        .prop_flat_map(|ids| {
            // include the root id as the top of every generated repo
            let mut all = vec![ROOT_ID.to_string()];
            all.extend(ids);
            let all = Rc::new(all);
            tree_strategy(all)
        })
        .prop_flat_map(|tree| {
            let backend = make_backend(&tree);
            // collect valid focus paths
            let mut paths: Vec<Vec<usize>> = vec![vec![]];
            fn walk(t: &GTree, prefix: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
                for (i, c) in t.children.iter().enumerate() {
                    prefix.push(i);
                    out.push(prefix.clone());
                    walk(c, prefix, out);
                    prefix.pop();
                }
            }
            walk(&tree, &mut Vec::new(), &mut paths);
            (Just(tree), proptest::sample::select(paths), Just(backend))
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn law_by_root_id((tree, focus, backend) in arb_repo()) {
        let repo = repo_value(&tree, &focus);
        let stored: BTreeSet<String> = backend.metas.keys().cloned().collect();
        let (mut i, cfg) = make_interp(backend);
        let id_expr = match repo.field("root").unwrap().field("id").unwrap() {
            Value::Id(id) => (*id).clone(),
            _ => unreachable!(),
        };
        let got = eval_fn(&mut i, &cfg, &format!("by @{}", id_expr), repo.clone());
        prop_assert!(got.is_ok(), "by failed: {:?}", got.err());
        prop_assert!(repos_match(&got.unwrap(), &repo, &stored));
    }

    #[test]
    fn law_goto_here((tree, focus, backend) in arb_repo()) {
        let repo = repo_value(&tree, &focus);
        let stored: BTreeSet<String> = backend.metas.keys().cloned().collect();
        let (mut i, cfg) = make_interp(backend);
        let got = eval_fn(&mut i, &cfg, "goto here", repo.clone());
        prop_assert!(got.is_ok(), "goto here failed: {:?}", got.err());
        prop_assert!(repos_match(&got.unwrap(), &repo, &stored));
    }

    #[test]
    fn law_rebase_parents((tree, focus, backend) in arb_repo()) {
        prop_assume!(!focus.is_empty(), "needs a parent");
        let repo = repo_value(&tree, &focus);
        let stored: BTreeSet<String> = backend.metas.keys().cloned().collect();
        let (mut i, cfg) = make_interp(backend);
        let got = eval_fn(&mut i, &cfg, "rebase parents", repo.clone());
        prop_assert!(got.is_ok(), "rebase parents failed: {:?}", got.err());
        prop_assert!(repos_match(&got.unwrap(), &repo, &stored));
    }

    #[test]
    fn law_rewrite_id((tree, focus, backend) in arb_repo()) {
        let repo = repo_value(&tree, &focus);
        let stored: BTreeSet<String> = backend.metas.keys().cloned().collect();
        let (mut i, cfg) = make_interp(backend);
        let got = eval_fn(&mut i, &cfg, "rewrite id", repo.clone());
        prop_assert!(got.is_ok(), "rewrite id failed: {:?}", got.err());
        prop_assert!(repos_match(&got.unwrap(), &repo, &stored));
    }

    #[test]
    fn law_each_child_id((tree, focus, backend) in arb_repo()) {
        let repo = repo_value(&tree, &focus);
        let stored: BTreeSet<String> = backend.metas.keys().cloned().collect();
        let (mut i, cfg) = make_interp(backend);
        let got = eval_fn(&mut i, &cfg, "eachChild id", repo.clone());
        prop_assert!(got.is_ok(), "eachChild id failed: {:?}", got.err());
        prop_assert!(repos_match(&got.unwrap(), &repo, &stored));
    }

    #[test]
    fn law_replay_identity((tree, focus, backend) in arb_repo()) {
        let repo = repo_value(&tree, &focus);
        let (mut i, cfg) = make_interp(backend);
        // replay b { from = b, to = x } = x  with b = focus files, x = files ++ one more
        let got = eval_fn(
            &mut i,
            &cfg,
            "let b = files r in let x = b ++ [{ path = [\"new\"], content = blob \"n\" }] in replay b { from = b, to = x }",
            repo.clone(),
        );
        // need a name for repo; bind r via a lambda instead
        let _ = got;
        let v = eval_fn(
            &mut i,
            &cfg,
            "\\repo -> let b = files repo in let x = b ++ [{ path = [\"new\"], content = blob \"n\" }] in replay b ({ from = b, to = x })",
            repo.clone(),
        )
        .expect("replay eval");
        let want = eval_fn(
            &mut i,
            &cfg,
            "\\r -> files r ++ [{ path = [\"new\"], content = blob \"n\" }]",
            repo,
        )
        .expect("want eval");
        // snapshots are path-sorted maps; compare as such
        let key = |snap: &Value| -> Vec<(Vec<String>, Value)> {
            let mut m: Vec<(Vec<String>, Value)> = snap
                .as_list()
                .unwrap()
                .iter()
                .map(|e| {
                    let p: Vec<String> = e
                        .field("path")
                        .unwrap()
                        .as_list()
                        .unwrap()
                        .iter()
                        .map(|c| c.as_text().unwrap().to_string())
                        .collect();
                    (p, e.field("content").unwrap())
                })
                .collect();
            m.sort_by(|a, b| a.0.cmp(&b.0));
            m
        };
        let kv = key(&v);
        let kw = key(&want);
        prop_assert_eq!(kv.len(), kw.len());
        for ((pa, ca), (pb, cb)) in kv.iter().zip(kw.iter()) {
            prop_assert_eq!(pa, pb);
            prop_assert!(value_eq(ca, cb).unwrap_or(false));
        }
    }

    #[test]
    fn law_from_here((tree, focus, backend) in arb_repo()) {
        let repo = repo_value(&tree, &focus);
        let (mut i, cfg) = make_interp(backend);
        let a = eval_fn(&mut i, &cfg, "from here ancestors", repo.clone()).expect("from here");
        let b = eval_fn(&mut i, &cfg, "ancestors", repo).expect("ancestors");
        prop_assert!(value_eq(&a, &b).unwrap_or(false));
    }

    #[test]
    fn law_at_rs_id((tree, focus, backend) in arb_repo()) {
        let repo = repo_value(&tree, &focus);
        let stored: BTreeSet<String> = backend.metas.keys().cloned().collect();
        let (mut i, cfg) = make_interp(backend);
        let got = eval_fn(&mut i, &cfg, "at here id", repo.clone());
        prop_assert!(got.is_ok(), "at here id failed: {:?}", got.err());
        prop_assert!(repos_match(&got.unwrap(), &repo, &stored));
    }

    #[test]
    fn law_for_each_const((tree, focus, backend) in arb_repo()) {
        let repo = repo_value(&tree, &focus);
        let stored: BTreeSet<String> = backend.metas.keys().cloned().collect();
        let (mut i, cfg) = make_interp(backend);
        let got = eval_fn(&mut i, &cfg, "forEach (const []) (describe \"x\")", repo.clone());
        prop_assert!(got.is_ok(), "forEach failed: {:?}", got.err());
        prop_assert!(repos_match(&got.unwrap(), &repo, &stored));
    }

    #[test]
    fn law_cached_revset_matches_direct((tree, focus, backend) in arb_repo()) {
        // `trunk` and `immutable` are memoised per Repo value, because the
        // binary asks for each more than once per run and every evaluation
        // walks the whole history. The cached answer must be the answer the
        // config gives, and must not change on a second ask.
        let repo = repo_value(&tree, &focus);
        let (mut i, cfg) = make_interp(backend);
        for name in ["trunk", "immutable"] {
            let direct = eval_fn(&mut i, &cfg, name, repo.clone());
            prop_assert!(direct.is_ok(), "{} failed: {:?}", name, direct.err());
            let direct = direct.unwrap();
            let first = i.apply_cached_revset(name, &repo);
            prop_assert!(first.is_ok(), "cached {} failed", name);
            let first = first.unwrap();
            let second = i.apply_cached_revset(name, &repo).unwrap();
            prop_assert!(value_eq(&direct, &first).unwrap_or(false), "{}: cached differs", name);
            prop_assert!(value_eq(&first, &second).unwrap_or(false), "{}: second ask differs", name);
        }
    }

    #[test]
    fn law_by_rebuilds_the_same_zipper((tree, focus, backend) in arb_repo()) {
        // `by` finds the commit first and then assembles the location in one
        // pass, instead of refocusing at every node it visits. The location it
        // produces must be the one a step-by-step walk produced: same focus,
        // same children, and a context that still climbs back to the top.
        let repo = repo_value(&tree, &focus);
        let stored: BTreeSet<String> = backend.metas.keys().cloned().collect();
        let (mut i, cfg) = make_interp(backend);
        for id in &stored {
            let got = j::repo::by_id(&repo, id).unwrap();
            prop_assert!(got.is_some(), "by {} found nothing", id);
            let got = got.unwrap();
            // the focus is the commit asked for
            match got.field("root").unwrap().field("id").unwrap() {
                Value::Id(f) => prop_assert_eq!(&*f, id),
                _ => prop_assert!(false, "focus has no id"),
            }
            // `goto` through the language agrees with it
            let viaz = eval_fn(&mut i, &cfg, &format!("by @{}", id), repo.clone());
            prop_assert!(viaz.is_ok(), "by @{} failed: {:?}", id, viaz.err());
            prop_assert!(repos_match(&viaz.unwrap(), &got, &stored));
            // climbing all the way up returns the whole history unchanged
            let up = eval_fn(&mut i, &cfg, "top", got.clone());
            prop_assert!(up.is_ok(), "top failed: {:?}", up.err());
            let from_repo = eval_fn(&mut i, &cfg, "top", repo.clone()).unwrap();
            prop_assert!(repos_match(&up.unwrap(), &from_repo, &stored));
        }
    }

    #[test]
    fn law_top_top((tree, focus, backend) in arb_repo()) {
        let repo = repo_value(&tree, &focus);
        let stored: BTreeSet<String> = backend.metas.keys().cloned().collect();
        let (mut i, cfg) = make_interp(backend);
        let a = eval_fn(&mut i, &cfg, "top (top r)", repo.clone());
        let a2 = eval_fn(&mut i, &cfg, "(\\r -> top (top r))", repo.clone()).expect("top2");
        let b = eval_fn(&mut i, &cfg, "top", repo).expect("top");
        let _ = a;
        prop_assert!(repos_match(&a2, &b, &stored));
    }

    #[test]
    fn law_concat_laws((_t, _f, backend) in arb_repo()) {
        let (mut i, cfg) = make_interp(backend);
        // concat [xs] = xs
        let a = eval_fn(&mut i, &cfg, "concat [[1 2 3]]", Value::Bool(true)).unwrap();
        prop_assert!(value_eq(&a, &Value::list(vec![Value::int(1), Value::int(2), Value::int(3)])).unwrap());
        // concat (xs ++ ys) = concat xs ++ concat ys
        let a = eval_fn(&mut i, &cfg, "concat ([[1] [2]] ++ [[3]])", Value::Bool(true)).unwrap();
        let b = eval_fn(&mut i, &cfg, "concat [[1] [2]] ++ concat [[3]]", Value::Bool(true)).unwrap();
        prop_assert!(value_eq(&a, &b).unwrap());
    }
}

/// A linear repo root -> c0 -> c1 -> ... focused on the top, built directly.
fn linear_repo(n: usize) -> (Value, Vec<String>) {
    let ids: Vec<String> = (0..n).map(|k| format!("kaaa{:04}", k)).collect();
    let mut subtree = Value::record(&[
        ("children", Value::list(vec![])),
        (
            "root",
            Value::record(&[
                ("files", Value::list(vec![])),
                ("id", Value::Id(Rc::new(ids[n - 1].clone()))),
                ("labels", Value::list(vec![])),
                ("message", Value::text(format!("commit {}", n - 1))),
            ]),
        ),
    ]);
    for k in (0..n - 1).rev() {
        subtree = Value::record(&[
            ("children", Value::list(vec![subtree])),
            (
                "root",
                Value::record(&[
                    ("files", Value::list(vec![])),
                    ("id", Value::Id(Rc::new(ids[k].clone()))),
                    ("labels", Value::list(vec![])),
                    ("message", Value::text(format!("commit {}", k))),
                ]),
            ),
        ]);
    }
    let repo = Value::record(&[
        ("children", Value::list(vec![subtree])),
        ("context", Value::list(vec![])),
        (
            "root",
            Value::record(&[
                ("files", Value::list(vec![])),
                ("id", Value::Id(Rc::new(ROOT_ID.to_string()))),
                ("labels", Value::list(vec![])),
                ("message", Value::text("")),
            ]),
        ),
    ]);
    (repo, ids)
}

#[test]
fn by_builds_the_context_innermost_first() {
    // `by` assembles the location in one pass now; the context must still run
    // from the immediate parent outwards, which is what `up` reads
    let (repo, ids) = linear_repo(8);
    for (depth, id) in ids.iter().enumerate() {
        let loc = j::repo::by_id(&repo, id).unwrap().expect("found");
        match loc.field("root").unwrap().field("id").unwrap() {
            Value::Id(f) => assert_eq!(&*f, id, "wrong focus"),
            _ => panic!("focus has no id"),
        }
        let ctx = loc.field("context").unwrap();
        let ctx = ctx.as_list().unwrap();
        assert_eq!(ctx.len(), depth + 1, "context depth for {}", id);
        // frame k names the ancestor k+1 levels up; the last is the root
        for (k, frame) in ctx.iter().enumerate() {
            let want = if depth >= k + 1 {
                ids[depth - k - 1].clone()
            } else {
                ROOT_ID.to_string()
            };
            match frame.field("parent").unwrap().field("id").unwrap() {
                Value::Id(p) => assert_eq!(&*p, &want, "frame {} of {}", k, id),
                _ => panic!("frame parent has no id"),
            }
        }
    }
}

#[test]
fn by_then_climbing_back_restores_the_history() {
    let (repo, ids) = linear_repo(8);
    let (mut i, cfg) = make_interp(MemBackend::new());
    let whole = eval_fn(&mut i, &cfg, "top", repo.clone()).unwrap();
    for id in &ids {
        let loc = j::repo::by_id(&repo, id).unwrap().expect("found");
        let back = eval_fn(&mut i, &cfg, "top", loc).unwrap();
        assert!(
            value_eq(&back, &whole).unwrap_or(false),
            "climbing back from {} changed the history",
            id
        );
    }
}

#[test]
fn repeated_navigation_is_consistent() {
    // `top_of` is memoised on the identity of the value it was asked about,
    // and `by` is applied to the same repo once per id in a revset walk. The
    // memo must not let one navigation affect the next, including across
    // repos that differ only in where they are focused.
    let (repo, ids) = linear_repo(6);
    let mut seen: Vec<Value> = Vec::new();
    for id in &ids {
        seen.push(j::repo::by_id(&repo, id).unwrap().expect("found"));
    }
    // asking again, interleaved with other repos, gives the same answers
    let (other, other_ids) = linear_repo(4);
    for (k, id) in ids.iter().enumerate() {
        let again = j::repo::by_id(&repo, id).unwrap().expect("found");
        assert!(
            value_eq(&again, &seen[k]).unwrap_or(false),
            "second lookup of {} differs",
            id
        );
        // navigating a different repo in between must not disturb it
        let o = j::repo::by_id(&other, &other_ids[0]).unwrap().expect("found");
        match o.field("root").unwrap().field("id").unwrap() {
            Value::Id(f) => assert_eq!(&*f, &other_ids[0]),
            _ => panic!("no id"),
        }
        let third = j::repo::by_id(&repo, id).unwrap().expect("found");
        assert!(
            value_eq(&third, &seen[k]).unwrap_or(false),
            "lookup of {} disturbed by another repo",
            id
        );
    }
    // and navigating from an already-refocused location still reaches the top
    for loc in &seen {
        for id in &ids {
            let from_loc = j::repo::by_id(loc, id).unwrap().expect("found");
            match from_loc.field("root").unwrap().field("id").unwrap() {
                Value::Id(f) => assert_eq!(&*f, id, "reached the wrong commit"),
                _ => panic!("no id"),
            }
        }
    }
}
