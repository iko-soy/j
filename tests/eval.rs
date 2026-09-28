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
        Ok(v) => panic!("{} unexpectedly succeeded: {}", src, j::show::show(interp, &v).unwrap()),
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
    // among them, one that refers to none of the others outside a lambda and
    // only stores its lambdas that do goes first: `env` is bound before
    // `out` applies `fmt`, whose body needs it, wherever the block lists it
    check!(
        i,
        cfg,
        "let fmt = \\x -> env.lanes; out = fmt 1; env = { lanes = 3, again = \\_ -> out } in out",
        Value::int(3)
    );
    check!(
        i,
        cfg,
        "let fmt = \\x -> env.lanes; out = fmt 1; env = let n = 3 in { lanes = n, again = [(\\_ -> out)] } in out",
        Value::int(3)
    );
    // a binding that applies such a lambda keeps its place in the first
    // order: `bm` must still follow `ax`
    check!(
        i,
        cfg,
        "let aa = \\_ -> bm; ax = { f = aa, n = 1 }; bm = (\\f -> f 0) (\\_ -> ax) in bm.n",
        Value::int(1)
    );
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

/// Signed definitions whose lambda takes fewer parameters than the signature,
/// so the body's value is a function the rest of the signature checks (§4.13).
const DEFERRED: &str = r#"
failIf : Int -> Edit
failIf = \n -> if n > 0 then id else crash "negative"

choose : Int -> Int -> Int
choose = \n -> if n > 0 then (\x -> x) else crash "negative"

bad : Int -> Int -> Int
bad = \a -> \b -> "oops"

bad3 : Int -> Int -> Int -> Int
bad3 = \a -> \b c -> "oops"

add3 : Int -> Int -> Int -> Int
add3 = \a -> \b c -> a + b + c
"#;

const REPO: &str = r#"{ root = { files = [], message = "kept", labels = [], id = @ }, children = [], context = [] }"#;

#[test]
fn deferred_body_crash_is_catchable_and_keeps_its_message() {
    // §4.6, §4.7: a crash in a deferred body is a crash like any other. It
    // was replaced by "<name>: body crashed when applied" and raised past
    // every enclosing `or`, so `failIf 0 or id` aborted the whole command.
    let (mut i, cfg) = make_interp_with(DEFERRED);
    check!(i, cfg, "choose 0 5 or 7", Value::int(7));
    check!(i, cfg, "(choose 0 or (\\x -> x + 1)) 5", Value::int(6));
    check!(i, cfg, "choose 1 5", Value::int(5));
    let m = crash(&mut i, &cfg, "choose 0 5");
    assert_eq!(m, "crash: negative");
    let src = format!("((failIf 0 or id) ({})).root.message", REPO);
    check!(i, cfg, &src, Value::text("kept"));
    let src = format!("(\\r -> failIf 0 r or r) ({})", REPO);
    let v = ok(&mut i, &cfg, &src);
    assert!(matches!(v, Value::Record(_)));
    let m = crash(&mut i, &cfg, &format!("failIf 0 ({})", REPO));
    assert_eq!(m, "crash: negative");
}

#[test]
fn deferred_body_is_contract_checked() {
    // §4.13: every argument of a signed definition is checked as it is
    // supplied, and the result once the application is complete. Those that
    // arrived after the body was deferred went unchecked, and so did the
    // result: `bad 1 "x"` was "oops".
    let (mut i, cfg) = make_interp_with(DEFERRED);
    let m = crash(&mut i, &cfg, "bad 1 \"x\"");
    assert!(m.contains("contract: bad expected Int as argument 2, got Text"), "{}", m);
    let m = crash(&mut i, &cfg, "bad 1 2");
    assert!(m.contains("contract: bad: result expected Int, got Text"), "{}", m);
    // and a violation is catchable like any other crash
    check!(i, cfg, "bad 1 2 or 9", Value::int(9));
    check!(i, cfg, "(bad 1 or (\\_ -> 9)) 2", Value::int(9));
    let m = crash(&mut i, &cfg, "failIf 1 3");
    assert!(m.contains("contract: failIf expected Repo"), "{}", m);
    // with two parameters left once the body is deferred, each of them and
    // the result are checked too
    let m = crash(&mut i, &cfg, "bad3 1 \"x\" 3");
    assert!(m.contains("contract: bad3 expected Int as argument 2, got Text"), "{}", m);
    let m = crash(&mut i, &cfg, "bad3 1 2 \"y\"");
    assert!(m.contains("contract: bad3 expected Int as argument 3, got Text"), "{}", m);
    let m = crash(&mut i, &cfg, "bad3 1 2 3");
    assert!(m.contains("contract: bad3: result expected Int, got Text"), "{}", m);
    check!(i, cfg, "add3 1 2 3", Value::int(6));
    check!(i, cfg, "let f = add3 1 2 in f 3 + f 4", Value::int(13));
    // the partial application stays a named value (§5.2)
    check!(i, cfg, "show (add3 1 2)", Value::text("add3 1 2"));
    // the reference config's own deferred definitions blame themselves
    let m = crash(&mut i, &cfg, "describe \"x\" 3");
    assert!(m.contains("contract: describe expected Repo"), "{}", m);
    assert!(m.contains("argument 2"), "{}", m);
}

#[test]
fn signature_does_not_change_when_a_body_runs() {
    // §4.1: evaluation is strict and a lambda body is evaluated on each
    // application; a signature only adds checks (§4.13). A signed definition
    // whose lambda took fewer parameters than its signature deferred its
    // body until the signature was used up, so `failIf 0` did not crash,
    // `failIf 0 or 7` crashed past the `or` (§4.6), and the body ran again
    // at every later application.
    let (mut i, cfg) = make_interp_with(&format!(
        "{}{}",
        DEFERRED,
        r#"
failIfU = \n -> if n > 0 then id else crash "negative"

pick2 : Int -> Int -> Int -> Int
pick2 = \n -> if n > 0 then (\a b -> a + b) else crash "negative"

notFn : Int -> Edit
notFn = \n -> n

once : Int -> Int -> Id
once = \n -> let i = @ in \m -> i

onceU = \n -> let i = @ in \m -> i
"#
    ));
    // the body crashes at the application that binds the lambda's last
    // parameter, signed or not, and `or` catches it there
    for f in ["failIf", "failIfU"] {
        check!(i, cfg, &format!("{} 0 or 7", f), Value::int(7));
        check!(i, cfg, &format!("show ({} 0 or \"fallback\")", f), Value::text("\"fallback\""));
        let m = crash(&mut i, &cfg, &format!("let e = {} 0 in 5", f));
        assert_eq!(m, "crash: negative");
    }
    check!(i, cfg, "choose 0 or 7", Value::int(7));
    check!(i, cfg, "pick2 0 or 5", Value::int(5));
    check!(i, cfg, "pick2 0 1 or 5", Value::int(5));
    let m = crash(&mut i, &cfg, "let e = pick2 0 1 in 5");
    assert_eq!(m, "crash: negative");
    let (m, def) = crash_in(&mut i, &cfg, "failIf 0");
    assert_eq!((m.as_str(), def.as_deref()), ("negative", Some("failIf")));
    // it runs once, not again at every later argument
    for f in ["once", "onceU"] {
        check!(i, cfg, &format!("let g = {} 1 in g 1 == g 2", f), Value::Bool(true));
    }
    // the body's value must be a function while the signature lists more
    let (m, def) = crash_in(&mut i, &cfg, "notFn 1");
    assert_eq!(m, "contract: notFn expected a function, got Int");
    assert_eq!(def.as_deref(), Some("notFn"));
    check!(i, cfg, "notFn 1 or 7", Value::int(7));
    // the application still renders by name (§5.2), and the arguments the
    // signature lists past the lambda's, and the result, are still checked
    check!(i, cfg, "show (pick2 1 2)", Value::text("pick2 1 2"));
    check!(i, cfg, "show (failIf 1)", Value::text("failIf 1"));
    check!(i, cfg, "show (describe \"wip\")", Value::text("describe \"wip\""));
    check!(
        i,
        cfg,
        "show (at (parents) (describe \"x\"))",
        Value::text("at (parents) (describe \"x\")")
    );
    check!(i, cfg, "pick2 1 2 3", Value::int(5));
    check!(i, cfg, "let f = pick2 1 in f 2 3 + f 4 5", Value::int(14));
    let m = crash(&mut i, &cfg, "pick2 1 \"x\"");
    assert!(m.contains("contract: pick2 expected Int as argument 2, got Text"), "{}", m);
    let m = crash(&mut i, &cfg, "pick2 1 2 \"y\"");
    assert!(m.contains("contract: pick2 expected Int as argument 3, got Text"), "{}", m);
    let m = crash(&mut i, &cfg, "once 1 \"x\"");
    assert!(m.contains("contract: once expected Int as argument 2, got Text"), "{}", m);
}

/// Definitions that crash at different depths, for the crash trace (§1.4).
const NESTED: &str = r#"
inner : Int -> Int
inner = \n -> if n > 0 then n else crash "not positive"

outer : Int -> Int
outer = \n -> inner n + 1

after : Int -> Int
after = \n -> inner n + head []

caught : Int -> Int
caught = \n -> (inner 0 or n) + head []

viaMap : [Int] -> [Int]
viaMap = \xs -> map (\x -> x + head []) xs

mapInner : [Int] -> [Int]
mapInner = \xs -> map inner xs

first : [Int] -> Int
first = head

mapped : [Int] -> [Int]
mapped = map (\x -> x + head [])

counted : [Int] -> Int
counted = map id

plain = \n -> if n > 0 then n else crash "not positive"

plainOuter = \n -> plain n + 1

plainAfter = \n -> plain n + head []

plainTail = \n -> if n > 0 then plainTail (n - 1) else plain n

plainBack = \n -> if n > 0 then plainBack (n - 1) else head []

signedCallsPlain : Int -> Int
signedCallsPlain = \n -> plain n

plainCallsSigned = \n -> inner n

sumPos : Int -> Int -> Int
sumPos = \a b -> if b > 0 then a + b else crash "not positive"

addOne : Int -> Int
addOne = sumPos 1

sameAsInner : Int -> Int
sameAsInner = inner
"#;

/// The message of the crash `src` raises and the definition it is in.
fn crash_in(interp: &mut Interp, cfg: &config::Config, src: &str) -> (String, Option<String>) {
    let outer = Rc::new(cfg.global_names.clone());
    let e = parse_expr(src, outer).unwrap_or_else(|p| panic!("{}: {}", src, p.msg));
    let e = config::resolve_ids(&e, interp).unwrap_or_else(|_| panic!("{}: ids", src));
    let env = interp.global_env();
    match interp.eval(&Rc::new(e), &env) {
        Ok(v) => panic!("{} unexpectedly succeeded: {}", src, j::show::show(interp, &v).unwrap()),
        Err(c) => (c.msg, c.def),
    }
}

#[test]
fn crash_names_the_innermost_definition_executing() {
    // §1.4, §5.1: a crash reports the innermost definition that was
    // executing. Nothing recorded one once loading was over, so every crash
    // at run time reported none and the CLI printed only `from EXPR`.
    let (mut i, cfg) = make_interp_with(&format!("{}{}", DEFERRED, NESTED));
    let goto = format!("goto (\\_ -> []) ({})", REPO);
    let prev = format!("prev ({})", REPO);
    let next = format!("next ({})", REPO);
    for (src, want) in [
        ("inner 0", Some("inner")),
        // §5.1's example, on a repository literal
        (goto.as_str(), Some("goto")),
        // the definition running, not the one that called it
        ("outer 0", Some("inner")),
        // once `inner` has returned it is no longer executing
        ("after 1", Some("after")),
        // a crash `or` caught in `inner` does not name the next one
        ("caught 1", Some("caught")),
        // a lambda a builtin runs is in the definition that made it; a
        // definition a builtin runs is itself the innermost
        ("viaMap [1]", Some("viaMap")),
        ("mapInner [0]", Some("inner")),
        // a body whose lambda is shorter than its signature runs in its
        // definition too (§4.13)
        ("choose 0 5", Some("choose")),
        // a contract violation is in the definition checked (§4.13),
        // whether an argument, one past the lambda's, or the result
        ("inner \"x\"", Some("inner")),
        ("outer \"x\"", Some("outer")),
        ("bad 1 \"x\"", Some("bad")),
        ("bad 1 2", Some("bad")),
        ("describe 3", Some("describe")),
        // a definition whose value is a builtin runs under its own name
        ("first []", Some("first")),
        ("first \"x\"", Some("first")),
        ("squash 3", Some("squash")),
        // and so do the functions it applies, and its result's check
        ("mapped [1]", Some("mapped")),
        ("counted [1]", Some("counted")),
        // a definition without a signature is executing while its body runs,
        // as one with a signature is: nothing named one, so `plain 0` and
        // `plainOuter 0` named none, and `signedCallsPlain 0` named the
        // signed definition that called `plain`
        ("plain 0", Some("plain")),
        ("plainOuter 0", Some("plain")),
        ("plainAfter 1", Some("plainAfter")),
        ("plainTail 3", Some("plain")),
        ("plainBack 3", Some("plainBack")),
        ("signedCallsPlain 0", Some("plain")),
        ("signedCallsPlain \"x\"", Some("signedCallsPlain")),
        ("plainCallsSigned 0", Some("inner")),
        // a definition whose value is another's function runs that one's
        // body, the innermost (`prev = goto parents`), where the outer
        // name was reported; its own signature still checks its arguments
        ("addOne 0", Some("sumPos")),
        ("addOne \"x\"", Some("addOne")),
        ("sameAsInner 0", Some("inner")),
        ("sameAsInner \"x\"", Some("sameAsInner")),
        (prev.as_str(), Some("goto")),
        (next.as_str(), Some("goto")),
        // a builtin is no definition: outside every definition, none
        ("head []", None),
        ("crash \"top\"", None),
        ("(\\x -> x + head []) 1", None),
    ] {
        let (msg, def) = crash_in(&mut i, &cfg, src);
        assert_eq!(def.as_deref(), want, "{} crashed with {}", src, msg);
    }
}

#[test]
fn unsigned_tail_recursion_keeps_its_continuation_constant() {
    // §1.4, §4.1: a definition's body runs under a frame naming it, and a
    // call in tail position of that body replaces its frame. A frame kept
    // per call grew the continuation with every step of an unsigned tail
    // recursion, which only signed definitions' checks did before, and a
    // crash at the end dropped the whole chain at once on the native stack.
    on_stack(4, || {
        let (mut i, cfg) = make_interp_with(NESTED);
        let (msg, def) = crash_in(&mut i, &cfg, "plainTail 50000");
        assert_eq!(def.as_deref(), Some("plain"), "{}", msg);
        let (msg, def) = crash_in(&mut i, &cfg, "plainBack 50000");
        assert_eq!(def.as_deref(), Some("plainBack"), "{}", msg);
    });
}

/// Signed recursions whose lambdas take fewer parameters than their
/// signatures, or as many, for the space they take (§4.1, §4.13).
const SIGNED_TAIL: &str = r#"
shortS : Int -> Int -> Int
shortS = \n -> if n == 0 then (\a -> a) else shortS (n - 1)

countS : Int -> Edit
countS = \n -> if n == 0 then id else countS (n - 1)

endS : Int -> Int
endS = \n -> if n == 0 then crash "end" else endS (n - 1)

endShortS : Int -> Int -> Int
endShortS = \n -> if n == 0 then crash "end" else endShortS (n - 1)

letS : Int -> Int -> Int
letS = \n -> if n == 0 then (\a -> a) else (let r = letS (n - 1) in r)

evenS : Int -> Int -> Int
evenS = \n -> if n == 0 then (\a -> a) else oddS (n - 1)

oddS : Int -> Int -> Int
oddS = \n -> if n == 0 then (\a -> a) else evenS (n - 1)

endEvenS : Int -> Int -> Int
endEvenS = \n -> if n == 0 then crash "end" else endOddS (n - 1)

endOddS : Int -> Int -> Int
endOddS = \n -> if n == 0 then crash "end" else endEvenS (n - 1)
"#;

#[test]
fn signed_tail_recursion_keeps_its_continuation_constant() {
    // §4.1, §4.13: a signature only adds checks, so a signed tail recursion
    // takes the space of its unsigned twin. Every call of `shortS` pushed
    // its own check and naming under the caller's, and the value came back
    // as `\x -> f x` wrapped once per call: holding it took a gigabyte at a
    // million calls, and dropping it recursed once per wrapper on the
    // native stack and aborted the process. A crash at the end of such a
    // chain dropped the continuation at once, the same way. A recursion
    // that is not a tail call, or goes through another signed definition,
    // still returns through every call, but its value drops as any other,
    // and so does its continuation when a crash abandons it.
    on_stack(4, || {
        let (mut i, cfg) = make_interp_with(SIGNED_TAIL);
        let n = 50000;
        check!(i, cfg, &format!("let f = shortS {n} in 1"), Value::int(1));
        check!(i, cfg, &format!("let e = countS {n} in 1"), Value::int(1));
        check!(i, cfg, &format!("show (shortS {n})"), Value::text(format!("shortS {n}")));
        check!(i, cfg, &format!("shortS {n} 7"), Value::int(7));
        for f in ["endS", "endShortS"] {
            let (msg, def) = crash_in(&mut i, &cfg, &format!("{f} {n}"));
            assert_eq!((msg.as_str(), def.as_deref()), ("end", Some(f)));
        }
        for f in ["letS", "evenS"] {
            check!(i, cfg, &format!("let g = {f} {n} in 1"), Value::int(1));
            check!(i, cfg, &format!("{f} {n} 7"), Value::int(7));
        }
        let (msg, def) = crash_in(&mut i, &cfg, &format!("endEvenS {n}"));
        assert_eq!((msg.as_str(), def.as_deref()), ("end", Some("endEvenS")));
        check!(i, cfg, &format!("endEvenS {n} or 5"), Value::int(5));
        // the checks the frames stood for still hold
        let m = crash(&mut i, &cfg, &format!("shortS {n} \"x\""));
        assert!(m.contains("contract: shortS expected Int as argument 2, got Text"), "{}", m);
        let m = crash(&mut i, &cfg, &format!("countS {n} 5"));
        assert!(m.contains("contract: countS expected Repo (record) as argument 2"), "{}", m);
    });
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
fn very_deep_recursion_through_builtins_that_apply_functions() {
    // §4.1: map, filter, foldl and a composition applied the functions they
    // were given in a nested run on the native stack, so a recursion through
    // one of them overflowed the stack and aborted the process at depths
    // that plain recursion (very_deep_recursion) reaches on this thread
    let (mut i, cfg) = make_interp();
    for src in [
        "let f = \\n -> if n == 0 then 0 else head (map f [(n - 1)]) in f 30000",
        "let f = \\n -> if n == 0 then 0 else length (filter (\\x -> f (n - 1) == 0) [1]) - 1 in f 30000",
        "let f = \\n -> if n == 0 then 0 else foldl (\\a x -> f (n - 1)) 0 [1] in f 30000",
        "let f = \\n -> if n == 0 then 0 else ((\\x -> x) . f) (n - 1) in f 30000",
        "let f = \\n -> if n == 0 then 0 else (head . map f) [(n - 1)] in f 30000",
    ] {
        check!(i, cfg, src, Value::int(0));
    }
}

/// Run `f` on a thread with a stack of `mb` megabytes.
fn on_stack(mb: usize, f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(mb * 1024 * 1024)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn deep_linear_history_takes_no_native_stack_per_commit() {
    // `commits` recursed through `map` once per level of history, and
    // `labelled`, `trunk` and `immutable` walk the history with it. The
    // binary evaluates `immutable` on every run (§7.2), so a long linear
    // history made every command overflow the stack and abort (§4.1). The
    // chain is built in the language, its deepest commit labelled `main`,
    // and seen from the top; the stack is a small fraction of the binary's,
    // so native stack spent per commit shows at a depth a test can afford.
    on_stack(16, || {
        let (mut i, cfg) = make_interp();
        let n = 3000;
        let src = format!(
            "let leaf = {{ root = {{ files = [], message = \"\", labels = [\"main\"], id = @ }}, children = [] }}; \
             chain = foldl (\\t k -> {{ root = {{ files = [], message = \"\", labels = [], id = @ }}, children = [t] }}) leaf (range 1 {n}); \
             repo = {{ root = chain.root, children = chain.children, context = [] }} \
             in [(length (commits repo)) (length (trunk repo)) (length (immutable repo))]"
        );
        check!(
            i,
            cfg,
            &src,
            Value::list(vec![Value::int(n), Value::int(1), Value::int(n)])
        );
    });
}

#[test]
fn history_walks_take_time_linear_in_the_length_of_history() {
    // `commits` was `\t -> t.root :: (concat (map commits t.children) or [])`
    // and `ancestors` consed the focus onto the ancestors of `up repo`. `::`
    // and `concat` build a new list, so every level copied the whole list
    // below it and both were quadratic in the depth of the history. The
    // binary evaluates `immutable` on every run (§7.2), which walks the
    // history with both through `labelled`, `trunk` and `ancestorsOf`: `j 1`
    // took a minute on a linear history of 32000 commits. Here the history
    // is seen from the top, its deepest commit labelled `main`, so
    // `immutable` walks it with both. In linear time that takes seconds in a
    // debug build; in quadratic time, many minutes. The stack is the
    // binary's, since `by` and dropping the chain take native stack per
    // level.
    let n = 100_000;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .stack_size(512 * 1024 * 1024)
        .spawn(move || {
            let (mut i, cfg) = make_interp();
            let src = format!(
                "let commit = \\l -> {{ files = [], message = \"\", labels = l, id = @ }}; \
                 chain = foldl (\\t k -> {{ root = commit [], children = [t] }}) \
                               ({{ root = commit [\"main\"], children = [] }}) (range 0 {n}); \
                 repo = {{ root = chain.root, children = chain.children, context = [] }} \
                 in [(length (commits repo)) (length (immutable repo))]"
            );
            let got = ev(&mut i, &cfg, &src).map(|v| j::show::show(&i, &v).unwrap());
            let _ = tx.send(got);
        })
        .unwrap();
    let limit = std::time::Duration::from_secs(60);
    match rx.recv_timeout(limit) {
        Ok(got) => assert_eq!(got, Ok(format!("[{m} {m}]", m = n + 1))),
        Err(_) => panic!("walking {} commits took more than {:?}", n, limit),
    }
}

#[test]
fn commits_gives_what_its_recursive_definition_gave_on_any_tree() {
    // `commits` is the builtin `subtreeCommits`, which must give what the
    // recursive definition it replaced gives (§4.9), on malformed trees too.
    // That definition's `or []` catches a crash anywhere under a node: only
    // a top without `root` crashes, and a node whose `children` is not a
    // list of values with a `root` contributes its own root alone.
    let (mut i, cfg) = make_interp();
    let old = "let old = \\t -> t.root :: (concat (map old t.children) or []) in";
    let ints = |xs: &[i64]| Value::list(xs.iter().map(|&x| Value::int(x)).collect());
    for (t, want) in [
        ("({ root = 1, children = [] })", ints(&[1])),
        (
            "({ root = 1, children = [({ root = 2, children = [({ root = 3, children = [] })] }) \
                                     ({ root = 4, children = [] })] })",
            ints(&[1, 2, 3, 4]),
        ),
        ("({ root = 1, children = [], context = [] })", ints(&[1])),
        ("({ root = 1, children = 5 })", ints(&[1])),
        ("({ root = 1 })", ints(&[1])),
        ("({ root = 1, children = [({ root = 2, children = [] }) 7] })", ints(&[1])),
        (
            "({ root = 1, children = [({ root = 2, children = [({ root = 3 })] }) \
                                     ({ root = 4, children = [5] })] })",
            ints(&[1, 2, 3, 4]),
        ),
        (
            "({ root = 1, children = [({ root = 2, children = [({ children = [] })] }) \
                                     ({ root = 4, children = [({ root = 5, children = \"x\" })] })] })",
            ints(&[1, 2, 4, 5]),
        ),
    ] {
        check!(i, cfg, &format!("commits {}", t), want);
        check!(i, cfg, &format!("{} old {}", old, t), want);
    }
    for (t, msg) in [
        ("5", "crash: cannot select field `root` from a Int"),
        ("({ children = [] })", "crash: record has no field `root`"),
    ] {
        assert_eq!(crash(&mut i, &cfg, &format!("commits {}", t)), msg);
        assert_eq!(crash(&mut i, &cfg, &format!("{} old {}", old, t)), msg);
    }
}

#[test]
fn builtins_that_apply_functions_keep_their_semantics() {
    let (mut i, cfg) = make_interp();
    let ints = |xs: &[i64]| Value::list(xs.iter().map(|&x| Value::int(x)).collect());
    // elements are visited left to right, so the first crash is the first
    // element's (§4: evaluation is left to right)
    for (src, msg) in [
        ("map (\\x -> crash (show x)) [1 2]", "1"),
        ("filter (\\x -> crash (show x)) [1 2]", "1"),
        ("foldl (\\a x -> crash (show x)) 0 [1 2]", "1"),
        ("filter (\\x -> x) [true 1]", "filter: predicate returned a Int"),
        ("(not . head) []", "head: empty list"),
    ] {
        let m = crash(&mut i, &cfg, src);
        assert!(m.ends_with(msg), "{} => {}", src, m);
    }
    // a crash in a function they apply unwinds to the nearest `or`, outside
    // them or inside the function
    check!(i, cfg, "map (\\x -> if x == 2 then crash \"two\" else x) [1 2 3] or [0]", ints(&[0]));
    check!(i, cfg, "map (\\x -> (if x == 2 then crash \"two\" else x) or 0) [1 2 3]", ints(&[1, 0, 3]));
    check!(i, cfg, "filter (\\x -> x > 1 or false) [1 2 3]", ints(&[2, 3]));
    check!(i, cfg, "foldl (\\a x -> (a + head []) or x) 0 [1 2 3]", Value::int(3));
    check!(i, cfg, "((\\x -> x + 1) . (\\x -> head [] or x)) 1", Value::int(2));
    // nested, empty, and partially applied
    check!(i, cfg, "map (map ((+) 1)) [[1] [] [2 3]]", Value::list(vec![ints(&[2]), ints(&[]), ints(&[3, 4])]));
    check!(i, cfg, "map id []", ints(&[]));
    check!(i, cfg, "filter (\\x -> true) []", ints(&[]));
    check!(i, cfg, "foldl (\\a x -> crash \"no\") 5 []", Value::int(5));
    check!(i, cfg, "foldl (\\a x -> a * 10 + x) 0 [1 2 3]", Value::int(123));
    check!(i, cfg, "(map (\\x -> x * 2) . filter (\\x -> x > 1)) [1 2 3]", ints(&[4, 6]));
    // the body of the function is evaluated on each application (§4.10)
    check!(i, cfg, "let ids = map (\\x -> @) [1 2 3] in nth 0 ids /= nth 2 ids", Value::Bool(true));
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
    assert!(matches!(v, Value::Id(_)), "got {}", j::show::show(&i, &v).unwrap());
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

/// The examples of a markdown file that document a result: the line each
/// starts on, the expression, and the text after its `=>`. An example starts
/// at column 0 in a fenced block (a `> ` quote stripped) and continues on the
/// indented lines below it, and on a line starting `in`; its result follows
/// `=>`, on its first line or an indented one, and continues on indented
/// lines. A `--` comment outside a text literal is dropped.
fn documented_results(md: &str) -> Vec<(usize, String, String)> {
    fn split(line: &str) -> (String, Option<String>) {
        let (mut in_text, mut escaped) = (false, false);
        for (at, c) in line.char_indices() {
            if in_text {
                match c {
                    _ if escaped => escaped = false,
                    '\\' => escaped = true,
                    '"' => in_text = false,
                    _ => {}
                }
            } else if c == '"' {
                in_text = true;
            } else if line[at..].starts_with("--") {
                return (line[..at].trim_end().to_string(), None);
            } else if line[at..].starts_with("=>") {
                let (rest, _) = split(&line[at + 2..]);
                return (line[..at].trim_end().to_string(), Some(rest.trim().to_string()));
            }
        }
        (line.trim_end().to_string(), None)
    }
    let mut out = Vec::new();
    // the example being read: its line, expression, and result so far
    let mut cur: Option<(usize, String, Option<String>)> = None;
    let mut in_block = false;
    for (n, raw) in md.lines().enumerate() {
        let line = raw.strip_prefix("> ").unwrap_or(if raw == ">" { "" } else { raw });
        let fence = line.trim_start().starts_with("```");
        let empty = line.trim().is_empty();
        let comment = line.trim_start().starts_with("--");
        let continues = line.starts_with(' ') || line.starts_with("in ");
        if fence {
            in_block = !in_block;
        }
        // an example ends at a blank line, at the next one, and with its block
        if !in_block || empty || !(comment || continues) {
            if let Some((at, e, Some(r))) = cur.take() {
                out.push((at, e, r));
            }
        }
        if !in_block || fence || empty || comment {
            continue;
        }
        let (text, result) = split(line);
        match &mut cur {
            None if !continues => cur = Some((n + 1, text, result)),
            None => {}
            Some((_, _, Some(r))) => {
                r.push(' ');
                r.push_str(text.trim());
            }
            Some((_, e, r)) => {
                if !text.trim().is_empty() {
                    e.push('\n');
                    e.push_str(&text);
                }
                *r = result;
            }
        }
    }
    out
}

#[test]
fn documented_examples_evaluate_to_their_results() {
    // docs/language.md listed two records side by side, `[{ id = 1 } { id = 2 }]`,
    // as a list of two; it is one record updated by the other (§3.4), and
    // `map (.id)` of it gave `[2]`, not the `[1 2]` documented. Each example
    // that documents a value must evaluate to it. One whose result is prose
    // (starting with a word that is no expression) or that names a commit
    // by its id is not checked here.
    let (mut i, cfg) = make_interp();
    let mut checked = 0;
    let mut wrong = Vec::new();
    for (name, md) in [
        ("docs/language.md", include_str!("../docs/language.md")),
        ("docs/base.md", include_str!("../docs/base.md")),
    ] {
        for (line, expr, result) in documented_results(md) {
            let got = ev(&mut i, &cfg, &expr);
            let want = ev(&mut i, &cfg, &result);
            let prose = result.starts_with(|c: char| c.is_alphabetic());
            match (got, want) {
                (Err(_), Err(_)) => {}
                // an id names no commit here
                (Err(e), _) if e == "id resolution" => {}
                (Ok(_), Err(_)) if prose => {}
                (Ok(_), Err(e)) | (Err(e), Ok(_)) => {
                    wrong.push(format!("{}:{}: {} => {}: {}", name, line, expr, result, e))
                }
                (Ok(got), Ok(want)) => match value_eq(&got, &want) {
                    Ok(true) => checked += 1,
                    Ok(false) => wrong.push(format!(
                        "{}:{}: {} => {}, not {}",
                        name,
                        line,
                        expr,
                        j::show::show(&i, &got).unwrap(),
                        result
                    )),
                    // functions do not compare
                    Err(_) => {}
                },
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    // the extraction still finds the examples
    assert!(checked >= 150, "only {} examples checked", checked);
}
