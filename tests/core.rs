//! Smoke tests for the interpreter core (no repository needed).

use j::config;
use j::domain::MemBackend;
use j::eval::Interp;
use j::parse::parse_expr;
use j::value::{value_eq, Env, Value};
use std::rc::Rc;

const CONFIG: &str = include_str!("../config.j");

fn make_interp() -> (Interp, config::Config) {
    let cfg = config::load_config(CONFIG).expect("reference config must load");
    let backend = Rc::new(MemBackend::new());
    let mut interp = Interp::new(backend, cfg.shapes.clone(), Env::empty());
    config::eval_config(&mut interp, &cfg).expect("reference config must evaluate");
    (interp, cfg)
}

fn eval_str(interp: &mut Interp, cfg: &config::Config, src: &str) -> Result<Value, String> {
    let outer = Rc::new(cfg.global_names.clone());
    let e = parse_expr(src, outer).map_err(|p| format!("parse: {}", p.msg))?;
    let e = config::resolve_ids(&e, interp).map_err(|_| "id resolution".to_string())?;
    let env = interp.global_env();
    interp
        .eval(&Rc::new(e), &env)
        .map_err(|c| format!("crash: {}", c.msg))
}

fn show_of(i: &Interp, v: &Value) -> String {
    j::show::show(i, v).unwrap()
}

#[track_caller]
fn assert_val(i: &Interp, got: Value, want: Value) {
    assert!(
        value_eq(&got, &want).unwrap_or(false),
        "values differ:\n  got:  {}\n  want: {}",
        show_of(i, &got),
        show_of(i, &want)
    );
}

macro_rules! check {
    ($i:expr, $cfg:expr, $src:expr, $want:expr) => {{
        let got = eval_str(&mut $i, &$cfg, $src).unwrap();
        assert_val(&$i, got, $want);
    }};
}

#[test]
fn reference_config_parses_and_loads() {
    make_interp();
}

#[test]
fn arithmetic_and_lists() {
    let (mut i, cfg) = make_interp();
    check!(i, cfg, "1 + 2 * 3", Value::int(7));
    check!(
        i,
        cfg,
        "map (\\x -> x * 2) [1 2 3]",
        Value::list(vec![Value::int(2), Value::int(4), Value::int(6)])
    );
    check!(i, cfg, "foldl (+) 0 (range 0 10)", Value::int(45));
}

#[test]
fn text_builtins() {
    let (mut i, cfg) = make_interp();
    check!(
        i,
        cfg,
        r#"splitOn "/" "a/b/c""#,
        Value::list(vec![Value::text("a"), Value::text("b"), Value::text("c")])
    );
    check!(i, cfg, r#""ab" ++ "cd""#, Value::text("abcd"));
}

#[test]
fn or_catches_crashes() {
    let (mut i, cfg) = make_interp();
    check!(i, cfg, "head [] or 42", Value::int(42));
    check!(i, cfg, r#"crash "boom" or 7"#, Value::int(7));
    check!(i, cfg, "1 or 2", Value::int(1));
}

#[test]
fn sections_and_composition() {
    let (mut i, cfg) = make_interp();
    check!(
        i,
        cfg,
        "map (1 +) [1 2]",
        Value::list(vec![Value::int(2), Value::int(3)])
    );
    check!(
        i,
        cfg,
        "map (+ 1) [1 2]",
        Value::list(vec![Value::int(2), Value::int(3)])
    );
    check!(i, cfg, "(not . null) [1]", Value::Bool(true));
}

#[test]
fn show_roundtrip() {
    let (mut i, cfg) = make_interp();
    let v = eval_str(
        &mut i,
        &cfg,
        r#"{ a = [1 2], b = "x\n", c = { d = true } }"#,
    )
    .unwrap();
    let s = eval_str(&mut i, &cfg, "show ({ a = [1 2], b = \"x\\n\", c = { d = true } })").unwrap();
    let s = s.as_text().unwrap().to_string();
    let outer = Rc::new(cfg.global_names.clone());
    let e = parse_expr(&s, outer).expect("show output must parse");
    let env = i.global_env();
    let v2 = i.eval(&Rc::new(e), &env).expect("show output must evaluate");
    assert!(
        value_eq(&v, &v2).unwrap(),
        "show roundtrip failed for {}",
        s
    );
}

#[test]
fn deep_recursion_does_not_overflow() {
    let (mut i, cfg) = make_interp();
    let r = eval_str(
        &mut i,
        &cfg,
        "let go = \\n acc -> if n == 0 then acc else go (n - 1) (acc + n) in go 20000 0",
    )
    .unwrap();
    assert_val(&i, r, Value::int(20000 * 20001 / 2));
}

#[test]
fn contracts_are_enforced() {
    let (mut i, cfg) = make_interp();
    let r = eval_str(&mut i, &cfg, "describe 3");
    let msg = match r {
        Ok(_) => panic!("describe 3 must be a contract crash"),
        Err(m) => m,
    };
    assert!(msg.contains("contract"), "got: {}", msg);
}

#[test]
fn selector_and_update() {
    let (mut i, cfg) = make_interp();
    check!(
        i,
        cfg,
        "map (.path) [{ path = [\"a\"], content = blob \"x\" }]",
        Value::list(vec![Value::list(vec![Value::text("a")])])
    );
    let got = eval_str(&mut i, &cfg, "({ a = 1, b = 2 }) { a = 9 }").unwrap();
    let want = eval_str(&mut i, &cfg, "{ a = 9, b = 2 }").unwrap();
    assert_val(&i, got, want);
}

#[test]
fn extract_works() {
    let (mut i, cfg) = make_interp();
    let got = eval_str(
        &mut i,
        &cfg,
        "length (extract Commit ({ root = { files = [], message = \"\", labels = [], id = @ }, children = [] }))",
    )
    .unwrap();
    assert_val(&i, got, Value::int(1));
}
