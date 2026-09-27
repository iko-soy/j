//! Evaluator (§4). Strict, call-by-value, left-to-right, trampolined so deep
//! recursion cannot overflow the native stack.

use crate::ast::{Expr, Pattern};
use crate::domain::Backend;
use crate::shape::{check as contract_check, Contract, Shapes};
use crate::value::{Crash, Env, FunVal, ListVal, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub struct Interp {
    pub backend: Rc<dyn Backend>,
    pub shapes: Shapes,
    /// global names -> values (builtins + config definitions)
    pub globals: Env,
    /// contracts per global name
    pub contracts: HashMap<String, Rc<Contract>>,
    /// fresh id counter
    fresh: RefCell<u64>,
    /// the currently executing top-level definition (for crash reports)
    pub current_def: RefCell<Option<String>>,
    /// The definitions written as lambdas, by the address of the lambda's
    /// body, which every closure made from it shares: while that body runs,
    /// the definition is executing (§1.4), whatever name its closure has
    /// been bound to since (`prev = goto parents` runs goto's body). The body
    /// is held so its address cannot be reused.
    def_bodies: HashMap<*const Expr, (Rc<str>, Rc<Expr>)>,
    /// the repository as loaded, before snapshotting (for `validate`)
    pub old_repo: RefCell<Option<Value>>,
    /// the repository the expression was given (§1.2 step 4, after the
    /// snapshot), where displaying a `Commit` reads where it stands (§5.1);
    /// `old_repo` when unset
    pub given_repo: RefCell<Option<Value>>,
    /// Results of applying a config revset the binary itself calls (`trunk`,
    /// `immutable`), keyed by the identity of the Repo record they were
    /// applied to. Both are pure and both are asked for more than once per
    /// run — `immutable` by the focus check and again by rendering or
    /// persisting — and each evaluation is a full interpreted walk of the
    /// history. The `Rc` is kept so the address cannot be reused while it is
    /// a key (§4.1: definitions are pure, so the result cannot change).
    revsets: RefCell<Vec<(&'static str, Rc<crate::value::RecordMap>, Value)>>,
    /// visible ids in sorted order, built on first use. The backend's
    /// `visible_ids` allocates every id afresh, and a shortest-unique-prefix
    /// needs the whole set, so computing one per displayed commit was
    /// quadratic in both allocations and comparisons.
    sorted_ids: RefCell<Option<Rc<Vec<String>>>>,
}

/// length of the common leading run of two ids (ids are ASCII, so bytes)
fn common_prefix_len(a: &str, b: &str) -> usize {
    a.bytes()
        .zip(b.bytes())
        .take_while(|(x, y)| x == y)
        .count()
}

/// The shortest prefix of `id` that no other id in `sorted` shares, at least
/// four characters long. Only the neighbours of `id` in sorted order can share
/// its longest prefix, so two lookups settle it.
pub fn unique_prefix_in(sorted: &[String], id: &str) -> String {
    let pos = sorted.partition_point(|o| o.as_str() < id);
    let mut shared = 0usize;
    if pos > 0 {
        shared = shared.max(common_prefix_len(&sorted[pos - 1], id));
    }
    let mut next = pos;
    while next < sorted.len() && sorted[next] == id {
        next += 1;
    }
    if next < sorted.len() {
        shared = shared.max(common_prefix_len(&sorted[next], id));
    }
    let n = 4.max(shared + 1);
    if n >= id.len() {
        id.to_string()
    } else {
        id[..n].to_string()
    }
}

/// machine state
#[derive(Clone)]
pub(crate) enum State {
    Eval(Rc<Expr>, Env, Cont),
    Ret(Value, Cont),
    /// apply f to arg, then continue
    Apply(Value, Value, Cont),
}

#[derive(Clone)]
pub(crate) enum Cont {
    Halt,
    Arg(Rc<Expr>, Env, Rc<Cont>),
    Fun(Value, Rc<Cont>),
    If(Rc<Expr>, Rc<Expr>, Env, Rc<Cont>),
    LetRest {
        names: Vec<String>,
        exprs: Vec<Rc<Expr>>,
        acc: Vec<(String, Value)>,
        cell: Rc<RefCell<Vec<(String, Value)>>>,
        body: Rc<Expr>,
        env: Env,
        cont: Rc<Cont>,
    },
    /// short-circuit ops with the unevaluated rhs
    And(Rc<Expr>, Env, Rc<Cont>),
    Or2(Rc<Expr>, Env, Rc<Cont>),
    /// check the rhs of && / || evaluated to a Bool
    ExpectBool(&'static str, Rc<Cont>),
    /// `or` after both evaluated as functions: build the lifted function
    OrBoth(Value, Rc<Cont>),
    /// a shared continuation (used when both `or` outcomes continue the same way)
    Shared(Rc<Cont>),
    /// run the continuation's computation, catching crashes: on success pass
    /// the value to `on_ok`, on crash switch to `on_crash` (§4.6)
    Try {
        on_ok: Option<TryOk>,
        snapshot: u64,
        on_crash: Box<State>,
    },
    /// apply the returned function to this argument: the `labelled "name"`
    /// of a Labelled to the real arg
    ApplyTo(Value, Rc<Cont>),
    UpdateBase(Vec<(String, Rc<Expr>)>, Env, Rc<Cont>),
    UpdateFields {
        base: Value,
        names: Vec<String>,
        exprs: Vec<Rc<Expr>>,
        acc: Vec<(String, Value)>,
        env: Env,
        cont: Rc<Cont>,
    },
    RecordFields {
        names: Vec<String>,
        exprs: Vec<Rc<Expr>>,
        acc: Vec<(String, Value)>,
        env: Env,
        cont: Rc<Cont>,
    },
    ListElems {
        exprs: Vec<Rc<Expr>>,
        acc: Vec<Value>,
        env: Env,
        cont: Rc<Cont>,
    },
    /// check a fully-applied result against a contract expression
    CheckExpr {
        name: String,
        cexpr: Rc<crate::shape::ContractExpr>,
        cont: Rc<Cont>,
    },
    /// the function a signed definition's value returned when it ran out of
    /// parameters before its signature (a lambda's body, or a builtin or a
    /// composition given its last argument): make it the named partial
    /// application of `args` (§5.2), checked against the rest of `cexpr`
    NamePartial {
        name: String,
        cexpr: Rc<crate::shape::ContractExpr>,
        args: Vec<Value>,
        cont: Rc<Cont>,
    },
    /// after applying, check the (partial) result against the contract
    #[allow(dead_code)]
    CheckResult {
        name: String,
        contract: Rc<Contract>,
        supplied: usize,
        cont: Rc<Cont>,
    },
    /// the body of a definition's lambda is running (§1.4); passes its value
    /// on unchanged
    InDef {
        name: Rc<str>,
        cont: Rc<Cont>,
    },
    // The builtins that apply functions they are given run as steps of this
    // machine, not in a nested run, so a recursion through them takes no
    // native stack (§4.1; Interp::step_applying). Each frame below receives
    // one value, so `acc` is pushed to once per element and shared, not
    // copied, when the trampoline clones the continuation.
    /// `map f xs`, receiving `f` of the element at `idx`
    MapNext {
        f: Value,
        xs: ListVal,
        idx: usize,
        acc: Rc<RefCell<Vec<Value>>>,
        cont: Rc<Cont>,
    },
    /// `filter p xs`, receiving `p` of the element at `idx`
    FilterNext {
        p: Value,
        xs: ListVal,
        idx: usize,
        acc: Rc<RefCell<Vec<Value>>>,
        cont: Rc<Cont>,
    },
    /// `foldl f z xs`, receiving `f` applied to the accumulator, to be
    /// applied to the element at `idx`
    FoldlArg {
        f: Value,
        xs: ListVal,
        idx: usize,
        cont: Rc<Cont>,
    },
    /// `foldl f z xs`, receiving the accumulator after the element at `idx`
    FoldlAcc {
        f: Value,
        xs: ListVal,
        idx: usize,
        cont: Rc<Cont>,
    },
    /// the result of such a builtin, checked against its contract once the
    /// functions it applies have run (§4.13); `def` is the signed definition
    /// the builtin is the value of, which a crash under this frame is in (§1.4)
    BuiltinResult {
        cname: String,
        check: Option<Rc<crate::shape::ContractExpr>>,
        def: Option<String>,
        cont: Rc<Cont>,
    },
}

pub type EResult = Result<Value, Crash>;

/// what to do with a value whose evaluation was guarded by `Try`
#[derive(Clone)]
pub(crate) enum TryOk {
    /// the lhs of `or`: a function value lifts over the rhs, anything else is
    /// the result
    OrLhs {
        rhs: Rc<Expr>,
        env: Env,
        cont: Rc<Cont>,
    },
    /// the lhs of a lifted `or` applied to an argument: like `OrLhs`, with
    /// the rhs function still to be applied to the same argument
    OrFunLhs {
        rhs: Value,
        arg: Value,
        cont: Rc<Cont>,
    },
}

pub(crate) enum Run {
    Step(State),
    Done(Value),
    Crash(Crash),
}

/// the parent continuation Rc of `k`, for iterative drop and unwinding
fn parent_rc(k: &Cont) -> Option<&Rc<Cont>> {
    match k {
        Cont::Halt | Cont::Try { .. } => None,
        Cont::Arg(_, _, k) => Some(k),
        Cont::Fun(_, k) => Some(k),
        Cont::If(_, _, _, k) => Some(k),
        Cont::LetRest { cont, .. } => Some(cont),
        Cont::And(_, _, k) => Some(k),
        Cont::Or2(_, _, k) => Some(k),
        Cont::ExpectBool(_, k) => Some(k),
        Cont::OrBoth(_, k) => Some(k),
        Cont::Shared(k) => Some(k),
        Cont::ApplyTo(_, k) => Some(k),
        Cont::UpdateBase(_, _, k) => Some(k),
        Cont::UpdateFields { cont, .. } => Some(cont),
        Cont::RecordFields { cont, .. } => Some(cont),
        Cont::ListElems { cont, .. } => Some(cont),
        Cont::CheckExpr { cont, .. } => Some(cont),
        Cont::NamePartial { cont, .. } => Some(cont),
        Cont::CheckResult { cont, .. } => Some(cont),
        Cont::InDef { cont, .. } => Some(cont),
        Cont::MapNext { cont, .. } => Some(cont),
        Cont::FilterNext { cont, .. } => Some(cont),
        Cont::FoldlArg { cont, .. } => Some(cont),
        Cont::FoldlAcc { cont, .. } => Some(cont),
        Cont::BuiltinResult { cont, .. } => Some(cont),
    }
}

/// the parent continuation of `k`, for unwinding to the nearest Try on crash
pub(crate) fn pop_cont(k: &Cont) -> Option<Cont> {
    parent_rc(k).map(|r| (**r).clone())
}

/// The innermost definition whose application `k` continues (§1.4): a
/// signed definition's application runs under the `CheckExpr` of its result,
/// the body of a definition's lambda under an `InDef` naming it unless that
/// `CheckExpr` names it already, and the functions a builtin applies as a
/// signed definition's value under a `BuiltinResult` naming it; each stays on
/// the chain until the body has returned. Only used on a crash no `Try`
/// caught, so there is no `Try` on the chain to stop the walk early.
fn innermost_def(k: &Cont) -> Option<String> {
    let mut k = k;
    loop {
        match k {
            Cont::CheckExpr { name, .. }
            | Cont::BuiltinResult {
                def: Some(name), ..
            } => return Some(name.clone()),
            Cont::InDef { name, .. } => return Some(name.to_string()),
            _ => {}
        }
        k = parent_rc(k)?;
    }
}

/// the builtins that apply the functions they are given; they run as steps
/// of the machine (Interp::step_applying)
fn applies_functions(builtin: &str) -> bool {
    matches!(builtin, "map" | "filter" | "foldl" | "(.)")
}

/// a builtin's function argument
fn function_arg(v: &Value) -> Result<Value, Crash> {
    match v {
        Value::Fun(_) => Ok(v.clone()),
        v => Err(Crash::new(format!(
            "expected a function, got a {}",
            v.kind_name()
        ))),
    }
}

/// a builtin's list argument, sharing its elements
fn list_arg(v: &Value) -> Result<ListVal, Crash> {
    match v {
        Value::List(xs) => Ok(xs.clone()),
        v => Err(Crash::new(format!("expected a list, got a {}", v.kind_name()))),
    }
}

/// `c`, raised while applying the definition `def`, is in it unless
/// something nearer, a nested run, already named one (§1.4)
fn in_def(mut c: Crash, def: &str) -> Crash {
    if c.def.is_none() {
        c.def = Some(def.to_string());
    }
    c
}

impl Interp {
    pub fn new(backend: Rc<dyn Backend>, shapes: Shapes, globals: Env) -> Interp {
        Interp {
            backend,
            shapes,
            globals,
            contracts: HashMap::new(),
            fresh: RefCell::new(0),
            current_def: RefCell::new(None),
            def_bodies: HashMap::new(),
            old_repo: RefCell::new(None),
            given_repo: RefCell::new(None),
            revsets: RefCell::new(Vec::new()),
            sorted_ids: RefCell::new(None),
        }
    }

    /// Shortest prefix of `id` unique among the visible ids, minimum four
    /// (§5.1). The sorted id list is built once per run.
    pub fn unique_prefix(&self, id: &str) -> String {
        let cached = self.sorted_ids.borrow().clone();
        let sorted = match cached {
            Some(s) => s,
            None => {
                let mut ids = self.backend.visible_ids();
                ids.sort();
                let rc = Rc::new(ids);
                *self.sorted_ids.borrow_mut() = Some(rc.clone());
                rc
            }
        };
        unique_prefix_in(&sorted, id)
    }

    /// Apply one of the config revsets the binary calls itself, reusing the
    /// result when the same Repo value has already been asked (§7.5).
    pub fn apply_cached_revset(&mut self, name: &'static str, repo: &Value) -> EResult {
        let key = match repo {
            Value::Record(m) => Some(m.clone()),
            _ => None,
        };
        if let Some(k) = &key {
            for (n, r, v) in self.revsets.borrow().iter() {
                if *n == name && Rc::ptr_eq(r, k) {
                    return Ok(v.clone());
                }
            }
        }
        let f = self
            .globals
            .lookup(name)
            .ok_or_else(|| Crash::new(format!("`{}` is not defined", name)))?;
        let v = self.apply(f, repo.clone())?;
        if let Some(k) = key {
            self.revsets.borrow_mut().push((name, k, v.clone()));
        }
        Ok(v)
    }

    /// An interpreter shell used only for recursive rendering; panics if any
    /// backend interaction is attempted.
    pub fn dummy() -> Interp {
        Interp {
            backend: Rc::new(crate::domain::MemBackend::new()),
            shapes: Shapes::new(),
            globals: Env::empty(),
            contracts: HashMap::new(),
            fresh: RefCell::new(0),
            current_def: RefCell::new(None),
            def_bodies: HashMap::new(),
            old_repo: RefCell::new(None),
            given_repo: RefCell::new(None),
            revsets: RefCell::new(Vec::new()),
            sorted_ids: RefCell::new(None),
        }
    }

    pub fn mint_id(&self) -> String {
        // minted ids must not collide with stored ids across runs; seed from
        // time and pid, then count
        let mut n = self.fresh.borrow_mut();
        if *n == 0 {
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0x9e3779b97f4a7c15)
                ^ (std::process::id() as u64).wrapping_mul(0x9e3779b97f4a7c15);
            *n = seed | 1;
        }
        let x = *n;
        *n = n.wrapping_add(0x9e3779b97f4a7c15);
        // 64 bits over 16 letters (k..z): 16 hex digits = 32 chars
        let mut s = String::with_capacity(32);
        for i in 0..32 {
            let d = ((x >> ((i % 16) * 4)) & 0xf) as u8;
            s.push((b'k' + d) as char);
        }
        s
    }

    pub fn global_env(&self) -> Env {
        self.globals.clone()
    }

    /// Record `expr`, the expression of the definition `name`: the body of
    /// the lambda it is written as runs as `name`'s (§1.4), and so does the
    /// body of each lambda that body is in turn (`\a -> \b -> e`). A lambda
    /// in parentheses or ending a `let` is the definition's lambda too.
    pub fn name_bodies(&mut self, name: &str, expr: &Rc<Expr>) {
        let name: Rc<str> = Rc::from(name);
        let mut e = expr;
        loop {
            match &**e {
                Expr::Paren(inner) | Expr::Let(_, inner) => e = inner,
                Expr::Lambda(_, body, _) => {
                    self.def_bodies
                        .insert(Rc::as_ptr(body), (name.clone(), body.clone()));
                    e = body;
                }
                _ => break,
            }
        }
    }

    /// Main entry: evaluate an expression in an environment.
    pub fn eval(&mut self, e: &Rc<Expr>, env: &Env) -> EResult {
        self.run(State::Eval(e.clone(), env.clone(), Cont::Halt))
    }

    pub fn apply(&mut self, f: Value, arg: Value) -> EResult {
        self.run(State::Apply(f, arg, Cont::Halt))
    }

    fn decorate(&self, mut c: Crash) -> Crash {
        if c.def.is_none() {
            c.def = self.current_def.borrow().clone();
        }
        c
    }

    /// `c`, raised while applying the builtin `builtin` under the contract
    /// named `cname`, is in that definition when the builtin is a signed
    /// definition's value (`squash = abandon . …`). A builtin's own name, or
    /// a composition's made from its operands', names no definition, and the
    /// crash is in whichever definition applied it (§1.4).
    fn blame_builtin(&self, c: Crash, builtin: &str, cname: &str) -> Crash {
        match self.builtin_def(builtin, cname) {
            Some(def) => in_def(c, &def),
            None => c,
        }
    }

    /// the definition a crash while applying `builtin` under the contract
    /// named `cname` is in, as blame_builtin decides
    fn builtin_def(&self, builtin: &str, cname: &str) -> Option<String> {
        if cname != builtin && self.contracts.contains_key(cname) {
            Some(cname.to_string())
        } else {
            None
        }
    }

    /// `map`, `filter`, `foldl` or a composition node given all its
    /// arguments, continuing in `k`. They apply the functions they are given,
    /// so they run as steps of this machine: applying them in a nested run
    /// put every level of a recursion through one of them on the native
    /// stack, which overflowed and aborted where plain recursion runs (§4.1).
    /// Evaluation stays left to right, element by element (§4).
    fn step_applying(&mut self, builtin: &str, args: &[Value], k: Cont) -> Result<Run, Crash> {
        match builtin {
            // (f . g) x = f (g x) (§4.9), whatever x is: a function argument
            // is applied like any other, and g's contract checks it (§4.13).
            // `squash = abandon . contract everything` waits for the
            // repository because it is this node, not because anything
            // composes on a function argument.
            "(.)" => Ok(Run::Step(State::Apply(
                args[1].clone(),
                args[2].clone(),
                Cont::Fun(args[0].clone(), Rc::new(k)),
            ))),
            "map" | "filter" => {
                let f = function_arg(&args[0])?;
                let xs = list_arg(&args[1])?;
                if xs.is_empty() {
                    return Ok(Run::Step(State::Ret(Value::list(vec![]), k)));
                }
                let x = xs[0].clone();
                let acc = Rc::new(RefCell::new(Vec::new()));
                let cont = Rc::new(k);
                let next = if builtin == "map" {
                    Cont::MapNext {
                        f: f.clone(),
                        xs,
                        idx: 0,
                        acc,
                        cont,
                    }
                } else {
                    Cont::FilterNext {
                        p: f.clone(),
                        xs,
                        idx: 0,
                        acc,
                        cont,
                    }
                };
                Ok(Run::Step(State::Apply(f, x, next)))
            }
            "foldl" => {
                let f = function_arg(&args[0])?;
                let xs = list_arg(&args[2])?;
                if xs.is_empty() {
                    return Ok(Run::Step(State::Ret(args[1].clone(), k)));
                }
                let next = Cont::FoldlArg {
                    f: f.clone(),
                    xs,
                    idx: 0,
                    cont: Rc::new(k),
                };
                Ok(Run::Step(State::Apply(f, args[1].clone(), next)))
            }
            _ => Err(Crash::new(format!(
                "internal: `{}` does not apply functions",
                builtin
            ))),
        }
    }

    fn run(&mut self, st: State) -> EResult {
        match self.trampoline(st) {
            Ok(v) => Ok(v),
            Err(c) => Err(self.decorate(c)),
        }
    }

    fn trampoline(&mut self, st: State) -> EResult {
        let mut st = st;
        // the continuation that produced the current state; used to unwind to
        // the nearest Try on crash
        let mut active: Cont;
        loop {
            let outcome = match st {
                State::Ret(v, k) => {
                    active = k.clone();
                    self.step_ret(v, k)
                }
                State::Eval(e, env, k) => {
                    active = k.clone();
                    self.step_eval(e, env, k)
                }
                State::Apply(f, a, k) => {
                    active = k.clone();
                    self.step_apply(f, a, k)
                }
            };
            match outcome {
                Run::Step(next) => st = next,
                Run::Done(v) => return Ok(v),
                Run::Crash(c) => {
                    // Unwind to the nearest Try continuation, restoring the
                    // fresh-id snapshot so a caught crash leaves no trace.
                    // A Try cannot catch a crash raised by its own handler:
                    // every `on_crash` state continues through `Cont::Shared`
                    // holding the continuation *outside* the Try, so the Try
                    // is no longer on the chain once the handler runs.
                    let mut k = Some(active.clone());
                    let caught = loop {
                        match k {
                            Some(Cont::Try {
                                snapshot, on_crash, ..
                            }) => {
                                *self.fresh.borrow_mut() = snapshot;
                                break Some(*on_crash);
                            }
                            Some(other) => k = pop_cont(&other),
                            None => break None,
                        }
                    };
                    match caught {
                        Some(next) => st = next,
                        None => {
                            let mut c = c;
                            if c.def.is_none() {
                                c.def = innermost_def(&active);
                            }
                            return Err(c);
                        }
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------
    fn step_eval(&mut self, e: Rc<Expr>, env: Env, k: Cont) -> Run {
        match &*e {
            Expr::Int(n) => Run::Step(State::Ret(Value::Int(n.clone()), k)),
            Expr::Text(s) => Run::Step(State::Ret(Value::text(s.clone()), k)),
            Expr::Bool(b) => Run::Step(State::Ret(Value::Bool(*b), k)),
            Expr::Id(id) => Run::Step(State::Ret(Value::Id(Rc::new(id.clone())), k)),
            Expr::IdLit(_) => Run::Crash(Crash::new("internal: unresolved id literal")),
            Expr::NewId => {
                let id = self.mint_id();
                Run::Step(State::Ret(Value::Id(Rc::new(id)), k))
            }
            Expr::LabelLit(name) => {
                let labelled = match env.lookup("labelled") {
                    Some(v) => v,
                    None => return Run::Crash(Crash::new("`labelled` is not defined")),
                };
                Run::Step(State::Ret(
                    Value::Fun(Rc::new(FunVal::Labelled(name.clone(), labelled))),
                    k,
                ))
            }
            Expr::PathLit(comps) => Run::Step(State::Ret(
                Value::list(comps.iter().map(|c| Value::text(c.clone())).collect()),
                k,
            )),
            Expr::Var(n) => match env.lookup(n) {
                Some(v) => Run::Step(State::Ret(v, k)),
                None => {

                    Run::Crash(Crash::new(format!("unbound name `{}`", n)))
                }
            },
            Expr::TypeName(n) => match self.shapes.shape_of(n) {
                Some(s) => Run::Step(State::Ret(Value::Shape(Rc::new(s)), k)),
                None => Run::Crash(Crash::new(format!("`{}` does not name a usable shape", n))),
            },
            Expr::SelectorFun(f) => Run::Step(State::Ret(crate::builtins::make_selector(f), k)),
            Expr::Lambda(params, body, src) => Run::Step(State::Ret(
                Value::Fun(Rc::new(FunVal::Closure {
                    name: None,
                    params: params.clone(),
                    applied: 0,
                    applied_args: Vec::new(),
                    body: body.clone(),
                    env,
                    src: src.clone(),
                    pending: None,
                })),
                k,
            )),
            Expr::If(c, t, f) => Run::Step(State::Eval(
                c.clone(),
                env.clone(),
                Cont::If(t.clone(), f.clone(), env, Rc::new(k)),
            )),
            Expr::Let(bs, body) => {
                if bs.is_empty() {
                    return Run::Step(State::Eval(body.clone(), env, k));
                }
                // the bindings are evaluated in dependency order, not source
                // order, and a cycle among them is a crash (§4.1)
                let order = match crate::config::let_order(bs) {
                    Some(order) => order,
                    None => return Run::Crash(Crash::new("let: a cycle among the bindings")),
                };
                let names: Vec<String> = order.iter().map(|&i| bs[i].0.clone()).collect();
                let exprs: Vec<Rc<Expr>> = order.iter().map(|&i| bs[i].1.clone()).collect();
                // one recursive frame shared by every binding and the body, so
                // the block is mutually recursive (§4.1)
                let (env2, cell) = env.extend_rec();
                Run::Step(State::Eval(
                    exprs[0].clone(),
                    env2.clone(),
                    Cont::LetRest {
                        names,
                        exprs,
                        acc: Vec::new(),
                        cell,
                        body: body.clone(),
                        env: env2,
                        cont: Rc::new(k),
                    },
                ))
            }
            Expr::App(f, x) => Run::Step(State::Eval(
                f.clone(),
                env.clone(),
                Cont::Arg(x.clone(), env, Rc::new(k)),
            )),
            Expr::BinOp(op, a, b) => {
                if *op == "&&" {
                    return Run::Step(State::Eval(
                        a.clone(),
                        env.clone(),
                        Cont::And(b.clone(), env, Rc::new(k)),
                    ));
                }
                if *op == "||" {
                    return Run::Step(State::Eval(
                        a.clone(),
                        env.clone(),
                        Cont::Or2(b.clone(), env, Rc::new(k)),
                    ));
                }
                let fv = match env.lookup(op) {
                    Some(v) => v,
                    None => return Run::Crash(Crash::new(format!("unbound name `{}`", op))),
                };
                // eval a, apply the operator to it, then apply the result to b
                Run::Step(State::Eval(
                    a.clone(),
                    env.clone(),
                    Cont::Fun(fv, Rc::new(Cont::Arg(b.clone(), env, Rc::new(k)))),
                ))
            }
            Expr::Or(a, b) => {
                // evaluate the lhs, catching crashes (§4.6); the Try
                // continuation keeps this on the heap, so `or`-recursive
                // walks (top/tip) do not consume native stack
                let k = Rc::new(k);
                Run::Step(State::Eval(
                    a.clone(),
                    env.clone(),
                    Cont::Try {
                        on_ok: Some(TryOk::OrLhs {
                            rhs: b.clone(),
                            env: env.clone(),
                            cont: k.clone(),
                        }),
                        snapshot: *self.fresh.borrow(),
                        on_crash: Box::new(State::Eval(b.clone(), env, Cont::Shared(k))),
                    },
                ))
            }
            Expr::Select(base, f) => {
                let sel = crate::builtins::make_selector(f);
                Run::Step(State::Eval(base.clone(), env, Cont::Fun(sel, Rc::new(k))))
            }
            Expr::Update(base, fields) => Run::Step(State::Eval(
                base.clone(),
                env.clone(),
                Cont::UpdateBase(fields.clone(), env, Rc::new(k)),
            )),
            Expr::Record(fields) => {
                if fields.is_empty() {
                    let m = crate::value::RecordMap::new();
                    return Run::Step(State::Ret(Value::Record(Rc::new(m)), k));
                }
                let names: Vec<String> = fields.iter().map(|(n, _)| n.clone()).collect();
                let exprs: Vec<Rc<Expr>> = fields.iter().map(|(_, v)| v.clone()).collect();
                Run::Step(State::Eval(
                    exprs[0].clone(),
                    env.clone(),
                    Cont::RecordFields {
                        names,
                        exprs,
                        acc: Vec::new(),
                        env,
                        cont: Rc::new(k),
                    },
                ))
            }
            Expr::List(elems) => {
                if elems.is_empty() {
                    return Run::Step(State::Ret(Value::list(vec![]), k));
                }
                Run::Step(State::Eval(
                    elems[0].clone(),
                    env.clone(),
                    Cont::ListElems {
                        exprs: elems.clone(),
                        acc: Vec::new(),
                        env,
                        cont: Rc::new(k),
                    },
                ))
            }
            Expr::Paren(inner) => Run::Step(State::Eval(inner.clone(), env, k)),
            Expr::Crash => unreachable!(),
        }
    }

    // ------------------------------------------------------------------
    fn step_ret(&mut self, v: Value, k: Cont) -> Run {
        match k {
            Cont::Halt => Run::Done(v),
            Cont::Arg(x, env, k) => Run::Step(State::Eval(x, env, Cont::Fun(v, k))),
            Cont::Fun(f, k) => Run::Step(State::Apply(f, v, (*k).clone())),
            Cont::If(t, f, env, k) => match v {
                Value::Bool(true) => Run::Step(State::Eval(t, env, (*k).clone())),
                Value::Bool(false) => Run::Step(State::Eval(f, env, (*k).clone())),
                _ => Run::Crash(Crash::new(format!(
                    "if: expected a Bool condition, got a {}",
                    v.kind_name()
                ))),
            },
            Cont::LetRest {
                names,
                exprs,
                mut acc,
                cell,
                body,
                env,
                cont,
            } => {
                let idx = acc.len();
                let pair = (names[idx].clone(), v);
                cell.borrow_mut().push(pair.clone());
                acc.push(pair);
                if acc.len() == names.len() {
                    Run::Step(State::Eval(body, env, (*cont).clone()))
                } else {
                    let next = exprs[acc.len()].clone();
                    Run::Step(State::Eval(
                        next,
                        env.clone(),
                        Cont::LetRest {
                            names,
                            exprs,
                            acc,
                            cell,
                            body,
                            env,
                            cont,
                        },
                    ))
                }
            }
            Cont::And(b, env, k) => match v {
                Value::Bool(false) => Run::Step(State::Ret(Value::Bool(false), (*k).clone())),
                Value::Bool(true) => Run::Step(State::Eval(b, env, Cont::ExpectBool("&&", k))),
                _ => Run::Crash(Crash::new(format!("&&: expected Bool, got a {}", v.kind_name()))),
            },
            Cont::Or2(b, env, k) => match v {
                Value::Bool(true) => Run::Step(State::Ret(Value::Bool(true), (*k).clone())),
                Value::Bool(false) => Run::Step(State::Eval(b, env, Cont::ExpectBool("||", k))),
                _ => Run::Crash(Crash::new(format!("||: expected Bool, got a {}", v.kind_name()))),
            },
            Cont::ExpectBool(op, k) => match v {
                Value::Bool(_) => Run::Step(State::Ret(v, (*k).clone())),
                _ => Run::Crash(Crash::new(format!(
                    "{}: expected Bool, got a {}",
                    op,
                    v.kind_name()
                ))),
            },
            Cont::OrBoth(a, k) => {
                if matches!(v, Value::Fun(_)) {
                    Run::Step(State::Ret(Value::Fun(Rc::new(FunVal::OrFun(a, v))), (*k).clone()))
                } else {
                    Run::Step(State::Ret(a, (*k).clone()))
                }
            }
            Cont::Shared(k) => {
                // both `or` outcomes continue here; unwrap the shared
                // continuation and continue it plainly
                Run::Step(State::Ret(v, (*k).clone()))
            }
            Cont::Try {
                on_ok,
                snapshot: _,
                on_crash: _,
            } => {
                // the guarded computation succeeded; route the value
                match on_ok {
                    Some(TryOk::OrLhs { rhs, env, cont }) => {
                        if matches!(v, Value::Fun(_)) {
                            Run::Step(State::Eval(
                                rhs,
                                env,
                                Cont::OrBoth(v, Rc::new(Cont::Shared(cont))),
                            ))
                        } else {
                            Run::Step(State::Ret(v, Cont::Shared(cont)))
                        }
                    }
                    Some(TryOk::OrFunLhs { rhs, arg, cont }) => {
                        // `f x or g x` (§4.6): when `f x` is itself a
                        // function, apply `g` too and lift again, so the
                        // fallback keeps guarding the later arguments
                        if matches!(v, Value::Fun(_)) {
                            Run::Step(State::Apply(
                                rhs,
                                arg,
                                Cont::OrBoth(v, Rc::new(Cont::Shared(cont))),
                            ))
                        } else {
                            Run::Step(State::Ret(v, (*cont).clone()))
                        }
                    }
                    None => Run::Crash(Crash::new("internal: Try with no success handler")),
                }
            }
            Cont::ApplyTo(real, k) => Run::Step(State::Apply(v, real, (*k).clone())),
            Cont::UpdateBase(fields, env, k) => {
                if !matches!(v, Value::Record(_)) {
                    return Run::Crash(Crash::new(format!(
                        "cannot update fields of a {}",
                        v.kind_name()
                    )));
                }
                let names: Vec<String> = fields.iter().map(|(n, _)| n.clone()).collect();
                let exprs: Vec<Rc<Expr>> = fields.iter().map(|(_, e)| e.clone()).collect();
                if exprs.is_empty() {
                    return Run::Step(State::Ret(v, (*k).clone()));
                }
                Run::Step(State::Eval(
                    exprs[0].clone(),
                    env.clone(),
                    Cont::UpdateFields {
                        base: v,
                        names,
                        exprs,
                        acc: Vec::new(),
                        env,
                        cont: k,
                    },
                ))
            }
            Cont::UpdateFields {
                base,
                names,
                exprs,
                mut acc,
                env,
                cont,
            } => {
                let idx = acc.len();
                acc.push((names[idx].clone(), v));
                if acc.len() == names.len() {
                    let mut m = match &base {
                        Value::Record(m) => m.as_ref().clone(),
                        _ => unreachable!(),
                    };
                    for (n, val) in acc {
                        if !m.contains_key(&n) {
                            return Run::Crash(Crash::new(format!(
                                "cannot update: record has no field `{}`",
                                n
                            )));
                        }
                        m.insert(n, val);
                    }
                    Run::Step(State::Ret(Value::Record(Rc::new(m)), (*cont).clone()))
                } else {
                    let next = exprs[acc.len()].clone();
                    Run::Step(State::Eval(
                        next,
                        env.clone(),
                        Cont::UpdateFields {
                            base,
                            names,
                            exprs,
                            acc,
                            env,
                            cont,
                        },
                    ))
                }
            }
            Cont::RecordFields {
                names,
                exprs,
                mut acc,
                env,
                cont,
            } => {
                let idx = acc.len();
                acc.push((names[idx].clone(), v));
                if acc.len() == names.len() {
                    let m: crate::value::RecordMap = acc.into_iter().collect();
                    Run::Step(State::Ret(Value::Record(Rc::new(m)), (*cont).clone()))
                } else {
                    let next = exprs[acc.len()].clone();
                    Run::Step(State::Eval(
                        next,
                        env.clone(),
                        Cont::RecordFields {
                            names,
                            exprs,
                            acc,
                            env,
                            cont,
                        },
                    ))
                }
            }
            Cont::ListElems {
                exprs,
                mut acc,
                env,
                cont,
            } => {
                acc.push(v);
                if acc.len() == exprs.len() {
                    Run::Step(State::Ret(Value::list(acc), (*cont).clone()))
                } else {
                    let next = exprs[acc.len()].clone();
                    Run::Step(State::Eval(
                        next,
                        env.clone(),
                        Cont::ListElems {
                            exprs,
                            acc,
                            env,
                            cont,
                        },
                    ))
                }
            }
            Cont::CheckExpr { name, cexpr, cont } => {
                if let Err(cr) = cexpr.check_result(&self.shapes, &name, &v) {
                    return Run::Crash(cr);
                }
                Run::Step(State::Ret(v, (*cont).clone()))
            }
            Cont::NamePartial {
                name,
                cexpr,
                args,
                cont,
            } => {
                let named = crate::value::named_apply(v, args, &name, cexpr);
                Run::Step(State::Ret(named, (*cont).clone()))
            }
            Cont::CheckResult {
                name,
                contract,
                supplied,
                cont,
            } => {
                if supplied < contract.params.len() {
                    if !matches!(v, Value::Fun(_)) {
                        return Run::Crash(Crash::new(format!(
                            "contract: {} expected a function, got {}",
                            name,
                            v.kind_name()
                        )));
                    }
                    // the result is itself checked against the next parameter
                    // when it is applied further (deferred)
                } else if let Err(msg) = contract_check(&self.shapes, &contract.result, &v) {
                    return Run::Crash(Crash::new(format!(
                        "contract: {}: result {}",
                        name, msg
                    )));
                }
                Run::Step(State::Ret(v, (*cont).clone()))
            }
            Cont::InDef { cont, .. } => Run::Step(State::Ret(v, (*cont).clone())),
            Cont::MapNext {
                f,
                xs,
                idx,
                acc,
                cont,
            } => {
                acc.borrow_mut().push(v);
                let idx = idx + 1;
                if idx < xs.len() {
                    let x = xs[idx].clone();
                    let next = Cont::MapNext {
                        f: f.clone(),
                        xs,
                        idx,
                        acc,
                        cont,
                    };
                    Run::Step(State::Apply(f, x, next))
                } else {
                    Run::Step(State::Ret(Value::list(acc.take()), (*cont).clone()))
                }
            }
            Cont::FilterNext {
                p,
                xs,
                idx,
                acc,
                cont,
            } => {
                match v {
                    Value::Bool(true) => acc.borrow_mut().push(xs[idx].clone()),
                    Value::Bool(false) => (),
                    v => {
                        return Run::Crash(Crash::new(format!(
                            "filter: predicate returned a {}",
                            v.kind_name()
                        )))
                    }
                }
                let idx = idx + 1;
                if idx < xs.len() {
                    let x = xs[idx].clone();
                    let next = Cont::FilterNext {
                        p: p.clone(),
                        xs,
                        idx,
                        acc,
                        cont,
                    };
                    Run::Step(State::Apply(p, x, next))
                } else {
                    Run::Step(State::Ret(Value::list(acc.take()), (*cont).clone()))
                }
            }
            Cont::FoldlArg { f, xs, idx, cont } => {
                let x = xs[idx].clone();
                Run::Step(State::Apply(v, x, Cont::FoldlAcc { f, xs, idx, cont }))
            }
            Cont::FoldlAcc { f, xs, idx, cont } => {
                let idx = idx + 1;
                if idx < xs.len() {
                    let next = Cont::FoldlArg {
                        f: f.clone(),
                        xs,
                        idx,
                        cont,
                    };
                    Run::Step(State::Apply(f, v, next))
                } else {
                    Run::Step(State::Ret(v, (*cont).clone()))
                }
            }
            Cont::BuiltinResult {
                cname,
                check,
                def,
                cont,
            } => {
                if let Some(c) = &check {
                    if let Err(cr) = c.check_result(&self.shapes, &cname, &v) {
                        return Run::Crash(match &def {
                            Some(d) => in_def(cr, d),
                            None => cr,
                        });
                    }
                }
                Run::Step(State::Ret(v, (*cont).clone()))
            }
        }
    }

    // ------------------------------------------------------------------
    fn step_apply(&mut self, f: Value, arg: Value, k: Cont) -> Run {
        match &f {
            Value::Fun(fv) => match fv.as_ref() {
                FunVal::Builtin {
                    name,
                    arity,
                    args,
                    f: bf,
                    pending,
                } => {
                    let mut all = args.clone();
                    let (cname, cexpr) = match pending {
                        Some((n, c)) => (n.clone(), Some(c.clone())),
                        None => (name.clone(), None),
                    };
                    if let Some(c) = &cexpr {
                        // numbered by the signature, not by the arguments
                        // baked into a partial application (§4.13)
                        let at = c.position().unwrap_or(args.len());
                        if let Err(cr) = c.check_arg_at(&self.shapes, &cname, at, &arg) {
                            return Run::Crash(self.blame_builtin(cr, name, &cname));
                        }
                    }
                    all.push(arg);
                    let advanced = cexpr
                        .map(|c| Rc::new(crate::shape::ContractExpr::apply_first_rc(c)));
                    if all.len() == *arity {
                        // a composition's contract derived from its operands
                        // leaves results to theirs, checked as the
                        // composition runs; a signature attached to it is
                        // checked like any other
                        let check = advanced.filter(|c| {
                            name != "(.)"
                                || matches!(**c, crate::shape::ContractExpr::Known { .. })
                        });
                        // a signed definition's builtin or composition that
                        // consumes fewer arguments than its signature lists
                        // (`addT = (+) . inc` under `Int -> Int -> Text`)
                        // returns a function, which becomes the named partial
                        // application `addT 1` under the rest of the
                        // signature, as a lambda that runs out of parameters
                        // first does (§4.13, §5.2)
                        let k = match &check {
                            Some(c)
                                if !c.is_exhausted()
                                    && self.builtin_def(name, &cname).is_some() =>
                            {
                                // the arguments the definition was given, not
                                // those baked into its value
                                let own = c.position().unwrap_or(0).min(all.len());
                                Cont::NamePartial {
                                    name: cname.clone(),
                                    cexpr: c.clone(),
                                    args: all[all.len() - own..].to_vec(),
                                    cont: Rc::new(k),
                                }
                            }
                            _ => k,
                        };
                        if applies_functions(name) {
                            // the result is checked once the functions have
                            // run, and a crash while they run is blamed as
                            // one in the builtin itself
                            let def = self.builtin_def(name, &cname);
                            let k = if check.is_some() || def.is_some() {
                                Cont::BuiltinResult {
                                    cname: cname.clone(),
                                    check,
                                    def,
                                    cont: Rc::new(k),
                                }
                            } else {
                                k
                            };
                            return match self.step_applying(name, &all, k) {
                                Ok(run) => run,
                                Err(c) => Run::Crash(self.blame_builtin(c, name, &cname)),
                            };
                        }
                        match bf(self, &all) {
                            Ok(r) => {
                                if let Some(c) = &check {
                                    if let Err(cr) = c.check_result(&self.shapes, &cname, &r) {
                                        return Run::Crash(self.blame_builtin(cr, name, &cname));
                                    }
                                }
                                Run::Step(State::Ret(r, k))
                            }
                            Err(c) => Run::Crash(self.blame_builtin(c, name, &cname)),
                        }
                    } else {
                        Run::Step(State::Ret(
                            Value::Fun(Rc::new(FunVal::Builtin {
                                name: name.clone(),
                                arity: *arity,
                                args: all,
                                f: *bf,
                                pending: advanced.map(|c| (cname, c)),
                            })),
                            k,
                        ))
                    }
                }
                FunVal::Closure {
                    name,
                    params,
                    applied,
                    applied_args,
                    body,
                    env,
                    src,
                    pending,
                } => {
                    let (cname, cexpr) = match pending {
                        Some((n, c)) => (Some(n.clone()), Some(c.clone())),
                        None => (name.clone(), None),
                    };
                    // a function argument is checked like any other: where
                    // the signature wants a Repo, `new new` is a contract
                    // crash, not a composition (§4.13, §4.9)
                    if let Some(c) = &cexpr {
                        let fname = cname.clone().unwrap_or_default();
                        let at = c.position().unwrap_or(applied_args.len());
                        if let Err(cr) = c.check_arg_at(&self.shapes, &fname, at, &arg) {
                            // a violation is in the definition checked
                            // (§1.4, §4.13's `describe 3`)
                            return Run::Crash(in_def(cr, &fname));
                        }
                    }
                    let p = &params[*applied];
                    let env2 = match p {
                        Pattern::Var(n) => env.extend(vec![(n.clone(), arg.clone())]),
                        Pattern::Wildcard => env.clone(),
                    };
                    let new_applied = applied + 1;
                    let advanced = cexpr
                        .map(|c| Rc::new(crate::shape::ContractExpr::apply_first_rc(c)));
                    let contract_exhausted = match &advanced {
                        Some(c) => c.is_exhausted(),
                        None => true,
                    };
                    if new_applied == params.len() {
                        // the definition whose lambda this body is, if any,
                        // executes while it runs (§1.4)
                        let def = self
                            .def_bodies
                            .get(&Rc::as_ptr(body))
                            .map(|(d, _)| d.clone());
                        // a call in tail position of a definition's body
                        // leaves that definition nothing to do but pass this
                        // body's value on, so its frame gives way to the one
                        // this body runs under, and unsigned tail recursion
                        // keeps its continuation constant in size
                        let signed = cname.is_some() && advanced.is_some();
                        let k = match k {
                            Cont::InDef { cont, .. } if signed || def.is_some() => {
                                (*cont).clone()
                            }
                            k => k,
                        };
                        // the body is evaluated now, as at any application
                        // (§4.1): a signature adds checks and never changes
                        // when a body runs, or whether an `or` around the
                        // application catches its crash (§4.6, §4.13)
                        let k = match (cname, advanced) {
                            (Some(n), Some(c)) => {
                                // a lambda that runs out of parameters before
                                // its signature (`describe = \m -> mapRoot …`)
                                // returns a function, which becomes the named
                                // partial application `describe "wip"` (§5.2)
                                // under the rest of the signature
                                let k = if c.is_exhausted() {
                                    k
                                } else {
                                    let mut args = applied_args.clone();
                                    args.push(arg);
                                    Cont::NamePartial {
                                        name: n.clone(),
                                        cexpr: c.clone(),
                                        args,
                                        cont: Rc::new(k),
                                    }
                                };
                                Cont::CheckExpr {
                                    name: n,
                                    cexpr: c,
                                    cont: Rc::new(k),
                                }
                            }
                            _ => k,
                        };
                        // inside the check of the definition it was bound to,
                        // which names this one already when it is the same
                        let k = match def {
                            Some(d)
                                if !matches!(&k, Cont::CheckExpr { name, .. }
                                    if **name == *d) =>
                            {
                                Cont::InDef {
                                    name: d,
                                    cont: Rc::new(k),
                                }
                            }
                            _ => k,
                        };
                        Run::Step(State::Eval(body.clone(), env2, k))
                    } else if contract_exhausted {
                        // lambda params remain but the contract is satisfied;
                        // keep currying (unsigned tail)
                        let mut new_args = applied_args.clone();
                        new_args.push(arg.clone());
                        let v = Value::Fun(Rc::new(FunVal::Closure {
                            name: name.clone(),
                            params: params.clone(),
                            applied: new_applied,
                            applied_args: new_args,
                            body: body.clone(),
                            env: env2,
                            src: src.clone(),
                            pending: match (&cname, &advanced) {
                                (Some(n), Some(c)) => Some((n.clone(), c.clone())),
                                _ => None,
                            },
                        }));
                        Run::Step(State::Ret(v, k))
                    } else {
                        let mut new_args = applied_args.clone();
                        new_args.push(match &arg { v => v.clone() });
                        let v = Value::Fun(Rc::new(FunVal::Closure {
                            name: name.clone(),
                            params: params.clone(),
                            applied: new_applied,
                            applied_args: new_args,
                            body: body.clone(),
                            env: env2,
                            src: src.clone(),
                            pending: match (&cname, &advanced) {
                                (Some(n), Some(c)) => Some((n.clone(), c.clone())),
                                _ => None,
                            },
                        }));
                        Run::Step(State::Ret(v, k))
                    }
                }
                FunVal::ComposeLazy(f, g) => {
                    // (f . g) x = f (g x) (§4.9), whatever x is: apply g,
                    // then f to the result; a crash in either keeps its
                    // message and unwinds to any enclosing `or` (§4.7)
                    Run::Step(State::Apply(g.clone(), arg, Cont::Fun(f.clone(), Rc::new(k))))
                }
                FunVal::OrFun(a, b) => {
                    // (f or g) x = f x or g x
                    let a = a.clone();
                    let b = b.clone();
                    let k = Rc::new(k);
                    Run::Step(State::Apply(
                        a,
                        arg.clone(),
                        Cont::Try {
                            on_ok: Some(TryOk::OrFunLhs {
                                rhs: b.clone(),
                                arg: arg.clone(),
                                cont: k.clone(),
                            }),
                            snapshot: *self.fresh.borrow(),
                            on_crash: Box::new(State::Apply(b, arg, Cont::Shared(k))),
                        },
                    ))
                }
                FunVal::Labelled(name, labelled) => {
                    // %name x = labelled "name" x
                    let text = Value::text(name.clone());
                    Run::Step(State::Apply(
                        labelled.clone(),
                        text,
                        Cont::ApplyTo(arg, Rc::new(k)),
                    ))
                }
            },
            _ => Run::Crash(Crash::new(format!(
                "cannot apply a {} as a function",
                f.kind_name()
            ))),
        }
    }
}

