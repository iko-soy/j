//! Builtin function tests (§4.9, §4.8).

use j::config;
use j::domain::MemBackend;
use j::eval::Interp;
use j::parse::parse_expr;
use j::value::{value_eq, Env, Value};
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

fn ev(interp: &mut Interp, cfg: &config::Config, src: &str) -> Result<Value, String> {
    let outer = Rc::new(cfg.global_names.clone());
    let e = parse_expr(src, outer).map_err(|p| format!("parse: {}", p.msg))?;
    let e = config::resolve_ids(&e, interp).map_err(|_| "id resolution".to_string())?;
    let env = interp.global_env();
    interp
        .eval(&Rc::new(e), &env)
        .map_err(|c| format!("crash: {}", c.msg))
}

fn ok(i: &mut Interp, cfg: &config::Config, src: &str) -> Value {
    ev(i, cfg, src).unwrap_or_else(|e| panic!("{} => {}", src, e))
}

fn crash(i: &mut Interp, cfg: &config::Config, src: &str) -> String {
    match ev(i, cfg, src) {
        Ok(v) => panic!("{} unexpectedly succeeded: {}", src, j::show::show(i, &v).unwrap()),
        Err(m) => m,
    }
}

macro_rules! check {
    ($i:expr, $cfg:expr, $src:expr, $want:expr) => {{
        let got = ok(&mut $i, &$cfg, $src);
        assert!(
            value_eq(&got, &$want).unwrap_or(false),
            "{} => {}, want {}",
            $src,
            j::show::show(&$i, &got).unwrap(),
            j::show::show(&$i, &$want).unwrap()
        );
    }};
}

fn ints(xs: &[i64]) -> Value {
    Value::list(xs.iter().map(|n| Value::int(*n)).collect())
}
fn texts(xs: &[&str]) -> Value {
    Value::list(xs.iter().map(|s| Value::text(*s)).collect())
}

#[test]
fn arithmetic() {
    let (mut i, cfg) = make_interp();
    check!(i, cfg, "2 + 3", Value::int(5));
    check!(i, cfg, "2 - 3", Value::int(-1));
    check!(i, cfg, "0 - 5", Value::int(-5));
    check!(i, cfg, "6 * 7", Value::int(42));
    // arbitrary precision
    check!(
        i,
        cfg,
        "100000000000000000000 * 100000000000000000000",
        Value::Int("10000000000000000000000000000000000000000".parse().unwrap())
    );
    check!(i, cfg, "3 < 4", Value::Bool(true));
    check!(i, cfg, "3 <= 3", Value::Bool(true));
    check!(i, cfg, "4 > 3", Value::Bool(true));
    check!(i, cfg, "3 >= 4", Value::Bool(false));
}

#[test]
fn identity_and_const() {
    let (mut i, cfg) = make_interp();
    check!(i, cfg, "id 5", Value::int(5));
    check!(i, cfg, "id \"x\"", Value::text("x"));
    check!(i, cfg, "const 1 2", Value::int(1));
    check!(i, cfg, "const \"a\" [1 2]", Value::text("a"));
}

#[test]
fn cons_and_list_ops() {
    let (mut i, cfg) = make_interp();
    check!(i, cfg, "1 :: [2 3]", ints(&[1, 2, 3]));
    check!(i, cfg, "1 :: []", ints(&[1]));
    check!(i, cfg, "head [1 2 3]", Value::int(1));
    check!(i, cfg, "tail [1 2 3]", ints(&[2, 3]));
    check!(i, cfg, "last [1 2 3]", Value::int(3));
    assert!(crash(&mut i, &cfg, "head []").contains("empty"));
    assert!(crash(&mut i, &cfg, "tail []").contains("empty"));
    assert!(crash(&mut i, &cfg, "last []").contains("empty"));
    check!(i, cfg, "length []", Value::int(0));
    check!(i, cfg, "length [1 2 3]", Value::int(3));
    check!(i, cfg, "null []", Value::Bool(true));
    check!(i, cfg, "null [1]", Value::Bool(false));
}

#[test]
fn nth_take_drop() {
    let (mut i, cfg) = make_interp();
    check!(i, cfg, "nth 0 [10 20 30]", Value::int(10));
    check!(i, cfg, "nth 2 [10 20 30]", Value::int(30));
    assert!(crash(&mut i, &cfg, "nth 3 [1 2 3]").contains("range"));
    assert!(crash(&mut i, &cfg, "nth (0 - 1) [1]").contains("negative"));
    // a huge positive index is out of range, not "negative"
    let msg = crash(&mut i, &cfg, "nth 1000000000000000000000 [1 2 3]");
    assert!(msg.contains("range"), "{}", msg);
    assert!(!msg.contains("negative"), "{}", msg);
    // clamp to length
    check!(i, cfg, "take 2 [1 2 3 4]", ints(&[1, 2]));
    check!(i, cfg, "take 99 [1 2]", ints(&[1, 2]));
    check!(i, cfg, "take 0 [1 2]", ints(&[]));
    check!(i, cfg, "drop 2 [1 2 3 4]", ints(&[3, 4]));
    check!(i, cfg, "drop 99 [1 2]", ints(&[]));
    check!(i, cfg, "drop 0 [1 2]", ints(&[1, 2]));
}

#[test]
fn map_filter_foldl() {
    let (mut i, cfg) = make_interp();
    check!(i, cfg, "map not [true false]", Value::list(vec![Value::Bool(false), Value::Bool(true)]));
    check!(i, cfg, "map (\\x -> x ++ \"!\") [\"a\"]", texts(&["a!"]));
    check!(i, cfg, "filter (\\x -> x > 2) [1 2 3 4]", ints(&[3, 4]));
    check!(i, cfg, "filter (const true) []", ints(&[]));
    check!(i, cfg, "foldl (-) 0 [1 2 3]", Value::int(-6));
    check!(i, cfg, "foldl (\\acc x -> x :: acc) [] [1 2 3]", ints(&[3, 2, 1]));
    check!(i, cfg, "foldl (+) 10 []", Value::int(10));
    // filter with non-Bool predicate result crashes
    assert!(crash(&mut i, &cfg, "filter (const 1) [1]").contains("predicate"));
}

#[test]
fn member_and_range() {
    let (mut i, cfg) = make_interp();
    check!(i, cfg, "member 2 [1 2 3]", Value::Bool(true));
    check!(i, cfg, "member 9 [1 2 3]", Value::Bool(false));
    check!(i, cfg, "member \"a\" []", Value::Bool(false));
    check!(i, cfg, "member ({ a = 1 }) [{ a = 1 }]", Value::Bool(true));
    check!(i, cfg, "range 0 5", ints(&[0, 1, 2, 3, 4]));
    check!(i, cfg, "range 3 3", ints(&[]));
    check!(i, cfg, "range 5 1", ints(&[]));
    check!(i, cfg, "range (0 - 2) 2", ints(&[-2, -1, 0, 1]));
}

#[test]
fn concat_and_append_overloads() {
    let (mut i, cfg) = make_interp();
    check!(i, cfg, "[1 2] ++ [3]", ints(&[1, 2, 3]));
    check!(i, cfg, "[] ++ []", ints(&[]));
    check!(i, cfg, "\"ab\" ++ \"cd\"", Value::text("abcd"));
    check!(i, cfg, "\"\" ++ \"x\"", Value::text("x"));
    check!(i, cfg, "concat [[1] [2 3] []]", ints(&[1, 2, 3]));
    check!(i, cfg, "concat [\"a\" \"b\" \"c\"]", Value::text("abc"));
    assert!(crash(&mut i, &cfg, "concat []").contains("empty"));
    assert!(crash(&mut i, &cfg, "[1] ++ \"x\"").contains("++"));
    assert!(crash(&mut i, &cfg, "\"x\" ++ [1]").contains("++"));
    assert!(crash(&mut i, &cfg, "concat [1 2]").contains("concat"));
    // functions crash
    assert!(crash(&mut i, &cfg, "id ++ id").contains("++"));
}

#[test]
fn text_predicates_and_split() {
    let (mut i, cfg) = make_interp();
    check!(i, cfg, "startsWith \"ab\" \"abc\"", Value::Bool(true));
    check!(i, cfg, "startsWith \"b\" \"abc\"", Value::Bool(false));
    check!(i, cfg, "startsWith \"\" \"abc\"", Value::Bool(true));
    check!(i, cfg, "endsWith \"bc\" \"abc\"", Value::Bool(true));
    check!(i, cfg, "endsWith \"a\" \"abc\"", Value::Bool(false));
    check!(i, cfg, "splitOn \"/\" \"a/b/c\"", texts(&["a", "b", "c"]));
    check!(i, cfg, "splitOn \"/\" \"a\"", texts(&["a"]));
    check!(i, cfg, "splitOn \"/\" \"\"", texts(&[""]));
    check!(i, cfg, "splitOn \"--\" \"a--b--\"", texts(&["a", "b", ""]));
    assert!(crash(&mut i, &cfg, "splitOn \"\" \"abc\"").contains("separator"));
}

#[test]
fn blob_and_text() {
    let (mut i, cfg) = make_interp();
    check!(i, cfg, "text (blob \"hello\")", Value::text("hello"));
    check!(i, cfg, "unresolved (blob \"x\")", Value::Bool(false));
    assert!(crash(&mut i, &cfg, "blob 1").contains("Text"));
    assert!(crash(&mut i, &cfg, "text 1").contains("Blob"));
    assert!(crash(&mut i, &cfg, "unresolved \"x\"").contains("Blob"));
}

#[test]
fn show_renders_literals() {
    let (mut i, cfg) = make_interp();
    check!(i, cfg, "show 42", Value::text("42"));
    check!(i, cfg, "show \"a\\nb\"", Value::text("\"a\\nb\""));
    check!(i, cfg, "show [1 \"x\" true]", Value::text("[1 \"x\" true]"));
    check!(i, cfg, "show ({ b = 2, a = 1 })", Value::text("{ a = 1, b = 2 }"));
    check!(i, cfg, "show (blob \"x\")", Value::text("blob \"x\""));
    check!(i, cfg, "show map", Value::text("map"));
    check!(i, cfg, "show (++)", Value::text("(++)"));
    check!(i, cfg, "show Commit", Value::text("Commit"));
    check!(i, cfg, "show id", Value::text("id"));
    // lambda shows its source, exactly as written (§5.2)
    check!(i, cfg, "show (\\x -> x + 1)", Value::text("\\x -> x + 1"));
}

#[test]
fn show_renders_selectors() {
    // a selector is the SELECTOR atom `.name` (§3.4), not the accessor
    // applied to the field name it carries internally (§5.2)
    let (mut i, cfg) = make_interp();
    check!(i, cfg, "show (.id)", Value::text(".id"));
    check!(i, cfg, "show [(.id)]", Value::text("[(.id)]"));
    check!(i, cfg, "show (map (.id))", Value::text("map (.id)"));
    // and the rendering evaluates back to the same selector
    let shown = ok(&mut i, &cfg, "show (map (.id))");
    let src = format!("({}) [{{ id = 5 }}]", shown.as_text().unwrap());
    check!(i, cfg, &src, ints(&[5]));
}

#[test]
fn extract_everywhere() {
    let (mut i, cfg) = make_interp();
    // ints anywhere
    check!(i, cfg, "extract Int ({ a = [1 2], b = { c = 3 } })", ints(&[1, 2, 3]));
    check!(i, cfg, "extract Int 5", ints(&[5]));
    check!(i, cfg, "extract Text [\"a\" [\"b\"]]", texts(&["a", "b"]));
    check!(i, cfg, "extract Text 5", texts(&[]));
    // functions and blobs are not entered
    check!(i, cfg, "extract Int (\\x -> x)", ints(&[]));
    // matching record also searched inside
    check!(i, cfg, "length (extract Push [{ id = @, name = \"a\" }])", Value::int(1));
    // ascending field-name order
    check!(i, cfg, "extract Int ({ z = 1, a = 2 })", ints(&[2, 1]));
}

#[test]
fn builtin_kind_errors() {
    let (mut i, cfg) = make_interp();
    assert!(crash(&mut i, &cfg, "length 5").contains("list"));
    assert!(crash(&mut i, &cfg, "map id 5").contains("list"));
    assert!(crash(&mut i, &cfg, "nth 0 5").contains("list"));
    assert!(crash(&mut i, &cfg, "not 1").contains("Bool"));
}

#[test]
fn composition_builtin() {
    let (mut i, cfg) = make_interp();
    check!(i, cfg, "(length . tail) [1 2 3]", Value::int(2));
    check!(i, cfg, "(not . null) []", Value::Bool(false));
    check!(i, cfg, "(head . tail) [1 2 3]", Value::int(2));
    check!(i, cfg, "(show . length) [1 2 3]", Value::text("3"));
}

#[test]
fn declared_but_undeclared_builtins() {
    // a config that does not declare `map` leaves it unbound
    let src = r#"
user = { name = "A B", email = "a@b" }
Path = [Text]
Entry = { path : Path, content : Blob }
Commit = { files : [Entry], message : Text, labels : [Text], id : Id }
Subtree = { root : Commit, children : [Subtree] }
Frame = { parent : Commit, left : [Subtree], right : [Subtree] }
Repo = { root : Commit, children : [Subtree], context : [Frame] }
Edit = Repo -> Repo
Revset = Repo -> [Id]
id : a -> a
immutable = \_ -> []
tree = \_ -> ""
labelled = \_ _ -> []
"#;
    let cfg = config::load_config(src).expect("config");
    let mut interp = Interp::new(
        Rc::new(MemBackend::new()),
        cfg.shapes.clone(),
        Env::empty(),
    );
    config::eval_config(&mut interp, &cfg).expect("eval");
    let outer = Rc::new(cfg.global_names.clone());
    let e = parse_expr("map id [1]", outer).unwrap();
    let env = interp.global_env();
    let r = interp.eval(&Rc::new(e), &env);
    match r {
        Ok(_) => panic!("map should be unbound"),
        Err(c) => assert!(c.msg.contains("unbound"), "{}", c.msg),
    }
}

#[test]
fn boolean_operators_as_values() {
    // `&&` and `||` short-circuit as infix operators, which the evaluator
    // handles directly. Used as values — a section, or passed to foldl/map —
    // both arguments are already evaluated, and the builtin must still
    // combine them rather than return the second one.
    let (mut i, cfg) = make_interp();
    check!(i, cfg, "foldl (&&) true [true true]", Value::Bool(true));
    check!(i, cfg, "foldl (&&) true [true false]", Value::Bool(false));
    check!(i, cfg, "foldl (&&) true [false true]", Value::Bool(false));
    check!(i, cfg, "foldl (||) false [false false]", Value::Bool(false));
    check!(i, cfg, "foldl (||) false [false true]", Value::Bool(true));
    check!(i, cfg, "foldl (||) false [true false]", Value::Bool(true));
    // partially applied sections
    check!(i, cfg, "map ((&&) false) [true true]", Value::list(vec![Value::Bool(false); 2]));
    check!(i, cfg, "map ((||) true) [false false]", Value::list(vec![Value::Bool(true); 2]));
    // infix still short-circuits: the rhs is never evaluated
    check!(i, cfg, "false && (crash \"boom\")", Value::Bool(false));
    check!(i, cfg, "true || (crash \"boom\")", Value::Bool(true));
}

#[test]
fn tail_and_drop_share_storage_without_changing_semantics() {
    // `tail`/`drop` now return a view over the same elements instead of
    // copying them (a copying `tail` made every zipper walk quadratic, since
    // `up` is `tail repo.context`). The results must still be ordinary lists.
    let (mut i, cfg) = make_interp();
    check!(i, cfg, "tail [1 2 3]", ints(&[2, 3]));
    check!(i, cfg, "tail (tail [1 2 3])", ints(&[3]));
    check!(i, cfg, "tail (tail (tail [1 2 3]))", ints(&[]));
    check!(i, cfg, "drop 2 [1 2 3 4]", ints(&[3, 4]));
    check!(i, cfg, "drop 0 [1 2]", ints(&[1, 2]));
    check!(i, cfg, "drop 99 [1 2]", ints(&[]));
    check!(i, cfg, "drop 1 (drop 1 [1 2 3 4])", ints(&[3, 4]));
    // a view is equal to the same list built directly
    check!(i, cfg, "tail [1 2 3] == [2 3]", Value::Bool(true));
    check!(i, cfg, "drop 1 (tail [1 2 3 4]) == [3 4]", Value::Bool(true));
    // and behaves like one everywhere else
    check!(i, cfg, "length (tail [1 2 3])", Value::int(2));
    check!(i, cfg, "head (tail [1 2 3])", Value::int(2));
    check!(i, cfg, "last (tail [1 2 3])", Value::int(3));
    check!(i, cfg, "nth 0 (tail [1 2 3])", Value::int(2));
    check!(i, cfg, "tail [1 2 3] ++ [9]", ints(&[2, 3, 9]));
    check!(i, cfg, "0 :: tail [1 2 3]", ints(&[0, 2, 3]));
    check!(i, cfg, "take 1 (tail [1 2 3])", ints(&[2]));
    check!(i, cfg, "map (\\x -> x * 2) (tail [1 2 3])", ints(&[4, 6]));
    check!(i, cfg, "member 2 (tail [1 2 3])", Value::Bool(true));
    check!(i, cfg, "member 1 (tail [1 2 3])", Value::Bool(false));
    check!(i, cfg, "concat [(tail [1 2]) (tail [3 4])]", ints(&[2, 4]));
    check!(i, cfg, "null (tail [1])", Value::Bool(true));
    // the original is untouched by taking a tail of it
    check!(i, cfg, "let xs = [1 2 3] in tail xs ++ xs", ints(&[2, 3, 1, 2, 3]));
    // show round-trips a view
    check!(i, cfg, "show (tail [1 2 3])", Value::text("[2 3]"));
    assert!(crash(&mut i, &cfg, "tail []").contains("empty"));
}
