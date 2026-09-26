//! config.j loading and validation (§6).

use crate::ast::{Expr, Item, TypeExpr};
use crate::eval::Interp;
use crate::parse::{parse_config, ParseError};
use crate::shape::{compile_contract, Shapes};
use crate::value::{Crash, Env, Value};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

#[derive(Debug)]
pub enum ConfigError {
    Parse(ParseError),
    Validation(String),
}

impl ConfigError {
    pub fn message(&self) -> String {
        match self {
            ConfigError::Parse(p) => format!("line {}: {}", p.line, p.msg),
            ConfigError::Validation(m) => m.clone(),
        }
    }
}

pub struct Config {
    pub shapes: Shapes,
    /// definition names in dependency order for evaluation
    pub defs: Vec<(String, Rc<Expr>)>,
    /// signatures for builtins and definitions
    pub sigs: HashMap<String, TypeExpr>,
    /// which sig names are builtins (no definition)
    pub builtin_names: HashSet<String>,
    /// all global names (for no-shadowing in CLI parsing)
    pub global_names: BTreeSet<String>,
}

const RESERVED_BUILTINS: &[&str] = &[
    ".", "id", "const", "crash", "==", "/=", "&&", "||", "not", "+", "-", "*", "<", "<=", ">",
    ">=", "show", "::", "map", "filter", "length", "null", "head", "tail", "last", "nth", "take",
    "drop", "member", "range", "foldl", "concat", "++", "startsWith", "endsWith", "splitOn",
    "replay", "unresolved", "blob", "text", "by", "meta", "validate", "diff", "difft", "treeWith",
    "extract", "touchedPaths",
];

pub fn is_reserved_builtin(name: &str) -> bool {
    RESERVED_BUILTINS.contains(&name)
}

pub fn reserved_set() -> BTreeSet<String> {
    RESERVED_BUILTINS.iter().map(|s| s.to_string()).collect()
}

/// Parse and validate config.j (§6.2). Id literals are resolved separately.
pub fn load_config(src: &str) -> Result<Config, ConfigError> {
    let outer = Rc::new(reserved_set());
    let items = parse_config(src, outer).map_err(ConfigError::Parse)?;
    validate_items(items)
}

fn validate_items(items: Vec<Item>) -> Result<Config, ConfigError> {
    let verr = |m: String| Err(ConfigError::Validation(m));
    let mut shapes = Shapes::new();
    let mut sigs: HashMap<String, (TypeExpr, usize)> = HashMap::new();
    let _defs: Vec<(String, Rc<Expr>, usize)> = Vec::new();
    let mut def_names: HashSet<String> = HashSet::new();

    for item in &items {
        match item {
            Item::TypeDecl(name, ty) => {
                if shapes.decls.contains_key(name) {
                    return verr(format!("type `{}` is declared twice", name));
                }
                shapes.decls.insert(name.clone(), ty.clone());
            }
            Item::Signature(name, ty, line) => {
                if sigs.contains_key(name) {
                    return verr(format!("`{}` has two signatures", name));
                }
                sigs.insert(name.clone(), (ty.clone(), *line));
            }
            Item::Definition(name, _, line) => {
                if is_reserved_builtin(name) {
                    return verr(format!(
                        "`{}` is a builtin and may not be defined",
                        name
                    ));
                }
                if !def_names.insert(name.clone()) {
                    return verr(format!("`{}` is defined twice", name));
                }
                let _ = line;
            }
        }
    }

    // pair signatures with definitions; a signature must immediately precede
    // its definition (blank lines/comments fine — items between are not, a
    // typedecl or another signature included). A builtin's signature has no
    // definition, so any item, or the end of the file, may follow it.
    let mut prev_sig: Option<&String> = None;
    for item in items.iter().map(Some).chain([None]) {
        if let Some(s) = prev_sig {
            let paired = matches!(item, Some(Item::Definition(name, _, _)) if name == s);
            if !paired && def_names.contains(s) {
                return verr(format!(
                    "the signature for `{}` is not immediately followed by its definition",
                    s
                ));
            }
        }
        prev_sig = match item {
            Some(Item::Signature(name, _, _)) => Some(name),
            _ => None,
        };
    }

    let mut builtin_names: HashSet<String> = HashSet::new();
    let mut final_sigs: HashMap<String, TypeExpr> = HashMap::new();
    for (name, (ty, _)) in &sigs {
        if def_names.contains(name) {
            final_sigs.insert(name.clone(), ty.clone());
        } else {
            if !is_reserved_builtin(name) {
                return verr(format!(
                    "`{}` is declared but is not a builtin and has no definition",
                    name
                ));
            }
            builtin_names.insert(name.clone());
            final_sigs.insert(name.clone(), ty.clone());
        }
    }

    // required definitions (§6.2.6) — checked after evaluation for shapes;
    // here only presence
    for req in ["user", "immutable", "tree", "labelled"] {
        if !def_names.contains(req) {
            return verr(format!("config.j must define `{}`", req));
        }
    }

    // dependency order (§4.1)
    let def_map: HashMap<String, Rc<Expr>> = items
        .iter()
        .filter_map(|it| match it {
            Item::Definition(n, e, _) => Some((n.clone(), e.clone())),
            _ => None,
        })
        .collect();
    let ordered = dependency_order(&def_map)?;

    let mut global_names = reserved_set();
    for n in def_map.keys() {
        global_names.insert(n.clone());
    }

    Ok(Config {
        shapes,
        defs: ordered,
        sigs: final_sigs,
        builtin_names,
        global_names,
    })
}

/// topological order by references outside lambdas (§4.1)
fn dependency_order(
    defs: &HashMap<String, Rc<Expr>>,
) -> Result<Vec<(String, Rc<Expr>)>, ConfigError> {
    let mut deps: HashMap<String, BTreeSet<String>> = HashMap::new();
    for (name, expr) in defs {
        let mut free = BTreeSet::new();
        load_deps(expr, &mut free);
        let d: BTreeSet<String> = free.into_iter().filter(|n| defs.contains_key(n)).collect();
        deps.insert(name.clone(), d);
    }
    let mut ordered = Vec::new();
    let mut done: HashSet<String> = HashSet::new();
    let mut names: Vec<String> = defs.keys().cloned().collect();
    names.sort();
    loop {
        let mut progress = false;
        for n in &names {
            if done.contains(n) {
                continue;
            }
            if deps[n].iter().all(|d| done.contains(d)) {
                done.insert(n.clone());
                ordered.push((n.clone(), defs[n].clone()));
                progress = true;
            }
        }
        if done.len() == names.len() {
            return Ok(ordered);
        }
        if !progress {
            return Err(ConfigError::Validation(
                "config.j: a cycle among top-level definitions".into(),
            ));
        }
    }
}

/// Names a definition's value depends on at load time (§4.1). Lambda bodies
/// never form load-time dependencies; inside `let` bindings, references to
/// the bound names are resolved at application time and also do not count.
fn load_deps(e: &Expr, out: &mut BTreeSet<String>) {
    match e {
        Expr::Var(n) => {
            out.insert(n.clone());
        }
        Expr::Lambda(_, _, _) => {}
        Expr::If(a, b, c) => {
            load_deps(a, out);
            load_deps(b, out);
            load_deps(c, out);
        }
        Expr::Let(bs, body) => {
            // a let is a lambda applied to a tuple at the value level; its
            // bindings evaluate now only insofar as the body of each binding
            // is evaluated now — but each binding is a value, and non-lambda
            // values may reference earlier bindings. References to names
            // bound in this let do not create load deps on same-named
            // top-level definitions.
            let local: BTreeSet<String> = bs.iter().map(|(n, _)| n.clone()).collect();
            let mut sub = BTreeSet::new();
            for (_, e) in bs {
                load_deps(e, &mut sub);
            }
            load_deps(body, &mut sub);
            for n in sub {
                if !local.contains(&n) {
                    out.insert(n);
                }
            }
        }
        Expr::App(f, x) => {
            load_deps(f, out);
            load_deps(x, out);
        }
        Expr::BinOp(_, a, b) | Expr::Or(a, b) => {
            load_deps(a, out);
            load_deps(b, out);
        }
        Expr::Select(e2, _) => load_deps(e2, out),
        Expr::Paren(e2) => load_deps(e2, out),
        Expr::Update(e2, fs) => {
            load_deps(e2, out);
            for (_, v) in fs {
                load_deps(v, out);
            }
        }
        Expr::Record(fs) => {
            for (_, v) in fs {
                load_deps(v, out);
            }
        }
        Expr::List(es) => {
            for e in es {
                load_deps(e, out);
            }
        }
        _ => {}
    }
}

/// Evaluate a loaded config into an interpreter's globals.
pub fn eval_config(interp: &mut Interp, cfg: &Config) -> Result<(), Crash> {
    // contracts
    for (name, ty) in &cfg.sigs {
        interp
            .contracts
            .insert(name.clone(), Rc::new(compile_contract(&interp.shapes, ty)));
    }
    // install builtins that are declared, with their contracts attached
    let mut globals: HashMap<String, Value> = HashMap::new();
    for (name, v) in crate::builtins::all_builtins() {
        if cfg.builtin_names.contains(&name) {
            let v = match cfg.sigs.get(&name) {
                Some(ty) => crate::value::attach_pending(
                    &v,
                    &name,
                    Rc::new(compile_contract(&interp.shapes, ty)),
                ),
                None => v,
            };
            globals.insert(name, v);
        }
    }
    interp.globals = Env::with_globals(globals);
    // evaluate definitions in dependency order, through one recursive frame so
    // top-level definitions are mutually recursive (§4.1)
    let (genv, cell) = interp.globals.extend_rec();
    for (name, expr) in &cfg.defs {
        *interp.current_def.borrow_mut() = Some(name.clone());
        let v = interp.eval(expr, &genv)?;
        // value-level contract for non-function definitions
        if let Some(ty) = cfg.sigs.get(name) {
            let c = compile_contract(&interp.shapes, ty);
            if c.params.is_empty() {
                if let Err(msg) = crate::shape::check(&interp.shapes, &c.result, &v) {
                    return Err(Crash::new(format!("contract: {}: {}", name, msg)));
                }
            }
        }
        cell.borrow_mut().push((name.clone(), v.clone()));
        // attach the definition's name and contract to its function value so
        // applications are checked (§4.13) and errors name the definition
        if let Some(ty) = cfg.sigs.get(name) {
            let contract = Rc::new(compile_contract(&interp.shapes, ty));
            let named = crate::value::attach_pending(&v, name, contract);
            let last = cell.borrow_mut().len() - 1;
            cell.borrow_mut()[last] = (name.clone(), named);
        }
    }
    interp.globals = genv;
    *interp.current_def.borrow_mut() = None;
    // §6.2.6 shape checks on the four read-by-interpreter definitions
    check_required(interp)?;
    Ok(())
}

fn check_required(interp: &Interp) -> Result<(), Crash> {
    let user = interp
        .globals
        .lookup("user")
        .ok_or_else(|| Crash::new("config.j must define `user`"))?;
    match &user {
        Value::Record(m) => {
            let ok = m.get("name").map(|v| matches!(v, Value::Text(t) if !t.is_empty())).unwrap_or(false)
                && m.get("email").map(|v| matches!(v, Value::Text(t) if !t.is_empty())).unwrap_or(false);
            if !ok {
                return Err(Crash::new(
                    "`user` must be a record with non-empty `name` and `email` texts",
                ));
            }
        }
        _ => {
            return Err(Crash::new(
                "`user` must be a record with non-empty `name` and `email` texts",
            ))
        }
    }
    for f in ["immutable", "tree", "labelled"] {
        match interp.globals.lookup(f) {
            Some(Value::Fun(_)) => {}
            _ => return Err(Crash::new(format!("`{}` must be a function", f))),
        }
    }
    Ok(())
}

/// Evaluate only `user`, for init/clone when there is no repository (§6.1).
pub fn eval_user_only(interp: &mut Interp, cfg: &Config) -> Result<(String, String), Crash> {
    for (name, v) in crate::builtins::all_builtins() {
        if cfg.builtin_names.contains(&name) {
            interp.globals = interp.globals.extend(vec![(name, v)]);
        }
    }
    for (name, ty) in &cfg.sigs {
        interp
            .contracts
            .insert(name.clone(), Rc::new(compile_contract(&interp.shapes, ty)));
    }
    for (name, expr) in &cfg.defs {
        if name == "user" {
            *interp.current_def.borrow_mut() = Some(name.clone());
            let genv = interp.globals.clone();
            let v = interp.eval(expr, &genv)?;
            *interp.current_def.borrow_mut() = None;
            interp.globals = interp.globals.extend(vec![(name.clone(), v.clone())]);
            if let Value::Record(m) = &v {
                let n = m.get("name").and_then(|v| v.as_text().ok()).unwrap_or("").to_string();
                let e = m.get("email").and_then(|v| v.as_text().ok()).unwrap_or("").to_string();
                if n.is_empty() || e.is_empty() {
                    return Err(Crash::new(
                        "`user` must be a record with non-empty `name` and `email` texts",
                    ));
                }
                return Ok((n, e));
            }
            return Err(Crash::new(
                "`user` must be a record with non-empty `name` and `email` texts",
            ));
        }
    }
    Err(Crash::new("config.j must define `user`"))
}

/// Resolve all `@prefix` literals in an AST against visible ids.
pub fn resolve_ids(
    e: &Expr,
    interp: &Interp,
) -> Result<Expr, Vec<(String, Vec<String>)>> {
    let mut failures = Vec::new();
    let out = resolve_ids_rec(e, interp, &mut failures);
    if failures.is_empty() {
        Ok(out)
    } else {
        Err(failures)
    }
}

fn resolve_ids_rec(
    e: &Expr,
    interp: &Interp,
    failures: &mut Vec<(String, Vec<String>)>,
) -> Expr {
    let mut r = |e: &Rc<Expr>| Rc::new(resolve_ids_rec(e, interp, failures));
    match e {
        Expr::IdLit(prefix) => {
            let matches = interp.backend.resolve_prefix(prefix);
            if matches.len() == 1 {
                Expr::Id(matches[0].clone())
            } else {
                failures.push((prefix.clone(), matches));
                Expr::Id(prefix.clone())
            }
        }
        Expr::Var(_)
        | Expr::TypeName(_)
        | Expr::Int(_)
        | Expr::Text(_)
        | Expr::Id(_)
        | Expr::NewId
        | Expr::LabelLit(_)
        | Expr::PathLit(_)
        | Expr::Bool(_)
        | Expr::SelectorFun(_)
        | Expr::Crash => e.clone(),
        Expr::Paren(e2) => Expr::Paren(r(e2)),
        Expr::Lambda(ps, body, src) => Expr::Lambda(ps.clone(), r(body), src.clone()),
        Expr::If(a, b, c) => Expr::If(r(a), r(b), r(c)),
        Expr::Let(bs, body) => Expr::Let(
            bs.iter().map(|(n, e)| (n.clone(), r(e))).collect(),
            r(body),
        ),
        Expr::App(f, x) => Expr::App(r(f), r(x)),
        Expr::BinOp(o, a, b) => Expr::BinOp(o, r(a), r(b)),
        Expr::Or(a, b) => Expr::Or(r(a), r(b)),
        Expr::Select(e2, f) => Expr::Select(r(e2), f.clone()),
        Expr::Update(e2, fs) => Expr::Update(
            r(e2),
            fs.iter().map(|(n, v)| (n.clone(), r(v))).collect(),
        ),
        Expr::Record(fs) => Expr::Record(fs.iter().map(|(n, v)| (n.clone(), r(v))).collect()),
        Expr::List(es) => Expr::List(es.iter().map(r).collect()),
    }
}

/// Resolve every `@prefix` literal in the config's definitions against the
/// visible ids, in place (§1.2.5: "in `config.j`, every literal is resolved at
/// load"). Returns the ones that did not resolve to exactly one commit, as
/// (definition, prefix, candidates); those definitions are left untouched.
///
/// Checking without rewriting is not enough: an unresolved literal reaching
/// the evaluator crashes with `internal: unresolved id literal`, and because a
/// lambda body is not evaluated at load, it does so only when the definition
/// is eventually used.
pub fn resolve_config_ids(
    cfg: &mut Config,
    interp: &Interp,
) -> Vec<(String, String, Vec<String>)> {
    let mut out = Vec::new();
    for (name, expr) in cfg.defs.iter_mut() {
        let mut failures = Vec::new();
        let resolved = resolve_ids_rec(expr, interp, &mut failures);
        if failures.is_empty() {
            *expr = Rc::new(resolved);
        } else {
            for (prefix, matches) in failures {
                out.push((name.clone(), prefix, matches));
            }
        }
    }
    out
}
