//! Tree rendering tests (§7.11): glyphs, elision, margin, rails layout.

use j::config;
use j::domain::{MemBackend, MetaInfo, ROOT_ID};
use j::eval::Interp;
use j::parse::parse_expr;
use j::value::{BlobVal, Env, Value};
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

fn subtree(root: Value, kids: Vec<Value>) -> Value {
    Value::record(&[
        ("children", Value::list(kids)),
        ("root", root),
    ])
}

/// build a repo focused on the root's child at `focus_idx`
fn repo_of(root: Value, kids: Vec<Value>, focus_idx: Option<usize>) -> Value {
    match focus_idx {
        None => Value::record(&[
            ("children", Value::list(kids)),
            ("context", Value::list(vec![])),
            ("root", root),
        ]),
        Some(i) => {
            let left: Vec<Value> = kids[..i].to_vec();
            let right: Vec<Value> = kids[i + 1..].to_vec();
            let focus = kids[i].clone();
            let frame = Value::record(&[
                ("left", Value::list(left)),
                ("parent", root),
                ("right", Value::list(right)),
            ]);
            Value::record(&[
                ("children", focus.field("children").unwrap()),
                ("context", Value::list(vec![frame])),
                ("root", focus.field("root").unwrap()),
            ])
        }
    }
}

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

fn backend_with(metas: Vec<(String, MetaInfo)>) -> MemBackend {
    let mut b = MemBackend::new();
    for (id, m) in metas {
        b.metas.insert(id, m);
    }
    b
}

fn make_interp(b: MemBackend) -> (Interp, config::Config) {
    let cfg = config::load_config(CONFIG).unwrap();
    let mut i = Interp::new(Rc::new(b), cfg.shapes.clone(), Env::empty());
    config::eval_config(&mut i, &cfg).unwrap();
    (i, cfg)
}

fn tree_text(interp: &mut Interp, cfg: &config::Config, src: &str, repo: Value) -> String {
    let outer = Rc::new(cfg.global_names.clone());
    let e = parse_expr(src, outer).unwrap();
    let e = config::resolve_ids(&e, interp).unwrap();
    let env = interp.global_env();
    let v = interp.eval(&Rc::new(e), &env).unwrap();
    let v = if matches!(v, Value::Fun(_)) {
        interp.apply(v, repo).unwrap()
    } else {
        v
    };
    v.as_text().unwrap().to_string()
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

const OPTS: &str =
    "{ detail = 2, margin = true, elide = false, icons = false, color = \"never\", lanes = 4, author = false, date = false, files = false }";

#[test]
fn tree_glyphs_basic() {
    let root = commit(ROOT_ID, "", &[], vec![]);
    let a = commit("kaaaaaaa", "work a", &[], vec![("f", "x")]);
    let b = commit("kbbbbbbb", "work b", &["main"], vec![("f", "y")]);
    // a second child of the root makes it a branch point, so the root row (and
    // its ⌂ glyph) is kept rather than dropped as a pure anchor
    let side = commit("kside000", "side", &[], vec![("g", "z")]);
    let repo = repo_of(
        root,
        vec![subtree(a, vec![subtree(b.clone(), vec![])]), subtree(side, vec![])],
        None,
    );
    // focus on root (context empty); b is a grandchild
    let (mut i, cfg) = make_interp(backend_with(vec![
        meta(ROOT_ID, "Root", now() - 100_000),
        meta("kaaaaaaa", "Ann Author", now() - 50_000),
        meta("kbbbbbbb", "Bob B", now() - 100),
        meta("kside000", "Bob B", now() - 90),
    ]));
    let text = tree_text(&mut i, &cfg, &format!("treeWith ({})", OPTS), repo);
    assert!(text.contains("⌂"), "{}", text);
    assert!(text.contains("work a"), "{}", text);
    assert!(text.contains("work b"), "{}", text);
    assert!(text.contains("main"), "{}", text);
    // margin: ages and initials
    assert!(text.contains("aa"), "{}", text);
    assert!(text.contains("bb"), "{}", text);
}

#[test]
fn tree_focus_marker_and_glyph() {
    let root = commit(ROOT_ID, "", &[], vec![]);
    let a = commit("kaaaaaaa", "a", &[], vec![("f", "x")]);
    let b = commit("kbbbbbbb", "b", &[], vec![("f", "y")]);
    let repo = repo_of(root, vec![subtree(a, vec![subtree(b, vec![])])], Some(0));
    let (mut i, cfg) = make_interp(backend_with(vec![
        meta(ROOT_ID, "Root", 1),
        meta("kaaaaaaa", "A", now() - 10),
        meta("kbbbbbbb", "B", now() - 5),
    ]));
    let text = tree_text(
        &mut i,
        &cfg,
        "treeWith ({ detail = 1, margin = false, elide = false, icons = false, color = \"never\", lanes = 4, author = false, date = false, files = false })",
        repo,
    );
    assert!(text.contains("▶"), "{}", text);
    assert!(text.contains("◉"), "{}", text);
}

#[test]
fn tree_conflict_and_empty_glyphs() {
    let root = commit(ROOT_ID, "", &[], vec![]);
    let conflicted = {
        let c = commit("kccccccc", "conflicted", &[], vec![]);
        let files = Value::list(vec![Value::record(&[
            (
                "content",
                Value::Blob(Rc::new(j::value::BlobVal {
                    kind: j::value::BlobKind::Regular,
                    content: j::value::BlobContent::Conflict(vec![
                        Some(j::value::ConflictSide::regular(b"a")),
                        Some(j::value::ConflictSide::regular(b"b")),
                        Some(j::value::ConflictSide::regular(b"c")),
                    ]),
                })),
            ),
            ("path", Value::list(vec![Value::text("f")])),
        ])]);
        Value::record(&[
            ("files", files),
            ("message", c.field("message").unwrap()),
            ("labels", c.field("labels").unwrap()),
            ("id", c.field("id").unwrap()),
        ])
    };
    let empty_child = commit("keeeeeee", "empty", &[], vec![]);
    let repo = repo_of(
        root,
        vec![subtree(conflicted, vec![]), subtree(empty_child, vec![])],
        None,
    );
    let (mut i, cfg) = make_interp(backend_with(vec![
        meta(ROOT_ID, "Root", 1),
        meta("kccccccc", "A", now()),
        meta("keeeeeee", "A", now()),
    ]));
    let text = tree_text(
        &mut i,
        &cfg,
        "treeWith ({ detail = 1, margin = false, elide = false, icons = false, color = \"never\", lanes = 4, author = false, date = false, files = false })",
        repo,
    );
    assert!(text.contains("⊗"), "{}", text);
}

#[test]
fn tree_icons_set() {
    let root = commit(ROOT_ID, "", &[], vec![]);
    let a = commit("kaaaaaaa", "a", &[], vec![("f", "x")]);
    // a second child keeps the root (a branch point) so its 🌱 icon renders
    let side = commit("kside000", "side", &[], vec![("g", "z")]);
    let repo = repo_of(root, vec![subtree(a, vec![]), subtree(side, vec![])], Some(0));
    let (mut i, cfg) = make_interp(backend_with(vec![
        meta(ROOT_ID, "Root", 1),
        meta("kaaaaaaa", "A", now()),
        meta("kside000", "B", now()),
    ]));
    let text = tree_text(
        &mut i,
        &cfg,
        "treeWith ({ detail = 1, margin = false, elide = false, icons = true, color = \"never\", lanes = 4, author = false, date = false, files = false })",
        repo,
    );
    assert!(text.contains("🌱"), "{}", text);
}

#[test]
fn tree_labels_column_only_when_present() {
    let root = commit(ROOT_ID, "", &[], vec![]);
    let a = commit("kaaaaaaa", "a", &[], vec![]);
    let b = commit("kbbbbbbb", "b", &["feat"], vec![]);
    let repo = repo_of(root, vec![subtree(a, vec![]), subtree(b, vec![])], None);
    let (mut i, cfg) = make_interp(backend_with(vec![
        meta(ROOT_ID, "R", 1),
        meta("kaaaaaaa", "A", now()),
        meta("kbbbbbbb", "B", now()),
    ]));
    let text = tree_text(
        &mut i,
        &cfg,
        "treeWith ({ detail = 1, margin = false, elide = false, icons = false, color = \"never\", lanes = 4, author = false, date = false, files = false })",
        repo,
    );
    assert!(text.contains("feat"), "{}", text);
}

#[test]
fn tree_with_missing_options_crashes() {
    let root = commit(ROOT_ID, "", &[], vec![]);
    let repo = repo_of(root, vec![], None);
    let (mut i, cfg) = make_interp(backend_with(vec![meta(ROOT_ID, "R", 1)]));
    let outer = Rc::new(cfg.global_names.clone());
    let e = parse_expr(
        "treeWith ({ detail = 1, margin = false, elide = true, icons = false })",
        outer,
    )
    .unwrap();
    let env = i.global_env();
    // missing fields crash, either when the option record is supplied or when
    // the result is applied (§7.11 "Missing fields crash")
    let r = i
        .eval(&Rc::new(e), &env)
        .and_then(|v| i.apply(v, repo));
    match r {
        Ok(_) => panic!("missing color must crash"),
        Err(c) => {
            let ok = c.msg.contains("color") || c.msg.contains("treeWith") || c.msg.contains("field");
            assert!(ok, "{}", c.msg);
        }
    }
}

#[test]
fn tree_color_never_has_no_ansi() {
    let root = commit(ROOT_ID, "", &[], vec![]);
    let a = commit("kaaaaaaa", "a", &[], vec![("f", "x")]);
    let repo = repo_of(root, vec![subtree(a, vec![])], Some(0));
    let (mut i, cfg) = make_interp(backend_with(vec![
        meta(ROOT_ID, "R", 1),
        meta("kaaaaaaa", "A", now()),
    ]));
    let text = tree_text(
        &mut i,
        &cfg,
        "treeWith ({ detail = 1, margin = false, elide = false, icons = false, color = \"never\", lanes = 4, author = false, date = false, files = false })",
        repo,
    );
    assert!(!text.contains('\x1b'), "ANSI found in color=never output");
}

#[test]
fn tree_color_always_has_ansi() {
    unsafe { std::env::remove_var("NO_COLOR") };
    let root = commit(ROOT_ID, "", &[], vec![]);
    let a = commit("kaaaaaaa", "a", &[], vec![("f", "x")]);
    let repo = repo_of(root, vec![subtree(a, vec![])], Some(0));
    let (mut i, cfg) = make_interp(backend_with(vec![
        meta(ROOT_ID, "R", 1),
        meta("kaaaaaaa", "A", now()),
    ]));
    let text = tree_text(
        &mut i,
        &cfg,
        "treeWith ({ detail = 1, margin = false, elide = false, icons = false, color = \"always\", lanes = 4, author = false, date = false, files = false })",
        repo,
    );
    assert!(text.contains('\x1b'), "no ANSI in color=always output");
}

#[test]
fn tree_minted_commits_have_blank_margin() {
    // minted commits have no metadata: no margin content (§7.11)
    let root = commit(ROOT_ID, "", &[], vec![]);
    let minted = commit("kqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq", "minted", &[], vec![]);
    let repo = repo_of(root, vec![subtree(minted, vec![])], None);
    let (mut i, cfg) = make_interp(backend_with(vec![meta(ROOT_ID, "R", 1)]));
    let text = tree_text(
        &mut i,
        &cfg,
        "treeWith ({ detail = 2, margin = true, elide = false, icons = false, color = \"never\", lanes = 4, author = false, date = false, files = false })",
        repo,
    );
    assert!(text.contains("minted"), "{}", text);
}

#[test]
fn detail_line_changed_paths() {
    let root = commit(ROOT_ID, "", &[], vec![("keep", "1"), ("mod", "1"), ("del", "1")]);
    let focus = commit(
        "kaaaaaaa",
        "changes",
        &[],
        vec![("keep", "1"), ("mod", "2"), ("new", "n")],
    );
    let repo = repo_of(root, vec![subtree(focus, vec![])], Some(0));
    let (mut i, cfg) = make_interp(backend_with(vec![
        meta(ROOT_ID, "R", now() - 10),
        meta("kaaaaaaa", "A", now()),
    ]));
    let text = tree_text(
        &mut i,
        &cfg,
        "treeWith ({ detail = 2, margin = false, elide = false, icons = false, color = \"never\", lanes = 4, author = false, date = false, files = false })",
        repo,
    );
    assert!(text.contains("+ new"), "{}", text);
    assert!(text.contains("~ mod"), "{}", text);
    assert!(text.contains("− del"), "{}", text);
}

#[test]
fn empty_ignores_the_order_of_entries() {
    // a snapshot stands for a tree, which has no order (§7.3): a commit
    // holding its parent's entries in another order — what `split m` leaves
    // when m moves nothing — changes nothing and is drawn empty
    let root = commit(ROOT_ID, "", &[], vec![]);
    let a = commit("kaaaaaaa", "parent", &[], vec![("a.txt", "1"), ("h/x", "2")]);
    let b = commit("kbbbbbbb", "reordered", &[], vec![("h/x", "2"), ("a.txt", "1")]);
    let c = commit("kccccccc", "changed", &[], vec![("h/x", "3"), ("a.txt", "1")]);
    let repo = repo_of(
        root,
        vec![subtree(a, vec![subtree(b, vec![subtree(c, vec![])])])],
        None,
    );
    let (mut i, cfg) = make_interp(backend_with(vec![
        meta(ROOT_ID, "R", 1),
        meta("kaaaaaaa", "A", now() - 30),
        meta("kbbbbbbb", "B", now() - 20),
        meta("kccccccc", "C", now() - 10),
    ]));
    let text = tree_text(
        &mut i,
        &cfg,
        "treeWith ({ detail = 1, margin = false, elide = false, icons = false, color = \"never\", lanes = 4, author = false, date = false, files = false })",
        repo,
    );
    let row = |msg: &str| {
        text.lines()
            .find(|l| l.contains(msg))
            .unwrap_or_else(|| panic!("no row for {}:\n{}", msg, text))
            .to_string()
    };
    assert!(row("reordered").contains('◌'), "{}", text);
    assert!(!row("changed").contains('◌'), "{}", text);
    assert!(!row("parent").contains('◌'), "{}", text);
}

// ----------------------------------------------------------------------
// the size bar and the files column (§7.11 column 5, specs/tree.md Step 4)
// ----------------------------------------------------------------------

/// Render a chain of commits on the root, each `(message, files)` the child
/// of the one before, with the files column on, and return each message's
/// row. The focus is the root, so at detail 1 only its child diffs.
fn chain_rows(detail: u8, chain: &[(&str, Vec<(String, String)>)]) -> Vec<(String, String)> {
    let root = commit(ROOT_ID, "", &[], vec![]);
    let mut metas = vec![meta(ROOT_ID, "R", 1)];
    let mut sub: Option<Value> = None;
    for (n, (msg, files)) in chain.iter().enumerate().rev() {
        let id = format!("k{:03}zzzzzzzzzzzz", n);
        let files: Vec<(&str, &str)> = files.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
        let c = commit(&id, msg, &[], files);
        metas.push(meta(&id, "A", now() - 1000 + n as i64));
        sub = Some(subtree(c, sub.into_iter().collect()));
    }
    let repo = repo_of(root, sub.into_iter().collect(), None);
    let (mut i, cfg) = make_interp(backend_with(metas));
    let text = tree_text(
        &mut i,
        &cfg,
        &format!(
            "treeWith ({{ detail = {}, margin = false, elide = false, icons = false, color = \"never\", lanes = 4, author = false, date = false, files = true }})",
            detail
        ),
        repo,
    );
    chain
        .iter()
        .map(|(msg, _)| {
            let row = text
                .lines()
                .find(|l| l.split_whitespace().any(|t| t == *msg))
                .unwrap_or_else(|| panic!("no row for {}:\n{}", msg, text));
            (msg.to_string(), row.to_string())
        })
        .collect()
}

fn numbered(lines: impl Iterator<Item = usize>) -> String {
    lines.map(|i| format!("line {}\n", i)).collect()
}

#[test]
fn size_bar_counts_lines_added_plus_removed() {
    // the bar is lines added plus removed against the parent, one glyph per
    // threshold 1, 10, 50, 200, 1000 (§7.11 column 5). It summed the whole
    // length of both versions of each changed file, so a one-line edit to a
    // long file drew the largest bar, and its thresholds were shifted by one
    // glyph, so `▁` never appeared and `▇` stood for both 200 and 1000.
    let mut chain: Vec<(&str, Vec<(String, String)>)> = Vec::new();
    let mut len = 0;
    for (msg, grow) in [
        ("grow0001", 1),
        ("grow0009", 9),
        ("grow0010", 10),
        ("grow0049", 49),
        ("grow0050", 50),
        ("grow0199", 199),
        ("grow0200", 200),
        ("grow0999", 999),
        ("grow1000", 1000),
    ] {
        len += grow;
        chain.push((msg, vec![("f".to_string(), numbered(0..len))]));
    }
    let edited = numbered(0..len).replace("line 1200\n", "changed\n");
    chain.push(("edit1", vec![("f".to_string(), edited.clone())]));
    let dropped = edited.replace("line 1300\n", "");
    chain.push(("drop1", vec![("f".to_string(), dropped.clone())]));
    // a path changed without a changed line still counts, so only an empty
    // commit has no bar
    let touched = vec![("e".to_string(), String::new()), ("f".to_string(), dropped)];
    chain.push(("touch", touched.clone()));
    chain.push(("nothing", touched));
    let want = [
        ("grow0001", "▁"),
        ("grow0009", "▁"),
        ("grow0010", "▂"),
        ("grow0049", "▂"),
        ("grow0050", "▃"),
        ("grow0199", "▃"),
        ("grow0200", "▅"),
        ("grow0999", "▅"),
        ("grow1000", "▇"),
        ("edit1", "▁"),
        ("drop1", "▁"),
        ("touch", "▁"),
        ("nothing", ""),
    ];
    let rows = chain_rows(2, &chain);
    for ((msg, row), (wmsg, bar)) in rows.iter().zip(want) {
        assert_eq!(msg, wmsg);
        let got: String = row.chars().filter(|c| "▁▂▃▅▇".contains(*c)).collect();
        assert_eq!(got, bar, "bar of {}: {:?}", msg, row);
    }
}

#[test]
fn files_column_counts_changed_files() {
    // `files` is the number of files changed against the parent (specs/tree.md
    // Step 4 column 9), not the size of the commit's snapshot: an empty
    // commit changes none. At detail 1 most rows skip the diff, and the
    // column must not depend on it.
    let f = |fs: &[(&str, &str)]| -> Vec<(String, String)> {
        fs.iter().map(|(p, c)| (p.to_string(), c.to_string())).collect()
    };
    let chain = vec![
        ("addthree", f(&[("a", "1\n"), ("b", "1\n"), ("c", "1\n")])),
        ("changetwo", f(&[("a", "2\n"), ("b", "1\n")])),
        ("addone", f(&[("a", "2\n"), ("b", "1\n"), ("d", "1\n")])),
        ("changenone", f(&[("a", "2\n"), ("b", "1\n"), ("d", "1\n")])),
    ];
    let want = [3, 2, 1, 0];
    for detail in [1, 2] {
        for ((msg, row), n) in chain_rows(detail, &chain).iter().zip(want) {
            let toks: Vec<&str> = row.split_whitespace().collect();
            let at = toks
                .iter()
                .position(|t| *t == "files")
                .unwrap_or_else(|| panic!("no files column for {}: {:?}", msg, row));
            assert_eq!(toks[at - 1], n.to_string(), "detail {}, {}: {:?}", detail, msg, row);
        }
    }
}

// ----------------------------------------------------------------------
// the spec's worked example (§7.11)
// ----------------------------------------------------------------------

fn conflict_commit(id: &str, msg: &str, labels: &[&str]) -> Value {
    // wip against kpqx: lexer conflicted (✖), parser modified (~), tests added (+)
    let files = Value::list(vec![
        Value::record(&[
            (
                "content",
                Value::Blob(Rc::new(j::value::BlobVal {
                    kind: j::value::BlobKind::Regular,
                    content: j::value::BlobContent::Conflict(vec![
                        Some(j::value::ConflictSide::regular(b"a")),
                        Some(j::value::ConflictSide::regular(b"b")),
                        Some(j::value::ConflictSide::regular(b"c")),
                    ]),
                })),
            ),
            (
                "path",
                Value::list(vec![Value::text("src"), Value::text("lexer.rs")]),
            ),
        ]),
        Value::record(&[
            ("content", BlobVal::text_blob("w\nw\n")),
            (
                "path",
                Value::list(vec![Value::text("src"), Value::text("parser.rs")]),
            ),
        ]),
        Value::record(&[
            ("content", BlobVal::text_blob("w\nw\n")),
            (
                "path",
                Value::list(vec![Value::text("tests"), Value::text("lexer.rs")]),
            ),
        ]),
    ]);
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

/// does this subtree's history contain the commit `id`?
fn contains_id(sub: &Value, id: &str) -> bool {
    let rid = match sub.field("root").unwrap().field("id").unwrap() {
        Value::Id(i) => i.to_string(),
        _ => return false,
    };
    if rid == id {
        return true;
    }
    sub.field("children")
        .unwrap()
        .as_list()
        .unwrap()
        .iter()
        .any(|c| contains_id(c, id))
}

fn worked_example() -> (Value, MemBackend) {
    worked_example_with_run(14)
}

/// The worked example with `n` uninteresting commits in its run.
fn worked_example_with_run(n: usize) -> (Value, MemBackend) {
    let t = now();
    let h = 3600;
    let d = 24 * h;
    let w = 7 * d;
    // root -> n uninteresting commits -> aaaa -> kpqx (trunk head)
    let root = commit(ROOT_ID, "", &[], vec![]);
    let mut kids: Vec<Value> = Vec::new();
    let ids: Vec<String> = (0..n).map(|i| format!("run{:02}xxxxxxxxxxxxx", i)).collect();

    // the base snapshot (the last run commit's files): three files of 40
    // lines. Each commit's diff against its parent sizes its bar (§7.11
    // column 5: lines added plus removed, thresholds 1, 10, 50, 200, 1000).
    let base: Vec<(String, String)> = vec![
        ("src/lexer.rs".to_string(), "b\n".repeat(40)),
        ("src/parser.rs".to_string(), "b\n".repeat(40)),
        ("tests/lexer.rs".to_string(), "b\n".repeat(40)),
    ];
    fn snap<'a>(fs: &'a [(String, String)]) -> Vec<(&'a str, &'a str)> {
        fs.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect()
    }
    // aaaa: base rewritten to 40 new lines a file, 3 × 80 = 240 lines (▅)
    let aaaa_files: Vec<(String, String)> = base
        .iter()
        .map(|(p, _)| (p.clone(), "a\n".repeat(40)))
        .collect();
    let aaaa = commit("aaaaaaaaaaaaaaaa", "add parser", &[], snap(&aaaa_files));
    // kpqx: aaaa's sources rewritten to 50 new lines and tests/lexer.rs
    // deleted, 90 + 90 + 40 = 220 lines (▅). It has no tests/lexer.rs, so
    // wqzt's tests/lexer.rs is an addition (+), as in the worked example.
    let kpqx_files: Vec<(String, String)> = vec![
        ("src/lexer.rs".to_string(), "k\n".repeat(50)),
        ("src/parser.rs".to_string(), "k\n".repeat(50)),
    ];
    let kpqx = commit("kpqxaaaaaaaaaaaa", "release 1.2", &["main"], snap(&kpqx_files));
    // mnrv: aaaa with 12 lines cut from the lexer (▂)
    let mnrv_files: Vec<(String, String)> = vec![
        ("src/lexer.rs".to_string(), "a\n".repeat(28)),
        ("src/parser.rs".to_string(), "a\n".repeat(40)),
        ("tests/lexer.rs".to_string(), "a\n".repeat(40)),
    ];
    let mnrv = commit("mnrvaaaaaaaaaaaa", "fix lexer", &[], snap(&mnrv_files));
    // wqzt: against kpqx, the lexer's 50 lines become the conflict's 8, the
    // parser's 50 become 2, and 2 are added: 58 + 52 + 2 = 112 lines (▃)
    let wqzt = conflict_commit("wqztaaaaaaaaaaaa", "wip", &["feature"]);
    // qrst: wqzt with the lexer conflict resolved to its two added sides,
    // which drops the 6 other lines of the conflict (▁)
    let qrst_files: Vec<(String, String)> = vec![
        ("src/lexer.rs".to_string(), "a\nc\n".to_string()),
        ("src/parser.rs".to_string(), "w\nw\n".to_string()),
        ("tests/lexer.rs".to_string(), "w\nw\n".to_string()),
    ];
    let qrst = commit("qrstaaaaaaaaaaaa", "spike", &[], snap(&qrst_files));
    // yxsk: wqzt with the conflict resolved to a new line, the other two files
    // rewritten to 30 lines, and 9 one-line files added: 9 + 32 + 32 + 9 = 82
    // lines (▃)
    let mut yxsk_files: Vec<(String, String)> = vec![
        ("src/lexer.rs".to_string(), "resolved2\n".to_string()),
        ("src/parser.rs".to_string(), "k\n".repeat(30)),
        ("tests/lexer.rs".to_string(), "k\n".repeat(30)),
    ];
    for i in 0..9 {
        yxsk_files.push((format!("new{}", i), "n\n".to_string()));
    }
    let yxsk = commit("yxskaaaaaaaaaaaa", "", &[], snap(&yxsk_files));
    // ptlm: no change against kpqx (empty commit)
    let ptlm = commit("ptlmaaaaaaaaaaaa", "docs", &[], snap(&kpqx_files));
    // ptlm has 3 hidden descendants
    let h1 = commit("hid1aaaaaaaaaaaa", "h1", &[], vec![("h1", "1")]);
    let h2 = commit("hid2aaaaaaaaaaaa", "h2", &[], vec![("h2", "2")]);
    let h3 = commit("hid3aaaaaaaaaaaa", "h3", &[], vec![("h3", "3")]);
    let ptlm_t = subtree(
        ptlm,
        vec![subtree(h1, vec![subtree(h2, vec![subtree(h3, vec![])])])],
    );
    let wqzt_t = subtree(wqzt, vec![subtree(qrst, vec![]), subtree(yxsk, vec![])]);
    let kpqx_t = subtree(kpqx, vec![wqzt_t, ptlm_t]);
    let aaaa_t = subtree(aaaa, vec![kpqx_t, subtree(mnrv, vec![])]);
    // the chain of 14 uninteresting commits, oldest first; the run is elided
    // so per-commit diffs don't matter, but the youngest must equal aaaa's
    // parent snapshot. Give the youngest the base and the rest empty diffs.
    let mut chain_t = aaaa_t;
    for (i, id) in ids.iter().enumerate().rev() {
        // youngest (i = 13) carries base; the others carry the same snapshot
        // minus one marker file each, keeping them uninteresting
        let fs: Vec<(String, String)> = base.clone();
        let c = commit(id, "", &[], snap(&fs));
        chain_t = subtree(c, vec![chain_t]);
        let _ = i;
    }
    kids.push(chain_t);
    // focus on wqzt: build the repo by hand (wqzt is kpqx's first child)
    let repo_top = subtree(root, kids);
    // focus the repo on wqzt via frames: top=root -> ... -> aaaa -> kpqx -> wqzt
    let mut be = backend_with(vec![
        meta(ROOT_ID, "Root", t - 30 * d),
        meta("aaaaaaaaaaaaaaaa", "Mary Ojeda", t - 3 * w),
        meta("kpqxaaaaaaaaaaaa", "Mary Ojeda", t - 2 * w),
        meta("mnrvaaaaaaaaaaaa", "Mary Ojeda", t - 1 * w),
        meta("wqztaaaaaaaaaaaa", "Mary Ojeda", t - 2 * d),
        meta("ptlmaaaaaaaaaaaa", "Aki Kato", t - 1 * d),
        meta("qrstaaaaaaaaaaaa", "Mary Ojeda", t - 5 * h),
        meta("yxskaaaaaaaaaaaa", "Mary Ojeda", t - 1 * h),
        meta("hid1aaaaaaaaaaaa", "Aki Kato", t - 20 * h),
        meta("hid2aaaaaaaaaaaa", "Aki Kato", t - 19 * h),
        meta("hid3aaaaaaaaaaaa", "Aki Kato", t - 18 * h),
    ]);
    for (i, id) in ids.iter().enumerate() {
        be.metas.insert(
            id.clone(),
            MetaInfo {
                hash: format!("h{}", id),
                author: "Mary Ojeda".into(),
                email: "a@x".into(),
                // a minute apart, all older than aaaa
                time: t - 3 * w - (60 * n as i64).max(3600) + (i as i64) * 60,
            },
        );
    }
    // parents, for the immutable closure and trunk ancestors
    let mut pmap: Vec<(String, String)> = Vec::new();
    pmap.push((ids[0].clone(), ROOT_ID.to_string()));
    for i in 1..ids.len() {
        pmap.push((ids[i].clone(), ids[i - 1].clone()));
    }
    pmap.push(("aaaaaaaaaaaaaaaa".into(), ids[ids.len() - 1].clone()));
    pmap.push(("kpqxaaaaaaaaaaaa".into(), "aaaaaaaaaaaaaaaa".into()));
    pmap.push(("mnrvaaaaaaaaaaaa".into(), "aaaaaaaaaaaaaaaa".into()));
    pmap.push(("wqztaaaaaaaaaaaa".into(), "kpqxaaaaaaaaaaaa".into()));
    pmap.push(("ptlmaaaaaaaaaaaa".into(), "kpqxaaaaaaaaaaaa".into()));
    pmap.push(("qrstaaaaaaaaaaaa".into(), "wqztaaaaaaaaaaaa".into()));
    pmap.push(("yxskaaaaaaaaaaaa".into(), "wqztaaaaaaaaaaaa".into()));
    pmap.push(("hid1aaaaaaaaaaaa".into(), "ptlmaaaaaaaaaaaa".into()));
    pmap.push(("hid2aaaaaaaaaaaa".into(), "hid1aaaaaaaaaaaa".into()));
    pmap.push(("hid3aaaaaaaaaaaa".into(), "hid2aaaaaaaaaaaa".into()));
    for (c, p) in pmap {
        be.parents.insert(c, vec![p]);
    }
    // focus wqzt: walk top -> chain -> aaaa -> kpqx -> wqzt building frames
    let focus_id = "wqztaaaaaaaaaaaa";
    let mut path: Vec<(Value, Value)> = Vec::new(); // (location, child) from the top down to kpqx
    let mut loc = repo_top.clone();
    loop {
        let rid = match loc.field("root").unwrap().field("id").unwrap() {
            Value::Id(i) => i.to_string(),
            _ => unreachable!(),
        };
        if rid == "kpqxaaaaaaaaaaaa" {
            break;
        }
        let kids = loc.field("children").unwrap().as_list().unwrap().to_vec();
        let next = kids
            .iter()
            .find(|k| {
                // the child whose subtree contains kpqx
                contains_id(k, "kpqxaaaaaaaaaaaa")
            })
            .cloned()
            .unwrap_or_else(|| panic!("no child of {} contains kpqx", rid));
        path.push((loc.clone(), next.clone()));
        loc = next;
    }
    // now loc is kpqx's subtree; focus is its first child
    let kpqx_kids = loc.field("children").unwrap().as_list().unwrap().to_vec();
    let focus_t = kpqx_kids[0].clone();
    let right = kpqx_kids[1..].to_vec();
    let mut ctx: Vec<Value> = vec![Value::record(&[
        ("left", Value::list(vec![])),
        ("parent", loc.field("root").unwrap()),
        ("right", Value::list(right)),
    ])];
    for (parent_loc, child) in path.iter().rev() {
        let kids = parent_loc.field("children").unwrap().as_list().unwrap().to_vec();
        let pos = kids
            .iter()
            .position(|k| j::value::value_eq(k, child).unwrap())
            .unwrap();
        ctx.push(Value::record(&[
            ("left", Value::list(kids[..pos].to_vec())),
            ("parent", parent_loc.field("root").unwrap()),
            ("right", Value::list(kids[pos + 1..].to_vec())),
        ]));
    }
    let _ = focus_id;
    let repo = Value::record(&[
        ("children", focus_t.field("children").unwrap()),
        ("context", Value::list(ctx)),
        ("root", focus_t.field("root").unwrap()),
    ]);
    (repo, be)
}

#[test]
fn tree_worked_example() {
    let (repo, be) = worked_example();
    let (mut i, cfg) = make_interp(be);
    let text = tree_text(
        &mut i,
        &cfg,
        "treeWith ({ detail = 2, margin = true, elide = true, icons = false, color = \"never\", lanes = 3, author = false, date = false, files = false })",
        repo,
    );
    let expected = "\
  ╎ 14
  ◆─╮    @aaaa ▅  add parser                 3w  mo
  ◆ │    @kpqx ▅  release 1.2      main      2w  mo
  │ ○    @mnrv ▂  fix lexer                  1w  mo
▶ ├─⊗    @wqzt ▃  wip              feature   2d  mo
  │ │      ✖ src/lexer.rs   ~ src/parser.rs   + tests/lexer.rs
  ╰─┼─◌  @ptlm    docs  ⋯ 3                  1d  ak
    ├─○  @qrst ▁  spike                      5h  mo
    ○    @yxsk ▃                             1h  mo
";
    // The spec pins down the rails *graph* (which structural characters appear
    // on each row, in order) and the id/size-bar/message/label/detail content.
    // Exact column widths and the gutter on the spec's first row are
    // implementation/spec-formatting details, so we compare (a) the ordered
    // sequence of structural characters per row and (b) the word tokens,
    // ignoring all whitespace.
    let struct_chars = |l: &str| -> String {
        l.chars()
            .filter(|c| "⌂◆○◉◌⊗├╰┼─╮┬│╎»".contains(*c))
            .collect()
    };
    let tokens = |l: &str| -> Vec<String> {
        l.split_whitespace().map(|t| t.to_string()).collect()
    };
    let got: Vec<&str> = text.lines().collect();
    // The legend follows the tree body after a blank line (§Legend). Split it
    // off; the body must match the spec's rails graph, and the legend must
    // explain exactly the symbols the body uses.
    let blank = got.iter().position(|l| l.trim().is_empty()).unwrap_or(got.len());
    let body = &got[..blank];
    let legend = &got[blank..];
    let want: Vec<&str> = expected.lines().collect();
    assert_eq!(body.len(), want.len(), "row count\n--- got ---\n{}\n--- want ---\n{}", text, expected);
    for (g, w) in body.iter().zip(want.iter()) {
        assert_eq!(struct_chars(g), struct_chars(w),
            "rails graph\nrow got:  {}\nrow want: {}", g, w);
        assert_eq!(tokens(g), tokens(w),
            "content\nrow got:  {}\nrow want: {}", g, w);
    }
    // legend explains the used symbols (only-used): ○ ◆ ◌ ⊗ ▶ ╎ ⋯ + ~ ✖ here
    // (no ⌂: the distant root folds into the run, §Option far root)
    let legend_text = legend.join("\n");
    for e in ["○ other commit", "◆ immutable", "◌ empty", "⊗ conflict",
              "▶ current commit", "╎ n run of n commits", "⋯ n collapsed, n hidden",
              "+ added", "~ modified", "✖ unresolved"] {
        assert!(legend_text.contains(e), "legend missing `{}`\n{}", e, legend_text);
    }
    // symbols not used must not be explained (no ●/◉/⌂/− in this tree)
    for e in ["●", "◉", "⌂ root", "− deleted"] {
        assert!(!legend_text.contains(e), "legend unexpectedly has `{}`\n{}", e, legend_text);
    }
}

// ----------------------------------------------------------------------
// text columns (§Step 4): "Columns 4–8 start at the same offset on every
// row", which the token comparison above cannot see
// ----------------------------------------------------------------------

/// The worked example's commit rows: id, message, age.
const WORKED_ROWS: [(&str, &str, &str); 7] = [
    ("@aaaa", "add parser", "3w"),
    ("@kpqx", "release 1.2", "2w"),
    ("@mnrv", "fix lexer", "1w"),
    ("@wqzt", "wip", "2d"),
    ("@ptlm", "docs", "1d"),
    ("@qrst", "spike", "5h"),
    ("@yxsk", "", "1h"),
];

/// The worked example's options at `detail`, with the data columns if `data`.
fn worked_opts(detail: u8, data: bool) -> String {
    format!(
        "{{ detail = {}, margin = true, elide = true, icons = false, color = \"never\", lanes = 3, author = {d}, date = {d}, files = {d} }}",
        detail,
        d = data
    )
}

/// The display column at which `needle` starts in `line`.
fn col_of(line: &str, needle: &str) -> usize {
    let at = line
        .find(needle)
        .unwrap_or_else(|| panic!("no {:?} in {:?}", needle, line));
    j::render::width(&line[..at])
}

/// The row of `text` that carries the id `id`.
fn row_of<'a>(text: &'a str, id: &str) -> &'a str {
    text.lines()
        .find(|l| l.contains(id))
        .unwrap_or_else(|| panic!("no row {}\n{}", id, text))
}

#[test]
fn tree_columns_line_up_on_every_row() {
    // the message moved two columns right on rows with a size bar (and a
    // row without one kept no blank slot), and the label and margin columns
    // were placed from absolute edges used as widths, ~msg_off columns too
    // far right, and per row from the row's own label width
    for detail in 0..=2 {
        for data in [false, true] {
            let (repo, be) = worked_example();
            let (mut i, cfg) = make_interp(be);
            let src = format!("treeWith ({})", worked_opts(detail, data));
            let text = tree_text(&mut i, &cfg, &src, repo);
            let ctx = format!("detail {} data {}\n{}", detail, data, text);
            // message: the same column on every row, bar or not
            let msg_col = col_of(row_of(&text, "@kpqx"), "release 1.2");
            for (id, msg, _) in WORKED_ROWS.iter().filter(|r| !r.1.is_empty()) {
                assert_eq!(col_of(row_of(&text, id), msg), msg_col, "{} message\n{}", id, ctx);
            }
            // labels: two columns past the widest message
            let lab_col = msg_col + j::render::width("release 1.2") + 2;
            assert_eq!(col_of(row_of(&text, "@kpqx"), "main"), lab_col, "main\n{}", ctx);
            assert_eq!(col_of(row_of(&text, "@wqzt"), "feature"), lab_col, "feature\n{}", ctx);
            // the data columns, then the margin: two past the widest label,
            // each at the same column on every row (authors differ in width)
            let block_col = lab_col + j::render::width("feature") + 2;
            let age_col = col_of(row_of(&text, "@aaaa"), "3w");
            for (id, _, age) in WORKED_ROWS.iter() {
                let row = row_of(&text, id);
                assert_eq!(col_of(row, age), age_col, "{} age\n{}", id, ctx);
                if data {
                    assert_eq!(col_of(row, "20"), block_col, "{} date\n{}", id, ctx);
                }
            }
            if !data {
                assert_eq!(age_col, block_col, "margin\n{}", ctx);
            }
        }
    }
}

#[test]
fn a_row_without_a_bar_keeps_the_bar_slot() {
    // specs/tree.md's worked example draws the empty `ptlm` as `@ptlm    docs`
    let (repo, be) = worked_example();
    let (mut i, cfg) = make_interp(be);
    let src = format!("treeWith ({})", worked_opts(2, false));
    let text = tree_text(&mut i, &cfg, &src, repo);
    assert!(row_of(&text, "@ptlm").contains("@ptlm    docs"), "{}", text);
    assert!(row_of(&text, "@wqzt").contains("@wqzt ▃  wip"), "{}", text);
}

// ----------------------------------------------------------------------
// the rails area (§Step 4): `2 · lanes` columns on every row, so the id
// column after it starts at one offset
// ----------------------------------------------------------------------

/// The worked example's options with `lanes` lanes and the `icons` glyph set
/// if `icons`.
fn lanes_opts(lanes: usize, icons: bool) -> String {
    format!(
        "{{ detail = 2, margin = true, elide = true, icons = {}, color = \"never\", lanes = {}, author = false, date = false, files = false }}",
        icons, lanes
    )
}

/// `line` up to display column `col`.
fn upto_col(line: &str, col: usize) -> String {
    let mut out = String::new();
    for c in line.chars() {
        if j::render::width(&out) + j::render::width(&c.to_string()) > col {
            break;
        }
        out.push(c);
    }
    out
}

/// The display column of the id on every commit row of the worked example.
fn id_cols(text: &str) -> Vec<usize> {
    WORKED_ROWS
        .iter()
        .map(|(id, _, _)| col_of(row_of(text, id), id))
        .collect()
}

#[test]
fn a_run_count_runs_on_into_the_id_column() {
    // the count was cut where the rails area ends: `╎` alone at lanes = 1,
    // and `╎ 12` for a run of 123 at lanes = 2. At lanes = 1 the id column
    // was also widened by one, though nothing was written into it.
    // A history 123 commits deep needs more stack than a test thread has in
    // a debug build (the binary runs on 512 MB).
    let deep = std::thread::Builder::new().stack_size(64 << 20).spawn(|| {
        for lanes in 1..=3 {
            let (repo, be) = worked_example_with_run(123);
            let (mut i, cfg) = make_interp(be);
            let src = format!("treeWith ({})", lanes_opts(lanes, false));
            let text = tree_text(&mut i, &cfg, &src, repo);
            let ctx = format!("lanes {}\n{}", lanes, text);
            assert_eq!(text.lines().next(), Some("  ╎ 123"), "{}", ctx);
            // the rest of the count fits the id column as it is
            assert_eq!(id_cols(&text), vec![2 + 2 * lanes + 1; WORKED_ROWS.len()], "{}", ctx);
            assert!(row_of(&text, "@aaaa").contains("@aaaa ▅  add parser"), "{}", ctx);
        }
    });
    deep.unwrap().join().unwrap();
}

#[test]
fn an_icons_glyph_fills_both_columns_of_its_lane() {
    // a glyph two columns wide was still followed by its lane's second
    // character, so every lane right of it, and the id, sat one column
    // further right on its row than on the rows around it
    let (repo, be) = worked_example();
    let (mut i, cfg) = make_interp(be);
    let src = format!("treeWith ({})", lanes_opts(3, true));
    let text = tree_text(&mut i, &cfg, &src, repo);
    // the gutter, the rails (lane 1 at column 4, lane 2 at 6) and the space
    // after them: the id starts at column 9 on every row
    let heads: Vec<String> = text.lines().take(9).map(|l| upto_col(l, 9)).collect();
    let want = [
        "  ╎ 14",
        "  🪨╮    ",
        "  🪨│    ",
        "  │ 🍃   ",
        "▶ ├─🔥   ",
        "  │ │    ",
        "  ╰─┼─🫙 ",
        "    ├─🍃 ",
        "    🍃   ",
    ];
    assert_eq!(heads, want, "\n{}", text);
    assert_eq!(id_cols(&text), vec![9; WORKED_ROWS.len()], "\n{}", text);
    // with every lane count, flattened rows (lanes = 2) included, the id is
    // where it is without icons
    for lanes in 2..=4 {
        let src = |icons| format!("treeWith ({})", lanes_opts(lanes, icons));
        let (repo, be) = worked_example();
        let (mut i, cfg) = make_interp(be);
        let plain = tree_text(&mut i, &cfg, &src(false), repo.clone());
        let icons = tree_text(&mut i, &cfg, &src(true), repo);
        assert_eq!(id_cols(&icons), id_cols(&plain), "lanes {}\n{}\n{}", lanes, plain, icons);
        assert_eq!(id_cols(&icons), vec![2 + 2 * lanes + 1; WORKED_ROWS.len()], "lanes {}\n{}", lanes, icons);
    }
}

// ----------------------------------------------------------------------
// row order and lanes (specs/tree.md Steps 1–3): rows go by time, minted
// commits last, and "last child" / "after t" mean in row order, which an
// edit that appends a child (`new`, `rebase`) makes differ from sibling order
// ----------------------------------------------------------------------

/// A commit whose only file is its own, so no row is drawn empty (`◌`).
fn own_commit(id: &str, msg: &str, labels: &[&str]) -> Value {
    commit(id, msg, labels, vec![(id, msg)])
}

/// The rows of the tree of `repo` (focused on the root) as (rails area,
/// the rest as words), the legend left out.
fn row_shapes(repo: Value, metas: Vec<(String, MetaInfo)>) -> Vec<(String, String)> {
    let (mut i, cfg) = make_interp(backend_with(metas));
    let text = tree_text(
        &mut i,
        &cfg,
        "treeWith ({ detail = 0, margin = false, elide = false, icons = false, color = \"never\", lanes = 4, author = false, date = false, files = false })",
        repo,
    );
    text.lines()
        .take_while(|l| !l.trim().is_empty())
        .map(|l| {
            // gutter 2, then 2 · lanes characters of rails
            let rails: String = l.chars().skip(2).take(8).collect();
            let rest: String = l.chars().skip(10).collect();
            let words: Vec<&str> = rest.split_whitespace().collect();
            (rails.trim_end().to_string(), words.join(" "))
        })
        .collect()
}

fn shapes(rows: &[(&str, &str)]) -> Vec<(String, String)> {
    rows.iter().map(|(r, w)| (r.to_string(), w.to_string())).collect()
}

#[test]
fn minted_commits_come_after_stored_ones() {
    // a minted commit (a dry run's `new`) has no stored time and sorted as
    // the oldest, so it took its parent's lane first and ended it, and its
    // stored sibling forked with `├` from under it, drawn as its child. It
    // has time ∞ (Step 1): after every stored row, inheriting the lane.
    let t = now();
    let root = commit(ROOT_ID, "", &[], vec![]);
    let a = own_commit("kaaaaaaa", "one", &[]);
    let b = own_commit("kbbbbbbb", "two", &[]);
    let c = own_commit("kccccccc", "three", &[]);
    let m = own_commit("kmmmmmmm", "minted", &[]);
    let metas = || {
        vec![
            meta(ROOT_ID, "R", 1),
            meta("kaaaaaaa", "A", t - 30),
            meta("kbbbbbbb", "A", t - 20),
            meta("kccccccc", "A", t - 10),
        ]
    };
    let want = shapes(&[
        ("○", "@kaaa one"),
        ("├─○", "@kbbb two"),
        ("│ ○", "@kccc three"),
        ("○", "@kmmm minted"),
    ]);
    // whichever sibling order the edit left (`new` appends)
    for minted_first in [false, true] {
        let b_t = subtree(b.clone(), vec![subtree(c.clone(), vec![])]);
        let m_t = subtree(m.clone(), vec![]);
        let kids = if minted_first { vec![m_t, b_t] } else { vec![b_t, m_t] };
        let repo = repo_of(root.clone(), vec![subtree(a.clone(), kids)], None);
        assert_eq!(row_shapes(repo, metas()), want, "minted first: {}", minted_first);
    }
}

#[test]
fn lane_inheritance_follows_row_order_not_sibling_order() {
    // `rebase` appends the moved commit to its new parent's children but it
    // keeps its time, so the last child in the list need not be the last
    // row. Rule 4 read the list: the older child inherited the lane and
    // emptied it, and the newer one forked from the empty lane under it.
    let t = now();
    let root = commit(ROOT_ID, "", &[], vec![]);
    let a = own_commit("kaaaaaaa", "one", &[]);
    let b = own_commit("kbbbbbbb", "two", &[]);
    let c = own_commit("kccccccc", "three", &[]);
    let d = own_commit("kddddddd", "four", &[]);
    let metas = || {
        vec![
            meta(ROOT_ID, "R", 1),
            meta("kaaaaaaa", "A", t - 40),
            meta("kbbbbbbb", "A", t - 30),
            meta("kccccccc", "A", t - 20),
            meta("kddddddd", "A", t - 10),
        ]
    };
    let want = shapes(&[
        ("○", "@kaaa one"),
        ("├─○", "@kbbb two"),
        ("│ ○", "@kccc three"),
        ("○", "@kddd four"),
    ]);
    for newest_first in [false, true] {
        let b_t = subtree(b.clone(), vec![subtree(c.clone(), vec![])]);
        let d_t = subtree(d.clone(), vec![]);
        let kids = if newest_first { vec![d_t, b_t] } else { vec![b_t, d_t] };
        let repo = repo_of(root.clone(), vec![subtree(a.clone(), kids)], None);
        assert_eq!(row_shapes(repo, metas()), want, "newest first: {}", newest_first);
    }
}

#[test]
fn trunk_head_forks_end_on_the_last_row() {
    // the trunk head's last child in row order ends lane 0 with `╰`; read
    // off the list, the older child ended it and the newer forked `├` below
    let t = now();
    let root = commit(ROOT_ID, "", &[], vec![]);
    let h = own_commit("khhhhhhh", "head", &["main"]);
    let x = own_commit("kxxxxxxx", "older", &[]);
    let y = own_commit("kyyyyyyy", "newer", &[]);
    let metas = || {
        vec![
            meta(ROOT_ID, "R", 1),
            meta("khhhhhhh", "A", t - 30),
            meta("kxxxxxxx", "A", t - 20),
            meta("kyyyyyyy", "A", t - 10),
        ]
    };
    let want = shapes(&[
        ("◆", "@khhh head main"),
        ("├─○", "@kxxx older"),
        ("╰─○", "@kyyy newer"),
    ]);
    for newest_first in [false, true] {
        let (x_t, y_t) = (subtree(x.clone(), vec![]), subtree(y.clone(), vec![]));
        let kids = if newest_first { vec![y_t, x_t] } else { vec![x_t, y_t] };
        let repo = repo_of(root.clone(), vec![subtree(h.clone(), kids)], None);
        assert_eq!(row_shapes(repo, metas()), want, "newest first: {}", newest_first);
    }
}

#[test]
fn trunk_reservations_are_taken_in_row_order() {
    // a trunk commit reserves lanes for its side children after the trunk
    // child "in row order" (Step 3), leftmost first; it took them in sibling
    // order, so the newer side child got the nearer lane
    let t = now();
    let root = commit(ROOT_ID, "", &[], vec![]);
    let p = own_commit("kppppppp", "base", &[]);
    let h = own_commit("khhhhhhh", "head", &["main"]);
    let x = own_commit("kxxxxxxx", "older", &[]);
    let y = own_commit("kyyyyyyy", "newer", &[]);
    let metas = || {
        vec![
            meta(ROOT_ID, "R", 1),
            meta("kppppppp", "A", t - 40),
            meta("khhhhhhh", "A", t - 30),
            meta("kxxxxxxx", "A", t - 20),
            meta("kyyyyyyy", "A", t - 10),
        ]
    };
    let want = shapes(&[
        ("◆─┬─╮", "@kppp base"),
        ("◆ │ │", "@khhh head main"),
        ("  ○ │", "@kxxx older"),
        ("    ○", "@kyyy newer"),
    ]);
    for newest_first in [false, true] {
        let (x_t, y_t) = (subtree(x.clone(), vec![]), subtree(y.clone(), vec![]));
        let h_t = subtree(h.clone(), vec![]);
        let kids = if newest_first { vec![h_t, y_t, x_t] } else { vec![h_t, x_t, y_t] };
        let repo = repo_of(root.clone(), vec![subtree(p.clone(), kids)], None);
        assert_eq!(row_shapes(repo, metas()), want, "newest first: {}", newest_first);
    }
}


// ----------------------------------------------------------------------
// terminal-width truncation (§Step 4). Only a tty has a width in the
// binary, so it is driven here through `tree_with_width`.
// ----------------------------------------------------------------------

/// The tree rows `treeWith opts` draws on a terminal `w` columns wide, the
/// legend (set at the bottom once a row fills the width) left out.
fn rows_on_terminal(i: &mut Interp, cfg: &config::Config, opts: &str, repo: &Value, w: usize) -> Vec<String> {
    let outer = Rc::new(cfg.global_names.clone());
    let e = parse_expr(opts, outer).unwrap();
    let env = i.global_env();
    let o = i.eval(&Rc::new(e), &env).unwrap();
    let text = j::render::tree_with_width(i, &o, repo, Some(w)).unwrap();
    let text = text.as_text().unwrap();
    text.lines()
        .take_while(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect()
}

/// A focused commit with `msg` and the label `lbl` on a root.
fn one_row_repo(msg: &str) -> Value {
    let root = commit(ROOT_ID, "", &[], vec![]);
    let a = commit("kaaaaaaa", msg, &["lbl"], vec![("f", "x")]);
    repo_of(root, vec![subtree(a, vec![])], Some(0))
}

const ONE_ROW_OPTS: &str =
    "{ detail = 1, margin = false, elide = false, icons = false, color = \"never\", lanes = 4, author = false, date = false, files = false }";

#[test]
fn truncation_cuts_only_the_message() {
    // the cut kept only what followed the *last* two-space run, the author
    // initials: labels were cut (`mast…`) and the age dropped, and a message
    // that fitted still got `…`
    let (repo, be) = worked_example();
    let (mut i, cfg) = make_interp(be);
    let w = 40;
    let rows = rows_on_terminal(&mut i, &cfg, &worked_opts(2, false), &repo, w);
    let ctx = rows.join("\n");
    let msg_col = col_of(row_of(&ctx, "@wqzt"), "wip");
    let lab_col = col_of(row_of(&ctx, "@kpqx"), "main");
    assert_eq!(col_of(row_of(&ctx, "@wqzt"), "feature"), lab_col, "labels moved\n{}", ctx);
    // 18 columns before the message and 17 after it (`feature  2d  mo`)
    // leave it 5
    let fits = 5;
    for (id, msg, age) in WORKED_ROWS.iter() {
        let row = row_of(&ctx, id);
        assert!(j::render::width(row) <= w, "{} is {} cols\n{}", id, j::render::width(row), ctx);
        // labels and the margin are whole and in their columns
        assert!(row.ends_with(&format!("{}  {}", age, if *id == "@ptlm" { "ak" } else { "mo" })),
            "{} margin\n{}", id, ctx);
        assert_eq!(col_of(row, age), col_of(row_of(&ctx, "@aaaa"), "3w"), "{} margin\n{}", id, ctx);
        // the message column (`⋯ n` included): whole when it fits, else a
        // prefix of it and `…`
        let full = if *id == "@ptlm" { "docs  ⋯ 3" } else { msg };
        let shown = between_cols(row, msg_col, lab_col);
        let shown = shown.trim_end();
        match shown.strip_suffix('…') {
            Some(head) => assert!(
                j::render::width(full) > fits && !head.is_empty() && full.starts_with(head),
                "{} cut to {:?}\n{}", id, shown, ctx
            ),
            None => assert!(
                j::render::width(full) <= fits && shown == full,
                "{} shows {:?}\n{}", id, shown, ctx
            ),
        }
    }
}

/// The characters of `line` from display column `from` up to `to`.
fn between_cols(line: &str, from: usize, to: usize) -> String {
    let mut col = 0;
    let mut out = String::new();
    for c in line.chars() {
        if col >= from && col < to {
            out.push(c);
        }
        col += j::render::width(&c.to_string());
    }
    out
}

#[test]
fn truncation_cuts_rows_without_labels_or_margin() {
    // with no label column and no margin nothing was ever cut
    let root = commit(ROOT_ID, "", &[], vec![]);
    let a = commit("kaaaaaaa", &"a long message ".repeat(6), &[], vec![("f", "x")]);
    let repo = repo_of(root, vec![subtree(a, vec![])], Some(0));
    let (mut i, cfg) = make_interp(backend_with(vec![meta(ROOT_ID, "R", 1)]));
    let rows = rows_on_terminal(&mut i, &cfg, ONE_ROW_OPTS, &repo, 30).join("\n");
    let row = row_of(&rows, "@kaaa");
    assert!(row.starts_with("▶ ◉") && row.ends_with('…'), "{:?}", row);
    assert_eq!(j::render::width(row), 30, "{:?}", row);
}

#[test]
fn truncation_keeps_the_tail_and_fits() {
    let repo = one_row_repo(&"message ".repeat(20));
    let (mut i, cfg) = make_interp(backend_with(vec![meta(ROOT_ID, "R", 1)]));
    let rows = rows_on_terminal(&mut i, &cfg, ONE_ROW_OPTS, &repo, 60).join("\n");
    let row = row_of(&rows, "@kaaa");
    assert!(row.ends_with("…  lbl"), "tail lost: {:?}", row);
    assert!(j::render::width(row) <= 60, "{} cols: {:?}", j::render::width(row), row);
}

#[test]
fn truncation_keeps_colours_closed() {
    // the cut runs on the coloured message (bold on the focus row): the
    // escape sequences are kept, not counted as width or cut in half
    unsafe { std::env::remove_var("NO_COLOR") };
    let repo = one_row_repo(&"message ".repeat(20));
    let (mut i, cfg) = make_interp(backend_with(vec![meta(ROOT_ID, "R", 1)]));
    let opts = ONE_ROW_OPTS.replace("\"never\"", "\"always\"");
    let rows = rows_on_terminal(&mut i, &cfg, &opts, &repo, 60).join("\n");
    let row = row_of(&rows, "@kaaa");
    assert!(row.contains("…\x1b[0m"), "{:?}", row);
    // the focus band pads the row to the terminal's width, and no further
    assert_eq!(j::render::width(row), 60, "{:?}", row);
}

#[test]
fn truncation_leaves_short_lines_alone() {
    let repo = one_row_repo("short");
    let (mut i, cfg) = make_interp(backend_with(vec![meta(ROOT_ID, "R", 1)]));
    let rows = rows_on_terminal(&mut i, &cfg, ONE_ROW_OPTS, &repo, 100).join("\n");
    let row = row_of(&rows, "@kaaa");
    assert!(row.contains("  short  lbl") && !row.contains('…'), "{:?}", row);
}

#[test]
fn truncation_handles_multibyte_messages() {
    // the head was sliced with a *char* count used as a *byte* index, which
    // panics whenever that index lands inside a multi-byte character
    let (mut i, cfg) = make_interp(backend_with(vec![meta(ROOT_ID, "R", 1)]));
    for n in 1..80 {
        let repo = one_row_repo(&"ω".repeat(n));
        for w in [20usize, 30, 40, 50, 60, 80] {
            let rows = rows_on_terminal(&mut i, &cfg, ONE_ROW_OPTS, &repo, w).join("\n");
            let row = row_of(&rows, "@kaaa");
            // only the message is cut: the gutter, rails, id, bar and label
            // take 25 columns whatever the width
            assert!(
                j::render::width(row) <= w.max(25),
                "n={} w={} -> {:?} ({} cols)",
                n,
                w,
                row,
                j::render::width(row)
            );
            // (on a wide terminal the legend follows it)
            assert!(row.contains("  lbl"), "n={} w={} -> {:?}", n, w, row);
        }
    }
}

#[test]
fn truncation_survives_mixed_scripts() {
    // wide (CJK), combining and ASCII in one message
    let (mut i, cfg) = make_interp(backend_with(vec![meta(ROOT_ID, "R", 1)]));
    for msg in [
        "日本語のテキストがとても長い場合の折り返し処理",
        "réfactorisation très importante — étape finale ✓",
        "αβγδε ωωωωω ✓✓✓ ascii tail here",
        "a̐éö̲ combining marks",
    ] {
        let repo = one_row_repo(msg);
        for w in [10usize, 25, 45, 70] {
            let rows = rows_on_terminal(&mut i, &cfg, ONE_ROW_OPTS, &repo, w).join("\n");
            let row = row_of(&rows, "@kaaa");
            assert!(j::render::width(row) <= w.max(25), "msg={:?} w={} -> {:?}", msg, w, row);
            assert!(row.contains("  lbl"), "msg={:?} w={} -> {:?}", msg, w, row);
        }
    }
}
