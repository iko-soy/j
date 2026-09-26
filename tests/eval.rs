//! Evaluator semantics tests (§4).

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

fn ev(interp: &mut Interp, cfg: &config::Config, src: &str) -> Result<Value, String> {
    let outer = Rc::new(cfg.global_names.clone());
    let e = parse_expr(src, outer).map_err(|p| format!("parse: {}", p.msg))?;
    let e = config::resolve_ids(&e, interp).map_err(|_| "id resolution".to_string())?;
    let env = interp.global_env();
    interp
        .eval(&Rc::new(e), &env)
        .map_err(|c| format!("crash: {}", c.msg))
}

fn ok(interp: &mut Interp, cfg: &config::Config, src: &str) -> Value {
    ev(interp, cfg, src).unwrap_or_else(|e| panic!("{} => {}", src, e))
}

fn crash(interp: &mut Interp, cfg: &config::Config, src: &str) -> String {
    match ev(interp, cfg, src) {
        Ok(v) => panic!("{} unexpectedly succeeded: {}", src, j::show::show(interp, &v)),
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
            j::show::show(&$i, &got),
            j::show::show(&$i, &$want)
        );
    }};
}

#[test]
fn or_non_function_lhs_skips_rhs() {
    let (mut i, cfg) = make_interp();
    // rhs would crash; not evaluated
    check!(i, cfg, "1 or head []", Value::int(1));
    check!(i, cfg, "\"x\" or crash \"no\"", Value::text("x"));
    // lhs crashes: rhs returned
    check!(i, cfg, "head [] or 9", Value::int(9));
}

#[test]
fn or_function_lifting() {
    let (mut i, cfg) = make_interp();
    // (f or g) x = f x or g x
    check!(
        i,
        cfg,
        "(head or (\\_ -> 5)) []",
        Value::int(5)
    );
    check!(
        i,
        cfg,
        "(head or (\\_ -> 5)) [7]",
        Value::int(7)
    );
    // same rule applies again to the result
    check!(
        i,
        cfg,
        "((\\x -> head x or 0) or (\\_ -> 9)) []",
        Value::int(0)
    );
    // ... including lifting again when `f x` is a function, so the fallback
    // still guards the later arguments of a curried lhs
    check!(
        i,
        cfg,
        "((\\a b -> head b) or (\\a b -> 5)) 1 []",
        Value::int(5)
    );
    check!(
        i,
        cfg,
        "((\\a b c -> head c) or (\\a b c -> 7)) 1 2 []",
        Value::int(7)
    );
    check!(
        i,
        cfg,
        "((\\a b -> a) or (\\a b -> 5)) 1 []",
        Value::int(1)
    );
    // `f x` a function and `g x` not: `f x` alone, no longer guarded
    let m = crash(&mut i, &cfg, "((\\a b -> head b) or (\\a -> 5)) 1 []");
    assert!(m.contains("head: empty list"), "{}", m);
    // `g x` is evaluated unguarded, as the rhs of a written `or` is
    let m = crash(&mut i, &cfg, "((\\a b -> b) or (\\a -> crash \"rhs\")) 1 2");
    assert!(m.contains("rhs"), "{}", m);
    // lhs function, rhs not a function: lhs returned
    let v = ok(&mut i, &cfg, "head or 3");
    assert!(matches!(v, Value::Fun(_)), "expected a function");
}

#[test]
fn short_circuit_booleans() {
    let (mut i, cfg) = make_interp();
    check!(i, cfg, "false && crash \"x\"", Value::Bool(false));
    check!(i, cfg, "true || crash \"x\"", Value::Bool(true));
    check!(i, cfg, "true && true", Value::Bool(true));
    check!(i, cfg, "false || false", Value::Bool(false));
    assert!(crash(&mut i, &cfg, "1 && true").contains("Bool"));
    // rhs of || is not evaluated when lhs is true; non-Bool rhs crashes when reached
    assert!(crash(&mut i, &cfg, "false || 1").contains("Bool"));
}

#[test]
fn if_requires_bool() {
    let (mut i, cfg) = make_interp();
    check!(i, cfg, "if 1 == 1 then \"a\" else \"b\"", Value::text("a"));
    let m = crash(&mut i, &cfg, "if [1] then 1 else 2");
    assert!(m.contains("Bool"), "{}", m);
}

#[test]
fn let_is_recursive() {
    let (mut i, cfg) = make_interp();
    // mutual recursion within one block
    check!(
        i,
        cfg,
        "let even = \\n -> if n == 0 then true else odd (n - 1); odd = \\n -> if n == 0 then false else even (n - 1) in even 10",
        Value::Bool(true)
    );
    // later binding sees earlier
    check!(i, cfg, "let x = 1; y = x + 1 in y", Value::int(2));
}

#[test]
fn let_bindings_evaluate_in_dependency_order() {
    let (mut i, cfg) = make_interp();
    // §4.1: a binding is evaluated after every binding it refers to outside
    // a lambda, whatever order the block lists them in
    check!(i, cfg, "let y = x + 1; x = 1 in y", Value::int(2));
    check!(i, cfg, "let a = b; b = 5 in a", Value::int(5));
    check!(i, cfg, "let a = b + 1; b = c + 1; c = 10 in a", Value::int(12));
    check!(i, cfg, "let z = f 2; f = \\x -> x * 3 in z", Value::int(6));
    check!(i, cfg, "let\n  total = base * 2\n  base = 21\nin total", Value::int(42));
    check!(i, cfg, "(\\n -> let y = x + n; x = 1 in y) 5", Value::int(6));
    // source order made `b` unbound here, a crash `or` caught: 0, not 1
    check!(i, cfg, "let a = b or 0; b = 1 in a", Value::int(1));
    // a function applied in a binding finds the bindings its body refers to,
    // as one applied at load does (§4.1)
    check!(i, cfg, "let f = \\x -> x * k; z = f 2; k = 3 in z", Value::int(6));
    check!(
        i,
        cfg,
        "let ev = \\n -> if n == 0 then k else od (n - 1); r = ev 4; od = \\n -> if n == 0 then 0 else ev (n - 1); k = 1 in r",
        Value::int(1)
    );
    // mutually referring bindings keep the order that references outside
    // lambdas give them: `b` applies `a`, whose body needs `y`, and `y`
    // refers back to `b`; `b` waits for `z`, so `y` is bound by then
    check!(i, cfg, "let a = \\_ -> y; b = a z; y = \\_ -> b; z = 1 in z", Value::int(1));
}

#[test]
fn let_cycle_is_a_crash() {
    let (mut i, cfg) = make_interp();
    for src in ["let a = b; b = a in a", "let a = a in a", "let a = [b]; b = { x = a } in 1"] {
        let m = crash(&mut i, &cfg, src);
        assert!(m.contains("cycle"), "{} => {}", src, m);
    }
    // an ordinary crash, which `or` catches, and only when the block is
    // evaluated
    check!(i, cfg, "(let a = b + 1; b = a in a) or 7", Value::int(7));
    check!(i, cfg, "if false then (let a = a in a) else 1", Value::int(1));
    // references inside lambdas never form a cycle
    check!(i, cfg, "let f = \\n -> if n == 0 then 0 else f (n - 1) in f 3", Value::int(0));
}

#[test]
fn lexical_scoping() {
    let (mut i, cfg) = make_interp();
    // closures capture their environment: `f` sees the `x` it was defined
    // under, not the `x` bound where it is called (§4.2 forbids shadowing, so
    // the two `x`s are in disjoint scopes)
    check!(
        i,
        cfg,
        "let f = (let x = 1 in \\y -> x + y); g = \\x -> f x in g 100",
        Value::int(101)
    );
    // and keep it after the scope that bound it has returned
    check!(
        i,
        cfg,
        "let add = \\x -> \\y -> x + y in let inc = add 1 in inc 41",
        Value::int(42)
    );
}

#[test]
fn unbound_names_crash() {
    let (mut i, cfg) = make_interp();
    let m = crash(&mut i, &cfg, "nosuchname");
    assert!(m.contains("unbound"), "{}", m);
}

#[test]
fn applying_non_function_crashes() {
    let (mut i, cfg) = make_interp();
    assert!(crash(&mut i, &cfg, "1 2").contains("apply"));
    assert!(crash(&mut i, &cfg, "\"x\" 1").contains("apply"));
    assert!(crash(&mut i, &cfg, "[1] 2").contains("apply"));
}

#[test]
fn record_selection_and_update() {
    let (mut i, cfg) = make_interp();
    check!(i, cfg, "{ a = 1 }.a", Value::int(1));
    let m = crash(&mut i, &cfg, "{ a = 1 }.b");
    assert!(m.contains("no field"), "{}", m);
    let m = crash(&mut i, &cfg, "1 .a");
    assert!(m.contains("select"), "{}", m);
    // update never adds fields
    let m = crash(&mut i, &cfg, "({ a = 1 }) { b = 2 }");
    assert!(m.contains("no field"), "{}", m);
    // selector function (record argument parenthesised)
    check!(i, cfg, "(.a) ({ a = 42 })", Value::int(42));
}

#[test]
fn equality_rules() {
    let (mut i, cfg) = make_interp();
    check!(i, cfg, "[1 [2]] == [1 [2]]", Value::Bool(true));
    check!(i, cfg, "{ a = 1 } == { a = 1 }", Value::Bool(true));
    check!(i, cfg, "{ a = 1 } == { a = 2 }", Value::Bool(false));
    check!(i, cfg, "{ a = 1 } == { b = 1 }", Value::Bool(false));
    check!(i, cfg, "1 /= 2", Value::Bool(true));
    check!(i, cfg, "blob \"x\" == blob \"x\"", Value::Bool(true));
    check!(i, cfg, "blob \"x\" == blob \"y\"", Value::Bool(false));
    // comparing functions crashes
    let m = crash(&mut i, &cfg, "map == map");
    assert!(m.contains("function"), "{}", m);
    // cross-kind is just false
    check!(i, cfg, "1 == \"1\"", Value::Bool(false));
    // ordering on non-Ints crashes
    assert!(crash(&mut i, &cfg, "\"a\" < \"b\"").contains("Int"));
}

#[test]
fn shapes_as_values() {
    let (mut i, cfg) = make_interp();
    let v = ok(&mut i, &cfg, "Commit");
    assert!(matches!(v, Value::Shape(_)));
    // alias of a record shape works
    let v = ok(&mut i, &cfg, "Repo");
    assert!(matches!(v, Value::Shape(_)));
    // function-shaped typedecls crash when used
    let m = crash(&mut i, &cfg, "Edit");
    assert!(m.contains("shape"), "{}", m);
    let m = crash(&mut i, &cfg, "Path");
    assert!(m.contains("shape"), "{}", m);
    let m = crash(&mut i, &cfg, "Undeclared");
    assert!(m.contains("shape"), "{}", m);
}

/// The reference config with `extra` appended, loaded and evaluated.
fn make_interp_with(extra: &str) -> (Interp, config::Config) {
    let src = format!("{}\n{}", CONFIG, extra);
    let cfg = config::load_config(&src).expect("config must load");
    let mut interp = Interp::new(Rc::new(MemBackend::new()), cfg.shapes.clone(), Env::empty());
    config::eval_config(&mut interp, &cfg).expect("config must evaluate");
    (interp, cfg)
}

#[test]
fn cyclic_alias_is_not_a_usable_shape() {
    // §4.12, §4.7: an alias that never reaches a type is a catchable crash
    // when used as a shape. Resolving it recursed until the native stack
    // overflowed (an abort `or` cannot catch), and so did displaying any
    // record, since display asks every typedecl for its shape.
    let (mut i, cfg) = make_interp_with("Self = Self\nA = B\nB = A\n");
    for name in ["Self", "A", "B"] {
        let m = crash(&mut i, &cfg, name);
        assert!(m.contains(&format!("`{}` does not name a usable shape", name)), "{}", m);
    }
    check!(i, cfg, "Self or 1", Value::int(1));
    check!(i, cfg, "A or 7", Value::int(7));
    let m = crash(&mut i, &cfg, "extract Self [1]");
    assert!(m.contains("usable shape"), "{}", m);
    let rec = ok(&mut i, &cfg, "{ a = 1 }");
    assert_eq!(j::render::display(&mut i, &rec, false).unwrap(), "a  1\n");
}

#[test]
fn cyclic_alias_in_a_signature_loads() {
    // §4.13: an alias of a function type is unfolded into the contract.
    // Unfolding a cyclic alias never stopped, so every run hung at load; a
    // self-referring function alias is unfolded once, and its result is
    // checked as the alias itself.
    let (mut i, cfg) = make_interp_with(
        "A = B\nB = A\n\nf : A\nf = 1\n\nStream = Int -> Stream\n\ng : Stream\ng = \\x -> g\n",
    );
    check!(i, cfg, "f", Value::int(1));
    let v = ok(&mut i, &cfg, "g 1 2 3");
    assert!(matches!(v, Value::Fun(_)));
    let m = crash(&mut i, &cfg, "g 1 \"x\"");
    assert!(m.contains("g expected Int as argument 1, got Text"), "{}", m);
}

#[test]
fn alias_of_an_undeclared_type_is_not_a_usable_shape() {
    // §4.12: an alias takes its type's shape, and an undeclared type's shape
    // is a crash when used. It used to be a shape nothing matched, so
    // `extract Foo xs` quietly returned [].
    let (mut i, cfg) = make_interp_with("Foo = Bar\n");
    let m = crash(&mut i, &cfg, "Foo");
    assert!(m.contains("`Foo` does not name a usable shape"), "{}", m);
    let m = crash(&mut i, &cfg, "extract Foo [1 \"a\"]");
    assert!(m.contains("usable shape"), "{}", m);
}

#[test]
fn new_id_mints_distinct_ids() {
    let (mut i, cfg) = make_interp();
    check!(i, cfg, "@ /= @", Value::Bool(true));
    // in a lambda body, each application mints
    check!(
        i,
        cfg,
        "let f = \\_ -> @ in f 1 /= f 1",
        Value::Bool(true)
    );
    // two mints in one expression are distinct
    check!(i, cfg, "@ == @", Value::Bool(false));
}

#[test]
fn label_literals_are_revsets() {
    let (mut i, cfg) = make_interp();
    // %name is a function
    let v = ok(&mut i, &cfg, "%main");
    assert!(matches!(v, Value::Fun(_)));
    // show renders it back
    check!(i, cfg, "show %main", Value::text("%main"));
}

#[test]
fn single_quoted_text() {
    let (mut i, cfg) = make_interp();
    check!(i, cfg, "'hello'", Value::text("hello"));
    check!(i, cfg, "'say \"hi\"'", Value::text("say \"hi\""));
    check!(i, cfg, "\"don't\"", Value::text("don't"));
    check!(i, cfg, "'a' ++ 'b'", Value::text("ab"));
    check!(i, cfg, "'x' == \"x\"", Value::Bool(true));
}

#[test]
fn path_literals_are_text_lists() {    let (mut i, cfg) = make_interp();
    check!(
        i,
        cfg,
        "./src/lexer.rs",
        Value::list(vec![Value::text("src"), Value::text("lexer.rs")])
    );
    check!(i, cfg, "./", Value::list(vec![]));
    check!(i, cfg, "./a/b/ == ./a/b", Value::Bool(true));
}

#[test]
fn crash_messages_propagate() {
    let (mut i, cfg) = make_interp();
    let m = crash(&mut i, &cfg, "crash \"custom message\"");
    assert!(m.contains("custom message"), "{}", m);
}

#[test]
fn contract_result_checked() {
    let (mut i, cfg) = make_interp();
    // head : [a] -> a — no constraint, fine
    check!(i, cfg, "head [1 2]", Value::int(1));
    // length's result is Int
    check!(i, cfg, "length \"x\" or 0", Value::int(0)); // length on text crashes (list only) -> or
}

#[test]
fn goto_error_message() {
    let (mut i, cfg) = make_interp();
    // goto on an empty revset against a trivial repo
    let m = ev(
        &mut i,
        &cfg,
        "goto (labelled \"nope\")",
    );
    // the function is returned; applying it to a repo crashes — test via at
    assert!(m.is_ok(), "goto itself is a value");
}

#[test]
fn wildcard_patterns() {
    let (mut i, cfg) = make_interp();
    check!(i, cfg, "(\\_ -> 42) \"anything\"", Value::int(42));
    check!(i, cfg, "(\\_ _ -> 1) 2 3", Value::int(1));
}

#[test]
fn deeply_nested_or() {
    let (mut i, cfg) = make_interp();
    check!(
        i,
        cfg,
        "head [] or head [] or head [] or 4",
        Value::int(4)
    );
}

#[test]
fn very_deep_recursion() {
    let (mut i, cfg) = make_interp();
    let v = ev(
        &mut i,
        &cfg,
        "let go = \\n -> if n == 0 then 0 else 1 + go (n - 1) in go 100000",
    );
    assert!(value_eq(&v.unwrap(), &Value::int(100000)).unwrap());
}

#[test]
fn very_deep_or_recursion() {
    // `or`-recursive walks (the shape of `top`/`tip` over history) must not
    // consume native stack: crash-catching is part of the heap machine
    let (mut i, cfg) = make_interp();
    let v = ev(
        &mut i,
        &cfg,
        "let go = \\n -> (if n <= 0 then crash \"bottom\" else go (n - 1)) or n in go 50000",
    );
    assert!(value_eq(&v.unwrap(), &Value::int(0)).unwrap());
}

#[test]
fn or_caught_crash_yields_rhs_id() {
    // a crash on the lhs, after minting an id, is caught and the rhs value is
    // used (the mint on the failing branch is rewound)
    let (mut i, cfg) = make_interp();
    let v = ok(
        &mut i,
        &cfg,
        "((\\_ -> let discard = @ in crash \"boom\") 0) or @",
    );
    assert!(matches!(v, Value::Id(_)), "got {}", j::show::show(&i, &v));
}

#[test]
fn partial_application_values() {
    let (mut i, cfg) = make_interp();
    check!(i, cfg, "(take 2) [1 2 3 4]", Value::list(vec![Value::int(1), Value::int(2)]));
    check!(i, cfg, "map ((+) 1) [1 2]", Value::list(vec![Value::int(2), Value::int(3)]));
    check!(i, cfg, "foldl (.) id [not not] true", Value::Bool(true));
}

#[test]
fn operator_as_function() {
    let (mut i, cfg) = make_interp();
    check!(i, cfg, "(+) 1 2", Value::int(3));
    check!(i, cfg, "foldl (++) \"\" [\"a\" \"b\"]", Value::text("ab"));
    check!(i, cfg, "(.) not not true", Value::Bool(true));
    check!(i, cfg, "(::) 1 [2]", Value::list(vec![Value::int(1), Value::int(2)]));
}
