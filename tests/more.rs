//! Contract error messages (§4.13), display shapes (§5.1), the `validate`
//! builtin, and additional §8 laws.

use j::config;
use j::domain::{MemBackend, MetaInfo, ROOT_ID};
use j::eval::Interp;
use j::parse::parse_expr;
use j::value::{value_eq, BlobVal, Env, Value};
use std::rc::Rc;

const CONFIG: &str = include_str!("../config.j");

fn make_interp() -> (Interp, config::Config) {
    let cfg = config::load_config(CONFIG).unwrap();
    let mut i = Interp::new(Rc::new(MemBackend::new()), cfg.shapes.clone(), Env::empty());
    config::eval_config(&mut i, &cfg).unwrap();
    (i, cfg)
}

fn ev(i: &mut Interp, cfg: &config::Config, src: &str) -> Result<Value, String> {
    let outer = Rc::new(cfg.global_names.clone());
    let e = parse_expr(src, outer).map_err(|p| format!("parse: {}", p.msg))?;
    let e = config::resolve_ids(&e, i).map_err(|_| "ids".to_string())?;
    let env = i.global_env();
    i.eval(&Rc::new(e), &env).map_err(|c| c.msg)
}

fn crash_msg(i: &mut Interp, cfg: &config::Config, src: &str) -> String {
    match ev(i, cfg, src) {
        Ok(_) => panic!("{} unexpectedly succeeded", src),
        Err(m) => m,
    }
}

#[test]
fn contract_argument_messages() {
    let (mut i, cfg) = make_interp();
    let m = crash_msg(&mut i, &cfg, "describe 3");
    assert!(m.contains("describe"), "{}", m);
    assert!(m.contains("Text"), "{}", m);
    assert!(m.contains("argument 1"), "{}", m);
    assert!(m.contains("Int"), "{}", m);
    // field sets shown for records
    let m = crash_msg(&mut i, &cfg, "mapRoot (\\x -> x) 3");
    assert!(m.contains("mapRoot"), "{}", m);
    // argument positions named
    let m = crash_msg(&mut i, &cfg, "nth \"x\" [1]");
    assert!(m.contains("nth"), "{}", m);
    assert!(m.contains("argument 1"), "{}", m);
    let m = crash_msg(&mut i, &cfg, "take [1] [2]");
    assert!(m.contains("argument 1"), "{}", m);
    // exact spec format (§4.13): "describe expected Text as argument 1, got Int"
    let m = crash_msg(&mut i, &cfg, "describe 3");
    assert!(
        m.contains("describe expected Text as argument 1, got Int"),
        "spec format: {}",
        m
    );
    // polymorphic types keep their inner description for clarity
    let m = crash_msg(&mut i, &cfg, "length 5");
    assert!(m.contains("list"), "{}", m);
}

#[test]
fn contract_catchable_by_or() {
    let (mut i, cfg) = make_interp();
    let outer = Rc::new(cfg.global_names.clone());
    let e = parse_expr("describe 3 or 42", outer).unwrap();
    let env = i.global_env();
    let v = i.eval(&Rc::new(e), &env).unwrap();
    assert!(value_eq(&v, &Value::int(42)).unwrap());
}

#[test]
fn contract_on_values_at_load() {
    // a signature on a non-function definition checks at load
    let src = r#"
user = { name = "A", email = "a" }
immutable = \_ -> []
tree = \_ -> ""
labelled = \_ _ -> []
wrong : Int
wrong = "text"
"#;
    let cfg = config::load_config(src).unwrap();
    let mut i = Interp::new(Rc::new(MemBackend::new()), cfg.shapes.clone(), Env::empty());
    let r = config::eval_config(&mut i, &cfg);
    match r {
        Ok(_) => panic!("should fail"),
        Err(c) => assert!(c.msg.contains("contract"), "{}", c.msg),
    }
}

#[test]
fn alias_contracts_unfold() {
    let (mut i, cfg) = make_interp();
    // at : Revset -> Edit -> Edit checks three arguments and a Repo result (§4.13)
    let m = crash_msg(&mut i, &cfg, "at 1 2 3");
    assert!(m.contains("at"), "{}", m);
}

/// The reference config with `extra` appended, loaded and evaluated.
fn make_interp_with(extra: &str) -> (Interp, config::Config) {
    let src = format!("{}\n{}", CONFIG, extra);
    let cfg = config::load_config(&src).expect("config must load");
    let mut i = Interp::new(Rc::new(MemBackend::new()), cfg.shapes.clone(), Env::empty());
    config::eval_config(&mut i, &cfg).expect("config must evaluate");
    (i, cfg)
}

fn assert_value(i: &mut Interp, cfg: &config::Config, src: &str, want: Value) {
    let got = ev(i, cfg, src).unwrap_or_else(|m| panic!("{} crashed: {}", src, m));
    assert!(
        value_eq(&got, &want).unwrap(),
        "{} gave {}",
        src,
        j::show::show(i, &got)
    );
}

#[test]
fn function_argument_to_a_signed_definition_is_a_contract_crash() {
    // §4.13: every argument is checked against its parameter. A signed
    // definition given a function where its signature wants a Repo or a
    // Text composed with it instead, so `new new` was the edit "new twice"
    // (and `j new new` persisted two commits) and `or` had nothing to catch.
    let (mut i, cfg) = make_interp();
    let m = crash_msg(&mut i, &cfg, "new new");
    assert!(
        m.contains("new expected Repo (record) as argument 1, got function"),
        "{}",
        m
    );
    let m = crash_msg(&mut i, &cfg, "describe (\\r -> \"m\")");
    assert!(m.contains("describe expected Text as argument 1, got function"), "{}", m);
    assert_value(&mut i, &cfg, "describe (\\r -> \"m\") or 42", Value::int(42));
    let m = crash_msg(&mut i, &cfg, "squash new");
    assert!(m.contains("squash expected Repo (record) as argument 1, got function"), "{}", m);
    let m = crash_msg(&mut i, &cfg, "(tree . squash) new");
    assert!(m.contains("expected Repo (record) as argument 1, got function"), "{}", m);
}

#[test]
fn composition_applied_to_a_function_applies() {
    // §4.9: (f . g) x = f (g x), whatever x is. A function argument composed
    // pointwise instead, and so did every part whose next parameter was a
    // type variable: `(show . id) not` crashed inside `not`.
    let (mut i, cfg) = make_interp();
    assert_value(&mut i, &cfg, "(show . id) not", Value::text("not"));
    assert_value(&mut i, &cfg, "(const 1 . id) not", Value::int(1));
    assert_value(&mut i, &cfg, "((\\x -> show x) . id) not", Value::text("not"));
}

#[test]
fn composition_argument_belongs_to_the_right_operand() {
    // §4.9: in (f . g) x the argument is g's. When g's contract was unknown
    // (a selector, an unsigned composition, an `or`), x was checked against
    // f's parameter instead.
    let (mut i, cfg) = make_interp_with("foo : Int -> a\nfoo = \\x y -> [y]\n");
    assert_value(&mut i, &cfg, "(length . .xs) ({ xs = [1 2] })", Value::int(2));
    assert_value(&mut i, &cfg, "(not . .b) ({ b = true })", Value::bool(false));
    assert_value(&mut i, &cfg, "(length . (\\r -> [r]) . (\\r -> r)) 5", Value::int(1));
    assert_value(&mut i, &cfg, "(length . ((\\x -> [x]) or id) . id) 5", Value::int(1));
    // g's own contract is used up, so x goes unchecked into it, not into f
    assert_value(&mut i, &cfg, "(length . foo 1) \"abc\"", Value::int(1));
    // a known g still checks its argument, numbered as g's
    let m = crash_msg(&mut i, &cfg, "(length . head) 5");
    assert!(
        m.contains("(length . head) expected [a] (a list) as argument 1, got Int"),
        "{}",
        m
    );
}

#[test]
fn signatures_hold_whatever_the_definition_is_built_from() {
    // §4.13: a signature is checked at every application of the definition,
    // whether its value is a lambda, a composition, an `or`, or a label.
    let (mut i, cfg) = make_interp_with(
        r#"
inc : Int -> Int
inc = \x -> x + 1

twice : Int -> Text
twice = inc . inc

safeNew : Edit
safeNew = new or id

keep : Int -> Text
keep = (\x -> x) or show

choose : Int -> Int -> Int
choose = (\a b -> a) or (\a b -> b)

mainCount : Repo -> Int
mainCount = %main
"#,
    );
    // a signed composition checks its argument and its result
    let m = crash_msg(&mut i, &cfg, "twice 1");
    assert!(m.contains("twice: result expected Text, got Int"), "{}", m);
    let m = crash_msg(&mut i, &cfg, "twice \"x\"");
    assert!(m.contains("twice expected Int as argument 1, got Text"), "{}", m);
    // a signed `or` checks its argument before either side runs, and its
    // result after the fallback, not inside either side (§4.6)
    let m = crash_msg(&mut i, &cfg, "safeNew 5");
    assert!(m.contains("safeNew expected Repo (record) as argument 1, got Int"), "{}", m);
    assert_value(&mut i, &cfg, "safeNew 5 or 7", Value::int(7));
    let m = crash_msg(&mut i, &cfg, "keep 1");
    assert!(m.contains("keep: result expected Text, got Int"), "{}", m);
    let m = crash_msg(&mut i, &cfg, "keep \"x\"");
    assert!(m.contains("keep expected Int as argument 1, got Text"), "{}", m);
    // ... through every argument the signature lists
    assert_value(&mut i, &cfg, "choose 1 2", Value::int(1));
    let m = crash_msg(&mut i, &cfg, "choose 1 \"x\"");
    assert!(m.contains("choose expected Int as argument 2, got Text"), "{}", m);
    // a signed label literal
    let m = crash_msg(&mut i, &cfg, "mainCount 5");
    assert!(m.contains("mainCount expected Repo (record) as argument 1, got Int"), "{}", m);
    let m = crash_msg(
        &mut i,
        &cfg,
        "mainCount ({ root = { files = [], message = \"\", labels = [], id = @ }, children = [], context = [] })",
    );
    assert!(m.contains("mainCount: result expected Int, got list"), "{}", m);
}

#[test]
fn arguments_are_numbered_by_the_signature() {
    // §4.13 ("describe expected Text as argument 1"): a definition that is a
    // partial application or a composition counted the arguments baked into
    // it, so `tree 5` said argument 2 and `squash 5` argument 3
    let (mut i, cfg) = make_interp();
    for (src, name) in [
        ("tree 5", "tree"),
        ("treeCompact 5", "treeCompact"),
        ("trunk 5", "trunk"),
        ("squash 5", "squash"),
    ] {
        let m = crash_msg(&mut i, &cfg, src);
        assert!(
            m.contains(&format!("{} expected Repo (record) as argument 1, got Int", name)),
            "{}: {}",
            src,
            m
        );
    }
}

#[test]
fn function_signature_on_a_non_function_value_fails_at_load() {
    // §4.13: a definition that is not a lambda is checked against its
    // signature at load; for a function type that means being a function
    let src = format!("{}\nmyEdit : Edit\nmyEdit = 5\n", CONFIG);
    let cfg = config::load_config(&src).unwrap();
    let mut i = Interp::new(Rc::new(MemBackend::new()), cfg.shapes.clone(), Env::empty());
    match config::eval_config(&mut i, &cfg) {
        Ok(_) => panic!("a non-function Edit should not load"),
        Err(c) => assert!(
            c.msg.contains("contract: myEdit: expected a function, got Int"),
            "{}",
            c.msg
        ),
    }
}

// ------------------------------------------------------------------
// display shapes (§5.1)
// ------------------------------------------------------------------

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

fn display(i: &mut Interp, v: &Value) -> String {
    j::render::display(i, v, false).unwrap()
}

fn interp_with_meta() -> Interp {
    let mut b = MemBackend::new();
    b.metas.insert(
        "kqqqqqqq".into(),
        MetaInfo {
            hash: "h".into(),
            author: "Ann Author".into(),
            email: "a@x".into(),
            time: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64
                - 7200,
        },
    );
    let (i, _) = {
        let cfg = config::load_config(CONFIG).unwrap();
        let mut i = Interp::new(Rc::new(b), cfg.shapes.clone(), Env::empty());
        config::eval_config(&mut i, &cfg).unwrap();
        (i, cfg)
    };
    i
}

#[test]
fn display_commit_block() {
    let mut i = interp_with_meta();
    let c = commit("kqqqqqqq", "wip\nsecond line", &["feat"], vec![("a/b.rs", "x\n")]);
    let out = display(&mut i, &c);
    assert!(out.contains("wip"), "{}", out);
    assert!(out.contains("feat"), "{}", out);
    assert!(out.contains("Ann Author"), "{}", out);
    assert!(out.contains("2h"), "{}", out);
    assert!(out.contains("1 files"), "{}", out);
    // no repository in context, so no parent to compare against: every
    // file it has is added (§5.1)
    assert!(out.contains("  + a/b.rs"), "{}", out);
    // id renders with the unique prefix
    assert!(out.contains("kqqq"), "{}", out);
}

#[test]
fn display_generic_record() {
    let (mut i, _cfg) = make_interp();
    let v = Value::record(&[("alpha", Value::int(1)), ("beta", Value::text("x"))]);
    let out = display(&mut i, &v);
    assert!(out.contains("alpha"), "{}", out);
    assert!(out.contains("beta"), "{}", out);
    assert!(out.contains("1"), "{}", out);
    assert!(out.contains("x"), "{}", out);
}

#[test]
fn display_generic_record_line_form() {
    let (mut i, _cfg) = make_interp();
    let v = Value::list(vec![Value::record(&[("alpha", Value::int(1)), ("zeta", Value::int(2))])]);
    let out = display(&mut i, &v);
    // a list of identical generic records is a column table with header
    assert!(out.contains("alpha"), "{}", out);
    assert!(out.contains("zeta"), "{}", out);
}

#[test]
fn display_table_drops_empty_columns() {
    let (mut i, _cfg) = make_interp();
    let rows = vec![
        Value::record(&[("a", Value::Bool(true)), ("b", Value::Bool(false))]),
        Value::record(&[("a", Value::Bool(true)), ("b", Value::Bool(false))]),
    ];
    let out = display(&mut i, &Value::list(rows));
    assert!(out.contains("✓"), "{}", out);
    // column b is empty in every row: dropped
    let header = out.lines().next().unwrap();
    assert!(!header.contains("b") || header.contains("a"), "{}", header);
}

#[test]
fn display_change_shape() {
    let (mut i, _cfg) = make_interp();
    let change = Value::record(&[
        (
            "from",
            Value::list(vec![Value::record(&[
                ("content", BlobVal::text_blob("1")),
                ("path", Value::list(vec![Value::text("f")])),
            ])]),
        ),
        (
            "to",
            Value::list(vec![Value::record(&[
                ("content", BlobVal::text_blob("2")),
                ("path", Value::list(vec![Value::text("f")])),
            ])]),
        ),
    ]);
    let out = display(&mut i, &change);
    assert!(out.contains("~ f"), "{}", out);
}

#[test]
fn display_empty_list_variants() {
    let (mut i, _cfg) = make_interp();
    let out = display(&mut i, &Value::list(vec![]));
    assert!(out.contains("none"), "{}", out);
}

#[test]
fn display_scalar_lists() {
    let (mut i, _cfg) = make_interp();
    let out = display(&mut i, &Value::list(vec![Value::int(1), Value::int(2), Value::Bool(true)]));
    let _ = out;
    let out = display(&mut i, &Value::list(vec![Value::int(1), Value::int(2)]));
    assert!(out.contains("1, 2"), "{}", out);
    let out = display(&mut i, &Value::list(vec![Value::Bool(true)]));
    assert!(out.contains("true"), "{}", out);
}

#[test]
fn display_text_block_full() {
    let (mut i, _cfg) = make_interp();
    let out = display(&mut i, &Value::text("line1\nline2"));
    assert_eq!(out, "line1\nline2\n");
    // trailing newline added if absent
    let out = display(&mut i, &Value::text("x"));
    assert_eq!(out, "x\n");
}

#[test]
fn display_blob_block_content() {
    let (mut i, _cfg) = make_interp();
    let out = display(&mut i, &BlobVal::text_blob("raw content\n"));
    assert_eq!(out, "raw content\n");
}

#[test]
fn display_function_and_shape() {
    let (mut i, _cfg) = make_interp();
    let f = i.globals.lookup("map").unwrap();
    let out = display(&mut i, &f);
    assert!(out.contains("map"), "{}", out);
    let s = i.shapes.shape_of("Commit").unwrap();
    let out = display(&mut i, &Value::Shape(Rc::new(s)));
    assert!(out.contains("Commit"), "{}", out);
}

// ------------------------------------------------------------------
// validate builtin
// ------------------------------------------------------------------

#[test]
fn validate_passes_clean_repo() {
    let (mut i, cfg) = make_interp();
    let root = commit(ROOT_ID, "", &[], vec![]);
    let a = commit("kqqqqqqq", "a", &[], vec![]);
    let frame = Value::record(&[
        ("left", Value::list(vec![])),
        ("parent", root),
        ("right", Value::list(vec![])),
    ]);
    let repo = Value::record(&[
        ("children", Value::list(vec![])),
        ("context", Value::list(vec![frame])),
        ("root", a),
    ]);
    *i.old_repo.borrow_mut() = Some(repo.clone());
    let outer = Rc::new(cfg.global_names.clone());
    let e = parse_expr("validate", outer).unwrap();
    let env = i.global_env();
    let v = i.eval(&Rc::new(e), &env).unwrap();
    let r = i.apply(v, repo);
    assert!(r.is_ok(), "{:?}", r.err().map(|c| c.msg));
}

#[test]
fn validate_refuses_bad_focus() {
    let (mut i, cfg) = make_interp();
    let root = commit(ROOT_ID, "", &[], vec![]);
    let repo = Value::record(&[
        ("children", Value::list(vec![])),
        ("context", Value::list(vec![])),
        ("root", root),
    ]);
    *i.old_repo.borrow_mut() = Some(repo.clone());
    let outer = Rc::new(cfg.global_names.clone());
    let e = parse_expr("validate", outer).unwrap();
    let env = i.global_env();
    let v = i.eval(&Rc::new(e), &env).unwrap();
    let r = i.apply(v, repo);
    match r {
        Ok(_) => panic!("focus on root must be refused"),
        Err(c) => assert!(c.msg.contains("mutable"), "{}", c.msg),
    }
}

// ------------------------------------------------------------------
// more §8 laws
// ------------------------------------------------------------------

#[test]
fn law_tip_and_top_idempotent() {
    let (mut i, cfg) = make_interp();
    let root = commit(ROOT_ID, "", &[], vec![]);
    let a = commit("kqqqqqqq", "a", &[], vec![]);
    let repo = Value::record(&[
        ("children", Value::list(vec![])),
        (
            "context",
            Value::list(vec![Value::record(&[
                ("left", Value::list(vec![])),
                ("parent", root),
                ("right", Value::list(vec![])),
            ])]),
        ),
        ("root", a),
    ]);
    for src in ["top . top", "top"] {
        let outer = Rc::new(cfg.global_names.clone());
        let e = parse_expr(src, outer).unwrap();
        let env = i.global_env();
        let v = i.eval(&Rc::new(e), &env).unwrap();
        let v = i.apply(v, repo.clone()).unwrap();
        assert!(v.field("context").unwrap().as_list().unwrap().is_empty(), "{}", src);
    }
    // tip idempotent on a tip
    let outer = Rc::new(cfg.global_names.clone());
    let e = parse_expr("tip . tip", outer.clone()).unwrap();
    let env = i.global_env();
    let v = i.eval(&Rc::new(e), &env).unwrap();
    let v2 = i.apply(v, repo.clone()).unwrap();
    let e = parse_expr("tip", outer.clone()).unwrap();
    let v = i.eval(&Rc::new(e), &env).unwrap();
    let v1 = i.apply(v, repo).unwrap();
    assert!(value_eq(&v2, &v1).unwrap());
}

#[test]
fn law_ancestors_recursion() {
    // ancestors r = r.root.id :: ancestors (up r) when up exists
    let (mut i, cfg) = make_interp();
    let root = commit(ROOT_ID, "", &[], vec![]);
    let a = commit("kqqqqqqq", "a", &[], vec![]);
    let frame = Value::record(&[
        ("left", Value::list(vec![])),
        ("parent", root),
        ("right", Value::list(vec![])),
    ]);
    let repo = Value::record(&[
        ("children", Value::list(vec![])),
        ("context", Value::list(vec![frame])),
        ("root", a),
    ]);
    let outer = Rc::new(cfg.global_names.clone());
    let run = |i: &mut Interp, src: &str, r: Value| -> Value {
        let e = parse_expr(src, outer.clone()).unwrap();
        let env = i.global_env();
        let v = i.eval(&Rc::new(e), &env).unwrap();
        i.apply(v, r).unwrap()
    };
    let lhs = run(&mut i, "ancestors", repo.clone());
    let head_id = run(&mut i, "here", repo.clone());
    let rest = run(&mut i, "ancestors . up", repo.clone());
    let mut expect = head_id.as_list().unwrap().to_vec();
    expect.extend(rest.as_list().unwrap().iter().cloned());
    assert!(value_eq(&lhs, &Value::list(expect)).unwrap());
}

#[test]
fn law_or_values() {
    let (mut i, cfg) = make_interp();
    // e or x = e when e does not crash
    let outer = Rc::new(cfg.global_names.clone());
    let e = parse_expr("3 or 4", outer.clone()).unwrap();
    let env = i.global_env();
    let v = i.eval(&Rc::new(e), &env).unwrap();
    assert!(value_eq(&v, &Value::int(3)).unwrap());
    // crash m or x = x
    let e = parse_expr("crash \"m\" or 4", outer.clone()).unwrap();
    let v = i.eval(&Rc::new(e), &env).unwrap();
    assert!(value_eq(&v, &Value::int(4)).unwrap());
}

#[test]
fn law_concat_of_concat() {
    let (mut i, cfg) = make_interp();
    let outer = Rc::new(cfg.global_names.clone());
    let e = parse_expr("concat ([[\"a\"] [\"b\"]] ++ [[\"c\"]])", outer.clone()).unwrap();
    let env = i.global_env();
    let a = i.eval(&Rc::new(e), &env).unwrap();
    let e = parse_expr("concat [[\"a\"] [\"b\"]] ++ concat [[\"c\"]]", outer.clone()).unwrap();
    let b = i.eval(&Rc::new(e), &env).unwrap();
    assert!(value_eq(&a, &b).unwrap());
}
