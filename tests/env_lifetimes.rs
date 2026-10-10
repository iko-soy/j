//! Recursive bindings retain escaping closures, but not unreachable cycles.

use j::config;
use j::domain::MemBackend;
use j::eval::Interp;
use j::parse::parse_expr;
use j::value::{Crash, Env, FunVal, Value};
use std::cell::RefCell;
use std::rc::{Rc, Weak};

thread_local! {
    static OBSERVED: RefCell<Vec<Weak<FunVal>>> = const { RefCell::new(Vec::new()) };
    static PEAK_ALIVE: RefCell<usize> = const { RefCell::new(0) };
}

fn observe(_: &mut Interp, args: &[Value]) -> Result<Value, Crash> {
    let Value::Fun(f) = &args[0] else { panic!("observe needs a function") };
    OBSERVED.with(|observed| {
        let mut observed = observed.borrow_mut();
        observed.push(Rc::downgrade(f));
        let alive = observed.iter().filter(|f| f.strong_count() != 0).count();
        PEAK_ALIVE.with(|peak| {
            let mut peak = peak.borrow_mut();
            *peak = (*peak).max(alive);
        });
    });
    Ok(args[0].clone())
}

fn interpreter() -> (Interp, config::Config) {
    OBSERVED.with(|observed| observed.borrow_mut().clear());
    PEAK_ALIVE.with(|peak| *peak.borrow_mut() = 0);
    let cfg = config::load_config(include_str!("../config.j")).unwrap();
    let mut interp = Interp::new(Rc::new(MemBackend::new()), cfg.shapes.clone(), Env::empty());
    config::eval_config(&mut interp, &cfg).unwrap();
    interp.globals = interp.globals.extend(vec![("observe".into(), Value::Fun(Rc::new(FunVal::Builtin {
        name: "observe".into(), arity: 1, args: vec![], f: observe, pending: None,
    })))]);
    (interp, cfg)
}

fn eval(interp: &mut Interp, cfg: &config::Config, src: &str) -> Value {
    let expr = parse_expr(src, Rc::new(cfg.global_names.clone())).unwrap();
    interp.eval(&Rc::new(expr), &interp.global_env()).unwrap()
}

fn alive() -> usize {
    OBSERVED.with(|observed| observed.borrow().iter().filter(|f| f.strong_count() != 0).count())
}

#[test]
fn unreachable_local_closures_and_their_payloads_are_reclaimed() {
    let (mut interp, cfg) = interpreter();
    let function = eval(&mut interp, &cfg, "let helper = observe (\\x -> x) in helper");
    assert_eq!(alive(), 1);
    drop(function);
    eval(&mut interp, &cfg, "0");
    assert_eq!(alive(), 0);

    // A recursive helper captures the block's shared frame, including a
    // payload such as a large snapshot, until the dead cycle is collected.
    eval(&mut interp, &cfg,
        "let payload = range 0 10000; helper = observe (\\x -> \
             if x == 0 then payload else helper (x - 1)) in 0");
    assert_eq!(alive(), 0);
}

#[test]
fn mutually_recursive_closures_escape_inside_shared_records_and_lists() {
    let (mut interp, cfg) = interpreter();
    let bundle = eval(&mut interp, &cfg,
        "let evenLocal = observe (\\n -> if n == 0 then true else \
             let scratch = observe (\\v -> v) in oddLocal (n - 1)); \
             oddLocal = observe (\\n -> if n == 0 then false else evenLocal (n - 1)) \
         in { callbacks = [evenLocal oddLocal evenLocal] }");
    eval(&mut interp, &cfg, "0");
    let callbacks = bundle.field("callbacks").unwrap();
    let function = callbacks.as_list().unwrap()[0].clone();
    drop(bundle);
    drop(callbacks);
    assert!(matches!(interp.apply(function.clone(), Value::int(2048)).unwrap(), Value::Bool(true)));
    assert_eq!(alive(), 2);
    drop(function);
    eval(&mut interp, &cfg, "0");
    assert_eq!(alive(), 0);
}

#[test]
fn nested_captures_and_partially_applied_functions_keep_their_scopes() {
    let (mut interp, cfg) = interpreter();
    let function = eval(&mut interp, &cfg,
        "let maker = \\n -> let recurse = observe (\\m -> \
             if m == 0 then n else recurse (m - 1)) \
         in \\addend -> recurse 1000 + addend in maker 40");
    eval(&mut interp, &cfg, "0");
    assert!(matches!(interp.apply(function.clone(), Value::int(2)).unwrap(), Value::Int(n) if n == 42.into()));
    drop(function);
    eval(&mut interp, &cfg, "0");
    assert_eq!(alive(), 0);

    let partial = eval(&mut interp, &cfg,
        "let helper = observe (\\a b -> a + b) in helper 40");
    eval(&mut interp, &cfg, "0");
    assert!(matches!(interp.apply(partial.clone(), Value::int(2)).unwrap(), Value::Int(n) if n == 42.into()));
    drop(partial);
    eval(&mut interp, &cfg, "0");
    assert_eq!(alive(), 0);
}

#[test]
fn wrappers_and_list_views_preserve_all_owned_references() {
    let (mut interp, cfg) = interpreter();
    for expression in [
        "let helper = observe (\\x -> x); wrapper = helper . helper in wrapper",
        "let helper = observe (\\x -> x); wrapper = helper or helper in wrapper",
        "let helper = observe (\\x -> x); wrapper = const helper in wrapper 0",
        "let bundle = { callbacks = [(observe (\\n -> if n == 0 then 7 else \
             (head bundle.callbacks) (n - 1)))] } in head bundle.callbacks",
    ] {
        let function = eval(&mut interp, &cfg, expression);
        eval(&mut interp, &cfg, "0");
        interp.apply(function.clone(), Value::int(7)).unwrap();
        assert!(alive() > 0);
        drop(function);
        eval(&mut interp, &cfg, "0");
        assert_eq!(alive(), 0, "{expression}");
    }

    let empty_view = eval(&mut interp, &cfg,
        "let helper = observe (\\x -> x); xs = [helper helper] in drop 2 xs");
    assert!(empty_view.as_list().unwrap().is_empty());
    eval(&mut interp, &cfg, "0");
    // An empty view still owns the two hidden entries in its backing Vec.
    assert_eq!(alive(), 1);
    drop(empty_view);
    eval(&mut interp, &cfg, "0");
    assert_eq!(alive(), 0);
}

#[test]
fn long_running_tail_calls_collect_dead_local_bindings_during_evaluation() {
    let (mut interp, cfg) = interpreter();
    // Acyclic helpers use immutable frames and need no cycle collection.
    // Make each local helper recursive so this exercises collection while
    // the enclosing tail-recursive evaluation is still running.
    let result = eval(&mut interp, &cfg,
        "let loop = \\n -> if n == 0 then 0 else \
             let helper = observe (\\x -> if x == 0 then 0 else helper (x - 1)); payload = range 0 32 \
             in loop (n - 1) in loop 4000");
    assert!(matches!(result, Value::Int(n) if n == 0.into()));
    assert_eq!(alive(), 0);
    PEAK_ALIVE.with(|peak| assert!(*peak.borrow() < 1024, "dead closures accumulate: {}", peak.borrow()));
}

#[test]
fn crashes_and_caught_crashes_release_abandoned_recursive_scopes() {
    let (mut interp, cfg) = interpreter();
    eval(&mut interp, &cfg,
        "(let helper = observe (\\x -> x) in crash \"discard\") or 42");
    assert_eq!(alive(), 0);
    let expr = parse_expr("let helper = observe (\\x -> x) in crash \"discard\"",
        Rc::new(cfg.global_names.clone())).unwrap();
    assert!(interp.eval(&Rc::new(expr), &interp.global_env()).is_err());
    assert_eq!(alive(), 0);
}

#[test]
fn dropping_the_interpreter_reclaims_its_global_recursive_environment() {
    let (interp, _) = interpreter();
    let Value::Fun(f) = interp.globals.lookup("new").unwrap() else { panic!() };
    let weak = Rc::downgrade(&f);
    drop(f);
    drop(interp);
    assert_eq!(weak.strong_count(), 0);
}

#[test]
fn returned_closures_outlive_the_interpreter_that_created_them() {
    let (mut interp, cfg) = interpreter();
    let function = eval(&mut interp, &cfg,
        "let recurse = observe (\\n -> if n == 0 then 42 else recurse (n - 1)) in recurse");
    drop(interp);
    assert_eq!(alive(), 1);
    let mut other = Interp::dummy();
    assert!(matches!(other.apply(function.clone(), Value::int(1000)).unwrap(), Value::Int(n) if n == 42.into()));
    drop(function);
    drop(other);
    assert_eq!(alive(), 0);
}

#[test]
fn collecting_deep_recursive_parent_chains_uses_a_bounded_native_stack() {
    std::thread::Builder::new().stack_size(2 * 1024 * 1024).spawn(|| {
        use j::ast::{Expr, Pattern, Source};
        let interp = Interp::dummy();
        let mut env = Env::empty();
        let source = Source::new(Rc::from("\\x -> true"), 0, 10);
        let body = Rc::new(Expr::Bool(true));
        let mut observed = Vec::new();
        for _ in 0..10000 {
            let (next, cell) = env.extend_rec(Rc::from(vec!["recursive".to_owned()]));
            let function = Rc::new(FunVal::Closure {
                name: None, params: vec![Pattern::Var("x".to_owned())],
                applied: 0, applied_args: vec![], body: body.clone(),
                env: next.clone(), src: source.clone(), pending: None,
            });
            observed.push(Rc::downgrade(&function));
            cell.borrow_mut().push(Value::Fun(function));
            env = next;
        }
        drop(env);
        drop(interp);
        assert!(observed.iter().all(|f| f.strong_count() == 0));
    }).unwrap().join().unwrap();
}
