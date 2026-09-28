//! Display tests (§5.1 examples) over the in-memory backend.

use j::config;
use j::domain::{MemBackend, MetaInfo, ROOT_ID};
use j::eval::Interp;
use j::parse::parse_expr;
use j::value::{
    BlobContent, BlobKind, BlobVal, ConflictSide, Crash, Env, LazyBlob, ThunkVal, Value,
};
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
fn lambda_over_several_lines_is_one_line_in_line_form() {
    // a lambda's source (§5.2) may run over several lines and hold comments;
    // its line form is the first line, `…` if there are more, as a Text's is
    // (§5.1), so a record keeps one line per field and a column table one
    // line per row, aligned. Its block form is all of the source.
    let (repo, be) = sample_repo();
    let (mut i, cfg) = make_interp(be);
    let f = "\\x -> x -- note\n      + 1";
    let out = eval_and_display(&mut i, &cfg, &format!("{{ f = {}, g = 2 }}", f), repo.clone());
    assert_eq!(out, "f  \\x -> x -- note…\ng  2\n");
    let out = eval_and_display(
        &mut i,
        &cfg,
        &format!("[({{ f = {}, g = 2 }}) ({{ f = \\y -> y, g = (1 +) 2 }})]", f),
        repo.clone(),
    );
    assert_eq!(out, "f                  g\n\\x -> x -- note…   2\n\\y -> y            3\n");
    let out = eval_and_display(&mut i, &cfg, &format!("{{ fs = [({}) (\\y -> y)] }}", f), repo.clone());
    assert_eq!(out, "fs  \n    \\x -> x -- note\n          + 1\n\n    \\y -> y\n");
    // a partial application whose argument runs over lines is cut the same way
    let out = eval_and_display(&mut i, &cfg, &format!("{{ m = map ({}) }}", f), repo);
    assert_eq!(out, "m  map (\\x -> x -- note…\n");
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
                    Some(j::value::ConflictSide::regular(b"ours\n")),
                    Some(j::value::ConflictSide::regular(b"base\n")),
                    Some(j::value::ConflictSide::regular(b"theirs\n")),
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

#[test]
fn blocks_of_blocks_take_no_stack_a_level() {
    // §5.1: a list that is not a table or paths is one block per item, and
    // a generic record's list field is a block under its key, so blocks nest
    // as deep as the value's lists do. Display recursed once for each, about
    // 14 KB a level in a debug build: a list nested as deep as the parser
    // allows (§3.4) aborted the binary's 512 MB, and these depths overflow
    // 16 MB. Running out of stack aborts the process, an exit §1.4 does not
    // allow.
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            let (mut i, _cfg) = make_interp(MemBackend::new());
            // `[[[[] 0] 1] …]`
            let n = 20_000;
            let lists = (0..n).fold(Value::list(vec![]), |v, k| Value::list(vec![v, Value::int(k)]));
            let items: String = (0..n).map(|k| format!("\n{}\n", k)).collect();
            let out = j::render::display(&mut i, &lists, false).unwrap();
            assert!(out == format!("none\n{}", items), "{}…", &out[..40.min(out.len())]);
            // `[({ a = [({ a = [] }) 0] }) 1]` and so on: each record's list
            // is indented under its key, the innermost empty one a line
            let n = 2_000;
            let records = (0..n).fold(Value::list(vec![]), |v, k| {
                Value::list(vec![Value::record(&[("a", v)]), Value::int(k)])
            });
            let pad = |k: i64| " ".repeat(3 * (n - 1 - k) as usize);
            let mut want: String = (1..n).rev().map(|k| format!("{}a  \n", pad(k))).collect();
            want.push_str(&format!("{}a  0 items\n", pad(0)));
            want.extend((0..n).map(|k| format!("\n{}{}\n", pad(k), k)));
            let out = j::render::display(&mut i, &records, false).unwrap();
            assert!(out == want, "{}…", &out[..40.min(out.len())]);
        })
        .unwrap()
        .join()
        .unwrap();
}

/// The glyph `tree` draws on the row of the commit whose message is `msg`.
fn tree_glyph(out: &str, msg: &str) -> char {
    let row = out
        .lines()
        .find(|l| l.contains(&format!("  {}", msg)))
        .unwrap_or_else(|| panic!("no row for {:?}:\n{}", msg, out));
    row.chars().find(|c| "◉●○◆◌⊗⌂".contains(*c)).unwrap()
}

#[test]
fn focus_example() {
    // the block listed the commit's whole snapshot, counted and unmarked, and
    // drew every commit with the focus glyph; §5.1's `j focus` lists the paths
    // it changes against its parent, marked, under the line `tree` draws
    let (repo, be) = sample_repo();
    let (mut i, cfg) = make_interp(be);
    let out = eval_and_display(&mut i, &cfg, "focus", repo.clone());
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "◉ wqztbbbb  wip  feature", "{}", out);
    assert!(
        lines[1].starts_with("  Montelot · ") && lines[1].ends_with(" · 2 files"),
        "{}",
        out
    );
    assert_eq!(lines[2..], ["", "  + src/lexer.rs", "  ~ src/parser.rs"], "{}", out);
    // the parent is not the focus, and `main` puts it in the immutable set
    let tree = eval_and_display(&mut i, &cfg, "tree", repo.clone());
    let out = eval_and_display(&mut i, &cfg, "\\r -> (up r).root", repo);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(tree_glyph(&tree, "add parser"), '◆', "{}", tree);
    assert_eq!(lines[0], "◆ kpqxaaaa  add parser  main", "{}", out);
    assert!(lines[1].ends_with(" · 1 files"), "{}", out);
    assert_eq!(lines[2..], ["", "  + src/parser.rs"], "{}", out);
}

/// A linear history above the root with eager file lists, oldest first,
/// focused on its last commit.
fn linear_repo(chain: Vec<(&str, &str, Vec<(&str, &str)>)>) -> (Value, MemBackend) {
    let frame = |c: Value| {
        Value::record(&[
            ("left", Value::list(vec![])),
            ("parent", c),
            ("right", Value::list(vec![])),
        ])
    };
    let mut be = MemBackend::new();
    be.metas.insert(ROOT_ID.to_string(), meta(ROOT_ID, "Root", 1_700_000_000).1);
    let mut frames = vec![frame(commit(ROOT_ID, "", &[], vec![]))];
    let mut parent = ROOT_ID.to_string();
    let n = chain.len();
    let mut focus = None;
    for (k, (id, msg, files)) in chain.into_iter().enumerate() {
        be.metas.insert(id.to_string(), meta(id, "M", 1_700_000_100 + k as i64).1);
        be.parents.insert(id.to_string(), vec![parent]);
        let c = commit(id, msg, &[], files);
        if k + 1 == n {
            focus = Some(c);
        } else {
            frames.push(frame(c));
        }
        parent = id.to_string();
    }
    frames.reverse();
    let repo = Value::record(&[
        ("children", Value::list(vec![])),
        ("context", Value::list(frames)),
        ("root", focus.unwrap()),
    ]);
    (repo, be)
}

#[test]
fn commit_block_marks_deletions_and_draws_the_tree_glyph() {
    // a deleted path was missing from the block, an unchanged one listed, an
    // ancestor of the focus drawn as the focus, and an empty commit never
    // drawn `◌` (§5.1; specs/tree.md §Glyphs)
    //
    // root -> a -> b (edits f, deletes g) -> c (the focus, b's files: empty)
    let (repo, be) = linear_repo(vec![
        ("kpqxaaaa", "msg-a", vec![("f.txt", "one\n"), ("g.txt", "g\n")]),
        ("kpqxbbbb", "msg-b", vec![("f.txt", "two\n")]),
        ("kpqxcccc", "msg-c", vec![("f.txt", "two\n")]),
    ]);
    let (mut i, cfg) = make_interp(be);
    let tree = eval_and_display(&mut i, &cfg, "tree", repo.clone());
    let b = eval_and_display(&mut i, &cfg, "\\r -> (up r).root", repo.clone());
    let lines: Vec<&str> = b.lines().collect();
    assert_eq!(tree_glyph(&tree, "msg-b"), '●', "{}", tree);
    assert_eq!(lines[0], "● kpqxbbbb  msg-b", "{}", b);
    assert!(lines[1].starts_with("  M · ") && lines[1].ends_with(" · 2 files"), "{}", b);
    assert_eq!(lines[2..], ["", "  ~ f.txt", "  − g.txt"], "{}", b);
    // the focus changes nothing: no paths under it
    let c = eval_and_display(&mut i, &cfg, "focus", repo.clone());
    let lines: Vec<&str> = c.lines().collect();
    assert_eq!(tree_glyph(&tree, "msg-c"), '◌', "{}", tree);
    assert_eq!(lines[0], "◌ kpqxcccc  msg-c", "{}", c);
    assert!(lines[1].ends_with(" · 0 files"), "{}", c);
    assert_eq!(lines.len(), 2, "{}", c);
    // a list holding commits among other values prints each one's block
    let both = eval_and_display(&mut i, &cfg, "\\r -> [(up r).root r.root 1]", repo);
    assert_eq!(both, format!("{}\n{}\n1\n", b, c));
}

/// The line under key `key` of a displayed record, without the key.
fn field_line<'a>(out: &'a str, key: &str) -> &'a str {
    out.lines()
        .find_map(|l| l.strip_prefix(key).filter(|rest| rest.starts_with(' ')))
        .unwrap_or_else(|| panic!("no field {:?} in:\n{}", key, out))
        .trim_start()
}

#[test]
fn a_commit_line_draws_the_glyph_its_block_does() {
    // the line form drew `○` for every commit and a `Repo` `◉`, so a commit
    // nested in a record, a frame, a subtree or a commit table was drawn
    // otherwise than its block, whose first line §5.1 defines as that line,
    // and than `tree`: the root got `○` in a table, not `⌂`
    let (repo, be) = sample_repo();
    let (mut i, cfg) = make_interp(be);
    let first = |out: String| out.lines().next().unwrap().to_string();
    let focus = first(eval_and_display(&mut i, &cfg, "focus", repo.clone()));
    let parent = first(eval_and_display(&mut i, &cfg, "focus . up", repo.clone()));
    let root = first(eval_and_display(&mut i, &cfg, "focus . top", repo.clone()));
    assert_eq!(focus, "◉ wqztbbbb  wip  feature");
    assert_eq!(parent, "◆ kpqxaaaa  add parser  main");
    assert!(root.starts_with("⌂ "), "{}", root);
    let src = "\\r -> { c = r.root, p = (up r).root, t = (top r).root, repo = r, \
               frame = head r.context, sub = head (up r).children }";
    let rec = eval_and_display(&mut i, &cfg, src, repo.clone());
    for (key, want) in [
        ("c", &focus),
        ("p", &parent),
        ("t", &root),
        ("repo", &focus),
        ("frame", &parent),
        ("sub", &focus),
    ] {
        assert_eq!(field_line(&rec, key), want, "{}\n{}", key, rec);
    }
    let table = eval_and_display(&mut i, &cfg, "ancestors", repo);
    assert_eq!(table, format!("{}\n{}\n{}\n", focus, parent, root));
    // an empty focus is `◌` in every form, as in `tree`
    let (repo, be) = linear_repo(vec![
        ("kpqxaaaa", "msg-a", vec![("f.txt", "one\n")]),
        ("kpqxbbbb", "msg-b", vec![("f.txt", "one\n")]),
    ]);
    let (mut i, cfg) = make_interp(be);
    let tree = eval_and_display(&mut i, &cfg, "tree", repo.clone());
    let block = first(eval_and_display(&mut i, &cfg, "focus", repo.clone()));
    let rec = eval_and_display(&mut i, &cfg, "\\r -> { c = r.root, repo = r }", repo);
    assert_eq!(tree_glyph(&tree, "msg-b"), '◌', "{}", tree);
    assert_eq!(block, "◌ kpqxbbbb  msg-b");
    assert_eq!(field_line(&rec, "c"), block, "{}", rec);
    assert_eq!(field_line(&rec, "repo"), block, "{}", rec);
}

#[test]
fn a_commit_the_expression_made_lists_its_files_unmarked() {
    // its parent is not in the repository the expression was given, and the
    // block diffed it against no files: `focus . new` said the new commit adds
    // every file, where `changeOf . new` and `tree . new` say it changes
    // nothing. What it changes is unknown there, so its block lists its files
    // unmarked and its glyph claims no `◌` (§5.1).
    let (repo, be) = sample_repo();
    let (mut i, cfg) = make_interp(be);
    let out = eval_and_display(&mut i, &cfg, "focus . new", repo.clone());
    let lines: Vec<&str> = out.lines().collect();
    // the minted id is random, so tell the new commit from the focus by the
    // focus's message, which no id can hold (ids are letters k..z)
    assert!(lines[0].starts_with("○ ") && !lines[0].contains("wip"), "{}", out);
    assert_eq!(lines[1..], ["", "    src/lexer.rs", "    src/parser.rs"], "{}", out);
    let rec = eval_and_display(&mut i, &cfg, "\\r -> { c = (new r).root }", repo.clone());
    assert!(field_line(&rec, "c").starts_with("○ "), "{}", rec);
    // an unresolved file keeps its mark
    let conflicted = Value::Blob(Rc::new(BlobVal {
        kind: BlobKind::Regular,
        content: BlobContent::Conflict(vec![
            Some(ConflictSide::regular(b"a\n")),
            None,
            Some(ConflictSide::regular(b"b\n")),
        ]),
    }));
    let made = Value::record(&[
        (
            "files",
            Value::list(vec![Value::record(&[
                ("content", conflicted),
                ("path", Value::list(vec![Value::text("x.txt")])),
            ])]),
        ),
        ("id", Value::Id(Rc::new("kpqxnnnn".to_string()))),
        ("labels", Value::list(vec![])),
        ("message", Value::text("made")),
    ]);
    let out = j::render::display(&mut i, &made, false).unwrap();
    assert_eq!(out, "⊗ kpqxnnnn  made\n\n  ✖ x.txt\n");
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
    lazy_commit_of(id, msg, entries)
}

fn lazy_commit_of(id: &str, msg: &str, entries: Vec<Value>) -> Value {
    Value::record(&[
        ("files", j::domain::lazy_files(id, entries)),
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

#[test]
fn tree_draws_the_focus_parents_bar_when_files_are_lazy() {
    // the focus's parent diffs against its own parent, whose file list a
    // backend that answers `is_empty` never loaded: the bar went missing
    // (specs/tree.md, Step 4 column 5)
    let (repo, be) = lazy_repo_chain(3);
    let (mut i, cfg) = make_interp(be);
    let out = eval_and_display(&mut i, &cfg, "tree", repo);
    let bars = ['▁', '▂', '▃', '▅', '▇'];
    for (msg, bar) in [("commit 0", false), ("commit 1", true), ("commit 2", true)] {
        let row = out.lines().find(|l| l.ends_with(msg)).unwrap();
        assert_eq!(row.contains(bars), bar, "{:?}\n{}", row, out);
    }
}

fn conflicted_entry(path: &str) -> Value {
    Value::record(&[
        (
            "content",
            Value::Blob(Rc::new(BlobVal {
                kind: j::value::BlobKind::Regular,
                content: j::value::BlobContent::Conflict(vec![
                    Some(j::value::ConflictSide::regular(b"ours\n")),
                    Some(j::value::ConflictSide::regular(b"base\n")),
                    Some(j::value::ConflictSide::regular(b"theirs\n")),
                ]),
            })),
        ),
        ("path", Value::list(vec![Value::text(path)])),
    ])
}

fn text_entry(path: &str, text: &str) -> Value {
    Value::record(&[
        ("content", BlobVal::text_blob(text)),
        ("path", Value::list(vec![Value::text(path)])),
    ])
}

#[test]
fn tree_glyphs_of_an_edited_lazy_repo_come_from_its_files() {
    // the backend answers `has_conflict`/`is_empty` for the *stored* commits;
    // an edit keeps a commit's id but not its files, and the tree still drew
    // the stored glyphs — for the edited commit and for its untouched child —
    // wherever the focus's neighbourhood did not force a diff (§1.2 dry run)
    //
    // root -> a -> b (same files: stored empty) -> c (stored conflict)
    //      -> d -> e (focus)
    let f1 = || text_entry("f.txt", "one\n");
    let chain: Vec<(&str, &str, Vec<Value>)> = vec![
        ("kpqxkkkk", "msg-a", vec![f1()]),
        ("kpqxllll", "msg-b", vec![f1()]),
        ("kpqxmmmm", "msg-c", vec![conflicted_entry("f.txt")]),
        ("kpqxnnnn", "msg-d", vec![conflicted_entry("f.txt"), text_entry("g.txt", "g\n")]),
        ("kpqxoooo", "msg-e", vec![text_entry("f.txt", "two\n"), text_entry("g.txt", "g\n")]),
    ];
    let mut be = MemBackend::answering();
    be.metas.insert(ROOT_ID.to_string(), meta(ROOT_ID, "Root", 1_700_000_000).1);
    be.parents.insert(ROOT_ID.to_string(), vec![]);
    let mut frames = vec![Value::record(&[
        ("left", Value::list(vec![])),
        ("parent", lazy_commit_of(ROOT_ID, "", vec![])),
        ("right", Value::list(vec![])),
    ])];
    let mut parent = ROOT_ID;
    let mut focus = None;
    for (k, (id, msg, entries)) in chain.into_iter().enumerate() {
        be.metas.insert(id.to_string(), meta(id, "M", 1_700_000_100 + k as i64).1);
        be.parents.insert(id.to_string(), vec![parent.to_string()]);
        be.empties.insert(id.to_string(), id == "kpqxllll");
        be.conflicts.insert(id.to_string(), id == "kpqxmmmm" || id == "kpqxnnnn");
        let c = lazy_commit_of(id, msg, entries);
        if id == "kpqxoooo" {
            focus = Some(c);
        } else {
            frames.push(Value::record(&[
                ("left", Value::list(vec![])),
                ("parent", c),
                ("right", Value::list(vec![])),
            ]));
        }
        parent = id;
    }
    frames.reverse();
    let repo = Value::record(&[
        ("children", Value::list(vec![])),
        ("context", Value::list(frames)),
        ("root", focus.unwrap()),
    ]);
    let (mut i, cfg) = make_interp(be);
    let glyph = |out: &str, msg: &str| -> char {
        let row = out
            .lines()
            .find(|l| l.contains(&format!("  {}", msg)))
            .unwrap_or_else(|| panic!("no row for {:?}:\n{}", msg, out));
        row.chars().find(|c| "◉●○◆◌⊗⌂".contains(*c)).unwrap()
    };

    // unedited: the stored answers hold
    let out = eval_and_display(&mut i, &cfg, "tree", repo.clone());
    assert_eq!(glyph(&out, "msg-a"), '●', "{}", out);
    assert_eq!(glyph(&out, "msg-b"), '◌', "{}", out);
    assert_eq!(glyph(&out, "msg-c"), '⊗', "{}", out);

    for (edit, want) in [
        // `a` emptied: it is empty now, and `b` (untouched) no longer is
        (
            "at (const [@kpqxkkkk]) (mapRoot (\\c -> c { files = [] }))",
            vec![("msg-a", '◌'), ("msg-b", '●'), ("msg-c", '⊗')],
        ),
        // `c`'s conflict resolved to its parent's files
        (
            "at (const [@kpqxmmmm]) (\\r -> r { root = r.root { files = (up r).root.files } })",
            vec![("msg-a", '●'), ("msg-b", '◌'), ("msg-c", '◌')],
        ),
        // `b` given `c`'s conflicted files
        (
            "\\r -> at (const [@kpqxllll]) (mapRoot (\\c -> c { files = (commitAt @kpqxmmmm r).files })) r",
            vec![("msg-a", '●'), ("msg-b", '⊗'), ("msg-c", '⊗')],
        ),
    ] {
        for renderer in ["tree", "treeFull"] {
            let out = eval_and_display(&mut i, &cfg, &format!("{} . ({})", renderer, edit), repo.clone());
            for (msg, g) in &want {
                assert_eq!(glyph(&out, msg), *g, "{} of `{}` at {}:\n{}", msg, edit, renderer, out);
            }
        }
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
        // `show` crashes as any other use does, and does not stand `<lazy>`
        // (which does not parse) in for the list: a record, first rendered
        // on one line and then again broken over lines, a list, a function
        "show repo.root",
        "[(show repo.root or \"\") (show repo.root)]",
        "show [repo.root]",
        "show (const repo.root)",
    ] {
        let (repo, be) = unreadable_repo(unreadable_files());
        let (mut i, cfg) = make_interp(be);
        let got = try_eval_and_display(&mut i, &cfg, &format!("(\\repo -> {})", src), repo);
        match got {
            Err(msg) => assert!(msg.contains(UNREADABLE), "{}: crashed with {:?}", src, msg),
            Ok(out) => panic!("{}: the second look did not crash:\n{}", src, out),
        }
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
        // `show` crashes too: `blob "<unreadable>"` is the literal of
        // another blob, one holding that text (§5.2)
        "show (contentAt [\"f.txt\"] (files repo))",
        "let b = contentAt [\"f.txt\"] (files repo) in [(text b or \"x\") (show b)]",
        "show (const (contentAt [\"f.txt\"] (files repo)))",
        "[(show (files repo)) (show (files repo))]",
        // and so does displaying a function holding it, rendered by `show`
        "const (contentAt [\"f.txt\"] (files repo))",
    ] {
        let (repo, be) = unreadable_repo(unreadable_blob_files());
        let (mut i, cfg) = make_interp(be);
        let got = try_eval_and_display(&mut i, &cfg, &format!("(\\repo -> {})", src), repo);
        match got {
            Err(msg) => assert!(msg.contains(UNREADABLE), "{}: crashed with {:?}", src, msg),
            Ok(out) => panic!("{}: the second look did not crash:\n{}", src, out),
        }
    }
    // the crash is an ordinary one, which `or` catches
    let (repo, be) = unreadable_repo(unreadable_blob_files());
    let (mut i, cfg) = make_interp(be);
    let src = "(\\repo -> show (contentAt [\"f.txt\"] (files repo)) or \"caught\")";
    assert_eq!(try_eval_and_display(&mut i, &cfg, src, repo), Ok("caught\n".to_string()));
}
