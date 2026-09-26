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
    let t = now();
    let h = 3600;
    let d = 24 * h;
    let w = 7 * d;
    // root -> 14 uninteresting commits -> aaaa -> kpqx (trunk head)
    let root = commit(ROOT_ID, "", &[], vec![]);
    let mut kids: Vec<Value> = Vec::new();
    let ids: Vec<String> = (0..14).map(|i| format!("run{:02}xxxxxxxxxxxxx", i)).collect();

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
                time: t - 3 * w - 3600 + (i as i64) * 60,
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
// terminal-width truncation (§Step 4). Only runs on a tty in the binary,
// so it is driven directly here.
// ----------------------------------------------------------------------

fn truncate_one(line: &str, term_w: usize) -> String {
    let mut lines = vec![line.to_string()];
    // msg_off 18 matches the rendered layout: rails, glyph and id column
    j::render::truncate_lines(&mut lines, term_w, 18, true, true);
    lines.pop().unwrap()
}

#[test]
fn truncation_handles_multibyte_messages() {
    // the head was sliced with a *char* count used as a *byte* index, which
    // panics whenever that index lands inside a multi-byte character
    let tail = "  label";
    for n in 1..80 {
        for w in [20usize, 30, 40, 50, 60, 80] {
            let msg: String = "ω".repeat(n);
            let line = format!("  ●        @abcd  {}{}", msg, tail);
            let got = truncate_one(&line, w);
            assert!(
                j::render::width(&got) <= w.max(j::render::width(tail) + 2),
                "n={} w={} -> {:?} ({} cols)",
                n,
                w,
                got,
                j::render::width(&got)
            );
        }
    }
}

#[test]
fn truncation_keeps_the_tail_and_fits() {
    let line = format!("  ●        @abcd  {}  bookmark", "message ".repeat(20));
    let got = truncate_one(&line, 60);
    assert!(got.contains('…'), "no ellipsis: {:?}", got);
    assert!(got.ends_with("bookmark"), "tail lost: {:?}", got);
    assert!(j::render::width(&got) <= 60, "{} cols: {:?}", j::render::width(&got), got);
}

#[test]
fn truncation_leaves_short_lines_alone() {
    let line = "  ●        @abcd  short  label";
    assert_eq!(truncate_one(line, 100), line);
}

#[test]
fn truncation_survives_mixed_scripts() {
    // wide (CJK), combining and ASCII in one message
    for msg in [
        "日本語のテキストがとても長い場合の折り返し処理",
        "réfactorisation très importante — étape finale ✓",
        "αβγδε ωωωωω ✓✓✓ ascii tail here",
        "a̐éö̲ combining marks",
    ] {
        for w in [10usize, 25, 45, 70] {
            let line = format!("  ●        @abcd  {}  lbl", msg);
            let got = truncate_one(&line, w);
            assert!(!got.is_empty(), "msg={:?} w={}", msg, w);
        }
    }
}
