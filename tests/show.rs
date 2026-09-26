//! `show` round-trips (§5.2), unified diff (§4.9), and text rendering.

use j::config;
use j::domain::MemBackend;
use j::eval::Interp;
use j::parse::parse_expr;
use j::show::{show, text_literal, unified_diff};
use j::value::{value_eq, BlobContent, BlobKind, BlobVal, Env, Value};
use std::rc::Rc;

const CONFIG: &str = include_str!("../config.j");

fn make_interp() -> (Interp, config::Config) {
    let cfg = config::load_config(CONFIG).expect("reference config must load");
    let mut interp = Interp::new(
        Rc::new(MemBackend::new()),
        cfg.shapes.clone(),
        Env::empty(),
    );
    config::eval_config(&mut interp, &cfg).expect("reference config must evaluate");
    (interp, cfg)
}

fn roundtrip(i: &mut Interp, cfg: &config::Config, v: &Value) {
    let s = show(i, v);
    let outer = Rc::new(cfg.global_names.clone());
    let e = parse_expr(&s, outer)
        .unwrap_or_else(|err| panic!("show output {:?} does not parse: {}", s, err.msg));
    let e = config::resolve_ids(&e, i).unwrap();
    let env = i.global_env();
    let v2 = i
        .eval(&Rc::new(e), &env)
        .unwrap_or_else(|c| panic!("show output {:?} does not evaluate: {}", s, c.msg));
    assert!(
        value_eq(v, &v2).unwrap_or(false),
        "roundtrip failed: {:?} -> {} -> {}",
        show(i, v),
        s,
        show(i, &v2)
    );
}

#[test]
fn show_roundtrips_all_kinds() {
    let (mut i, cfg) = make_interp();
    roundtrip(&mut i, &cfg, &Value::int(42));
    roundtrip(&mut i, &cfg, &Value::int(-7));
    roundtrip(&mut i, &cfg, &Value::text(""));
    roundtrip(&mut i, &cfg, &Value::text("with \"quotes\" and \\ slashes\nnewlines\ttabs\rCR"));
    roundtrip(&mut i, &cfg, &Value::Bool(true));
    roundtrip(&mut i, &cfg, &Value::Bool(false));
    roundtrip(&mut i, &cfg, &Value::list(vec![]));
    roundtrip(
        &mut i,
        &cfg,
        &Value::list(vec![Value::int(1), Value::text("two"), Value::Bool(false)]),
    );
    roundtrip(&mut i, &cfg, &Value::record(&[]));
    roundtrip(
        &mut i,
        &cfg,
        &Value::record(&[
            ("zebra", Value::int(1)),
            ("apple", Value::list(vec![Value::int(2)])),
        ]),
    );
    roundtrip(
        &mut i,
        &cfg,
        &Value::record(&[(
            "nested",
            Value::record(&[("deep", Value::list(vec![Value::list(vec![])]))]),
        )]),
    );
    // blobs
    roundtrip(&mut i, &cfg, &j::value::BlobVal::text_blob("content"));
    // shapes
    let s = i.shapes.shape_of("Commit").unwrap();
    roundtrip(&mut i, &cfg, &Value::Shape(Rc::new(s)));
}

#[test]
fn show_record_field_order() {
    let (i, _cfg) = make_interp();
    let v = Value::record(&[("z", Value::int(1)), ("a", Value::int(2)), ("m", Value::int(3))]);
    assert_eq!(show(&i, &v), "{ a = 2, m = 3, z = 1 }");
}

#[test]
fn show_escapes() {
    assert_eq!(text_literal("a\"b"), "\"a\\\"b\"");
    assert_eq!(text_literal("a\\b"), "\"a\\\\b\"");
    assert_eq!(text_literal("a\nb\tc\rd"), "\"a\\nb\\tc\\rd\"");
}

#[test]
fn show_nested_list_atoms() {
    let (i, _cfg) = make_interp();
    // a nested list is a self-delimiting atom: no parens (§5.2)
    let v = Value::list(vec![
        Value::list(vec![Value::int(1), Value::int(2)]),
        Value::list(vec![Value::int(3)]),
    ]);
    assert_eq!(show(&i, &v), "[[1 2] [3]]");
    let deep = Value::list(vec![Value::list(vec![Value::list(vec![Value::int(1)])])]);
    assert_eq!(show(&i, &deep), "[[[1]]]");
    // a record element keeps its parens: `{ … }` after another element would
    // parse as a record update on it (§3.4)
    let r = Value::list(vec![Value::record(&[("a", Value::int(1))])]);
    assert_eq!(show(&i, &r), "[({ a = 1 })]");
    let mixed = Value::list(vec![
        Value::text("x"),
        Value::record(&[("a", Value::int(1))]),
    ]);
    assert_eq!(show(&i, &mixed), "[\"x\" ({ a = 1 })]");
}

#[test]
fn show_unresolved_blob() {
    let (i, _cfg) = make_interp();
    let v = Value::Blob(Rc::new(BlobVal {
        kind: BlobKind::Regular,
        content: BlobContent::Conflict(vec![
            Rc::new(b"ours".to_vec()),
            Rc::new(b"base".to_vec()),
            Rc::new(b"theirs".to_vec()),
        ]),
    }));
    let s = show(&i, &v);
    assert!(s.starts_with("{- unresolved -} blob"), "{}", s);
}

#[test]
fn show_line_breaking() {
    let (i, _cfg) = make_interp();
    // over 80 columns breaks
    let long = Value::list((0..30).map(Value::int).collect());
    let s = show(&i, &long);
    assert!(s.contains('\n'), "{}", s);
    // short stays on one line
    let short = Value::list(vec![Value::int(1), Value::int(2)]);
    assert!(!show(&i, &short).contains('\n'));
}

#[test]
fn show_wide_roundtrips() {
    let (mut i, cfg) = make_interp();
    // a record wide enough to force multi-line rendering must still parse and
    // evaluate back to an equal value
    let names: Vec<String> = (0..14).map(|n| format!("field{:02}", n)).collect();
    let fields: Vec<(&str, Value)> = names
        .iter()
        .enumerate()
        .map(|(n, name)| (name.as_str(), Value::int(n as i64)))
        .collect();
    let wide_rec = Value::record(&fields);
    let s = show(&i, &wide_rec);
    assert!(s.contains('\n'), "expected wide rendering: {}", s);
    roundtrip(&mut i, &cfg, &wide_rec);
    // a wide list too
    let wide_list = Value::list((0..30).map(Value::int).collect());
    assert!(show(&i, &wide_list).contains('\n'));
    roundtrip(&mut i, &cfg, &wide_list);
    // a wide record with nested wide values
    let nested = Value::record(&[
        ("alpha", Value::list((0..20).map(Value::int).collect())),
        ("beta", Value::list((100..120).map(Value::int).collect())),
        ("gamma", Value::list((200..220).map(Value::int).collect())),
    ]);
    let s = show(&i, &nested);
    assert!(s.contains('\n'), "{}", s);
    roundtrip(&mut i, &cfg, &nested);
}

#[test]
fn show_wide_sees_through_lazy_files() {
    // a commit's `files` is a thunk until forced (value.rs); `show` must give
    // the same text for it as for the equal record holding the forced list,
    // breaking the list one element per line (§5.2)
    let (mut i, cfg) = make_interp();
    let entries: Vec<Value> = (1..=4)
        .map(|n| {
            Value::record(&[
                ("content", BlobVal::text_blob(&format!("content {}\n", n))),
                ("path", Value::list(vec![Value::text(format!("file{}.txt", n))])),
            ])
        })
        .collect();
    let commit = |files: Value| {
        Value::record(&[
            ("files", files),
            ("id", Value::Id(Rc::new("xruqnqvokyloollnxruqnqvokyloolln".to_string()))),
            ("labels", Value::list(vec![])),
            ("message", Value::text("")),
        ])
    };
    let lazy = commit(j::domain::lazy_files(entries.clone()));
    let eager = commit(Value::list(entries.clone()));
    let s = show(&i, &lazy);
    assert_eq!(s, show(&i, &eager));
    assert!(
        s.lines().all(|l| j::render::width(l) <= 80),
        "files list left on one line: {}",
        s
    );
    // the broken-up list still reads back (the id above names no commit of
    // the in-memory backend, so round-trip the files alone)
    roundtrip(&mut i, &cfg, &Value::record(&[("files", j::domain::lazy_files(entries))]));
}

#[test]
fn show_fits_by_display_width() {
    // "fits in 80 columns" counts display cells, not code points (§5.1): 55
    // characters of which 45 are wide is 100 columns and must break
    let (mut i, cfg) = make_interp();
    let wide = Value::list(vec![Value::text("日本語日本語日本語日本語日本語"); 3]);
    let s = show(&i, &wide);
    assert!(s.contains('\n'), "{}", s);
    roundtrip(&mut i, &cfg, &wide);
    // two of them are 67 columns and stay on one line
    let narrow = Value::list(vec![Value::text("日本語日本語日本語日本語日本語"); 2]);
    assert!(!show(&i, &narrow).contains('\n'));
}

#[test]
fn show_partial_application() {
    let (mut i, cfg) = make_interp();
    let outer = Rc::new(cfg.global_names.clone());
    let e = parse_expr("describe \"wip\"", outer).unwrap();
    let env = i.global_env();
    let v = i.eval(&Rc::new(e), &env).unwrap();
    let s = show(&i, &v);
    assert_eq!(s, "describe \"wip\"", "{}", s);
}

#[test]
fn show_id_full() {
    let (i, _cfg) = make_interp();
    let v = Value::Id(Rc::new("wqztkpqxmnrvyxskptlmzzzzabcd".to_string()));
    assert_eq!(show(&i, &v), "@wqztkpqxmnrvyxskptlmzzzzabcd");
}

// ------------------------------------------------------------------
// unified diff
// ------------------------------------------------------------------

#[test]
fn unified_diff_basic() {
    let d = unified_diff("a\nb\nc\n", "a\nx\nc\n");
    assert!(d.contains("-b"), "{}", d);
    assert!(d.contains("+x"), "{}", d);
    assert!(d.contains("@@"), "{}", d);
    // context lines present
    assert!(d.contains(" a"), "{}", d);
    // no header lines
    assert!(!d.contains("---"), "{}", d);
    assert!(!d.contains("+++"), "{}", d);
}

#[test]
fn unified_diff_empty_for_equal() {
    assert_eq!(unified_diff("same\n", "same\n"), "");
    assert_eq!(unified_diff("", ""), "");
}

#[test]
fn unified_diff_additions_and_deletions() {
    let d = unified_diff("", "new\n");
    assert!(d.contains("+new"), "{}", d);
    let d = unified_diff("old\n", "");
    assert!(d.contains("-old"), "{}", d);
}

#[test]
fn unified_diff_no_trailing_newline() {
    // a last line without a trailing newline must not produce spurious blank
    // lines in the output
    let d = unified_diff("a\nb", "a\nc");
    assert!(d.contains("-b\n"), "{}", d);
    assert!(d.contains("+c\n"), "{}", d);
    assert!(!d.contains("\n\n"), "spurious blank line: {:?}", d);
    // and neither does one with a trailing newline
    let with_nl = unified_diff("a\nb\n", "a\nc\n");
    assert!(!with_nl.contains("\n\n"), "{:?}", with_nl);
}

#[test]
fn unified_diff_marks_missing_newline() {
    // a line with no terminator is followed by the marker GNU diff and git
    // print; without it, dropping or adding the final newline shows as the
    // same line removed and re-added, and both directions look alike
    const MARK: &str = "\\ No newline at end of file\n";
    assert_eq!(
        unified_diff("a\nb\n", "a\nb"),
        format!("@@ -1,2 +1,2 @@\n a\n-b\n+b\n{}", MARK)
    );
    assert_eq!(
        unified_diff("a\nb", "a\nb\n"),
        format!("@@ -1,2 +1,2 @@\n a\n-b\n{}+b\n", MARK)
    );
    assert_eq!(
        unified_diff("a\nb", "a\nc"),
        format!("@@ -1,2 +1,2 @@\n a\n-b\n{}+c\n{}", MARK, MARK)
    );
    // an unterminated context line is marked too
    assert_eq!(
        unified_diff("a\nb", "x\nb"),
        format!("@@ -1,2 +1,2 @@\n-a\n+x\n b\n{}", MARK)
    );
    // terminated lines get no marker
    assert!(!unified_diff("a\nb\n", "a\nc\n").contains("No newline"));
}

#[test]
fn unified_diff_context_radius() {
    // 3 lines of context: a change far from the ends shows 3 before/after
    let a = (1..=10).map(|n| format!("{}\n", n)).collect::<String>();
    let b = a.replace("5\n", "five\n");
    let d = unified_diff(&a, &b);
    assert!(d.contains(" 2"), "{}", d); // 3 lines before
    assert!(!d.contains(" 1\n"), "{}", d); // 4th line before not shown
    assert!(d.contains(" 8"), "{}", d); // 3 lines after
    assert!(!d.contains(" 9"), "{}", d);
}

// ------------------------------------------------------------------
// conflict marker rendering (§7.4 materialisation)
// ------------------------------------------------------------------

#[test]
fn conflict_blob_markers() {
    let v = Value::Blob(Rc::new(BlobVal {
        kind: BlobKind::Regular,
        content: BlobContent::Conflict(vec![
            Rc::new(b"ours\n".to_vec()),
            Rc::new(b"base\n".to_vec()),
            Rc::new(b"theirs\n".to_vec()),
        ]),
    }));
    let Value::Blob(b) = &v else { panic!() };
    let text = String::from_utf8(b.bytes().unwrap()).unwrap();
    assert!(text.contains("<<<<<<<"), "{}", text);
    assert!(text.contains(">>>>>>>"), "{}", text);
    assert!(text.contains("+++++++"), "{}", text);
    assert!(text.contains("%%%%%%%"), "{}", text);
    assert!(text.contains("ours") && text.contains("base") && text.contains("theirs"));
}

#[test]
fn blob_sizes() {
    let v = BlobVal::text_blob("12345");
    let Value::Blob(b) = &v else { panic!() };
    assert_eq!(b.size(), 5);
    let c = Value::Blob(Rc::new(BlobVal {
        kind: BlobKind::Regular,
        content: BlobContent::Conflict(vec![Rc::new(b"ab".to_vec())]),
    }));
    let Value::Blob(b) = &c else { panic!() };
    assert!(b.is_unresolved());
    assert_eq!(b.size(), 2);
}

#[test]
fn width_counts_wide_chars() {
    assert_eq!(j::render::width("abc"), 3);
    assert_eq!(j::render::width("日本語"), 6);
    assert_eq!(j::render::width("a日b"), 4);
    // ansi escapes not counted
    assert_eq!(j::render::width("\x1b[31mred\x1b[0m"), 3);
}

#[test]
fn age_rendering() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert_eq!(j::render::render_age(now - 5), "5s");
    assert_eq!(j::render::render_age(now - 120), "2m");
    assert_eq!(j::render::render_age(now - 7200), "2h");
    assert_eq!(j::render::render_age(now - 3 * 86400), "3d");
    assert_eq!(j::render::render_age(now - 14 * 86400), "2w");
    assert_eq!(j::render::render_age(now - 400 * 86400), "1y");
}

#[test]
fn color_respects_no_color() {
    unsafe { std::env::set_var("NO_COLOR", "1") };
    assert!(!j::render::color_enabled("always"));
    assert!(!j::render::color_enabled("auto"));
    unsafe { std::env::remove_var("NO_COLOR") };
    assert!(j::render::color_enabled("always"));
    assert!(!j::render::color_enabled("never"));
}

#[test]
fn render_date_matches_utc_calendar() {
    // civil-from-days, no date crate: check epochs, leap days, century rules
    for (t, want) in [
        (0i64, "1970-01-01"),
        (86_399, "1970-01-01"),
        (86_400, "1970-01-02"),
        (951_782_400, "2000-02-29"),   // leap year divisible by 400
        (1_078_012_800, "2004-02-29"), // ordinary leap year
        (4_107_542_400, "2100-03-01"), // 2100 is not a leap year
        (1_700_000_000, "2023-11-14"),
        (2_147_483_647, "2038-01-19"),
        (-1, "1969-12-31"),            // before the epoch
        (-86_400, "1969-12-31"),
        (-86_401, "1969-12-30"),
    ] {
        assert_eq!(j::render::render_date(t), want, "t={}", t);
    }
}

#[test]
fn unique_prefix_is_shortest_and_unambiguous() {
    // computed from sorted neighbours rather than a scan of every id; the
    // answers must match the definition: shortest prefix no other id shares,
    // minimum four characters
    let ids: Vec<String> = [
        "kkkkllll", "kkkkmmmm", "kkkmnnnn", "lllloooo", "zzzzzzzz",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let p = |id: &str| j::eval::unique_prefix_in(&ids, id);
    // shares "kkkk" with kkkkmmmm, so needs five
    assert_eq!(p("kkkkllll"), "kkkkl");
    assert_eq!(p("kkkkmmmm"), "kkkkm");
    // differs from the others by the fourth character: the minimum applies
    assert_eq!(p("kkkmnnnn"), "kkkm");
    assert_eq!(p("lllloooo"), "llll");
    assert_eq!(p("zzzzzzzz"), "zzzz");
    // an id not in the set is measured against the set all the same:
    // "kkkkll" would still be a prefix of kkkkllll, so it needs seven
    assert_eq!(p("kkkkllmm"), "kkkkllm");
    // and every answer really is unique among the others
    for id in &ids {
        let pre = p(id);
        assert!(pre.len() >= 4, "{} -> {}", id, pre);
        let sharers = ids.iter().filter(|o| *o != id && o.starts_with(&pre)).count();
        assert_eq!(sharers, 0, "{} -> {} is shared", id, pre);
        // and it is the shortest such prefix
        if pre.len() > 4 {
            let shorter = &id[..pre.len() - 1];
            assert!(
                ids.iter().any(|o| o != id && o.starts_with(shorter)),
                "{} -> {} is longer than needed",
                id,
                pre
            );
        }
    }
    // an empty set still respects the minimum
    assert_eq!(j::eval::unique_prefix_in(&[], "kkkkllll"), "kkkk");
}
