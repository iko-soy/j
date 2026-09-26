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

/// Evaluation order of the definitions (§4.1).
///
/// References outside lambdas decide it, and only they can form a cycle;
/// ready definitions are taken in name order. That alone can apply a
/// function at load before a definition its body refers to (`opts = mk 1`
/// with `mk = \d -> { lanes = lanes }` and `lanes` still waiting), an unbound
/// name with no cycle anywhere. So the definitions are then grouped into the
/// strongly connected components of all references, lambda bodies included,
/// and the groups evaluated referenced first, the members of each in the
/// first order. Every definition still follows its references outside
/// lambdas, and since evaluating one looks up only names it reaches by
/// references, whatever the first order had bound in time still is.
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
    while done.len() < names.len() {
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
        if !progress {
            return Err(ConfigError::Validation(
                "config.j: a cycle among top-level definitions".into(),
            ));
        }
    }
    let position: HashMap<&str, usize> =
        ordered.iter().enumerate().map(|(i, (n, _))| (n.as_str(), i)).collect();
    let edges: Vec<Vec<usize>> = ordered
        .iter()
        .map(|(n, e)| {
            let mut free = deps[n].clone();
            e.free_vars(&mut Vec::new(), &mut free);
            free.iter().filter_map(|n| position.get(n.as_str()).copied()).collect()
        })
        .collect();
    let mut grouped = Vec::with_capacity(ordered.len());
    for mut group in components(&edges) {
        group.sort_unstable();
        grouped.extend(group.into_iter().map(|i| ordered[i].clone()));
    }
    Ok(grouped)
}

/// The strongly connected components of a graph given as adjacency lists,
/// each listed after every component it has an edge into (Tarjan's
/// algorithm, on an explicit stack so a long chain of definitions cannot
/// overflow the native one).
fn components(edges: &[Vec<usize>]) -> Vec<Vec<usize>> {
    const UNSEEN: usize = usize::MAX;
    let n = edges.len();
    let (mut index, mut low, mut on_stack) = (vec![UNSEEN; n], vec![0; n], vec![false; n]);
    let (mut stack, mut out, mut next) = (Vec::new(), Vec::new(), 0);
    for root in 0..n {
        if index[root] != UNSEEN {
            continue;
        }
        // (vertex, how many of its edges have been followed)
        let mut work = vec![(root, 0)];
        while let Some(&(v, i)) = work.last() {
            if i == 0 {
                index[v] = next;
                low[v] = next;
                next += 1;
                stack.push(v);
                on_stack[v] = true;
            }
            if let Some(&w) = edges[v].get(i) {
                work.last_mut().unwrap().1 = i + 1;
                if index[w] == UNSEEN {
                    work.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
                continue;
            }
            work.pop();
            if let Some(&(u, _)) = work.last() {
                low[u] = low[u].min(low[v]);
            }
            if low[v] == index[v] {
                let mut group = Vec::new();
                loop {
                    let w = stack.pop().unwrap();
                    on_stack[w] = false;
                    group.push(w);
                    if w == v {
                        break;
                    }
                }
                out.push(group);
            }
        }
    }
    out
}

/// Evaluation order of the bindings of a `let` block (§4.1), as indices into
/// them, or `None` when references outside lambdas form a cycle.
///
/// The rule is `dependency_order`'s, with ready bindings taken in source
/// order: a binding follows every binding it refers to outside a lambda, and
/// the bindings are then grouped by all their references, so a function
/// applied in a binding finds the bindings its body refers to. That order is
/// the source order when every reference is to an earlier binding, or from
/// inside a lambda to its own, as in nearly every block, which is so taken
/// without the sorting: this runs each time a block is evaluated.
pub(crate) fn let_order(bs: &[(String, Rc<Expr>)]) -> Option<Vec<usize>> {
    let refs: Vec<Vec<(usize, bool)>> = bs
        .iter()
        .map(|(_, e)| {
            let mut out = Vec::new();
            binding_refs(e, bs, false, &mut out);
            out
        })
        .collect();
    let in_order =
        |i: usize, r: &[(usize, bool)]| r.iter().all(|&(j, outside)| j < i || (j == i && !outside));
    if refs.iter().enumerate().all(|(i, r)| in_order(i, r)) {
        return Some((0..bs.len()).collect());
    }
    let mut ordered = Vec::with_capacity(bs.len());
    let mut done = vec![false; bs.len()];
    while ordered.len() < bs.len() {
        let mut progress = false;
        for i in 0..bs.len() {
            if !done[i] && refs[i].iter().all(|&(j, outside)| !outside || done[j]) {
                done[i] = true;
                ordered.push(i);
                progress = true;
            }
        }
        if !progress {
            return None;
        }
    }
    let mut position = vec![0; bs.len()];
    for (p, &i) in ordered.iter().enumerate() {
        position[i] = p;
    }
    let edges: Vec<Vec<usize>> = ordered
        .iter()
        .map(|&i| refs[i].iter().map(|&(j, _)| position[j]).collect())
        .collect();
    let mut grouped = Vec::with_capacity(bs.len());
    for mut group in components(&edges) {
        group.sort_unstable();
        grouped.extend(group.into_iter().map(|p| ordered[p]));
    }
    Some(grouped)
}

/// Record the references in `e` to the names a `let` block binds, as indices
/// into its bindings `bs`, each with whether it lies outside every lambda,
/// where evaluating `e` needs that binding's value (§4.1). No binder inside
/// the block may reuse one of its names (§4.2), so every occurrence is one.
fn binding_refs(
    e: &Expr,
    bs: &[(String, Rc<Expr>)],
    in_lambda: bool,
    out: &mut Vec<(usize, bool)>,
) {
    let mut sub = |e: &Expr| binding_refs(e, bs, in_lambda, out);
    match e {
        Expr::Var(n) => {
            if let Some(j) = bs.iter().position(|(b, _)| b == n) {
                out.push((j, !in_lambda));
            }
        }
        Expr::Lambda(_, body, _) => binding_refs(body, bs, true, out),
        Expr::If(a, b, c) => {
            sub(a);
            sub(b);
            sub(c);
        }
        Expr::Let(inner, body) => {
            for (_, e) in inner {
                sub(e);
            }
            sub(body);
        }
        Expr::App(a, b) | Expr::BinOp(_, a, b) | Expr::Or(a, b) => {
            sub(a);
            sub(b);
        }
        Expr::Select(a, _) | Expr::Paren(a) => sub(a),
        Expr::Update(a, fs) => {
            sub(a);
            for (_, v) in fs {
                sub(v);
            }
        }
        Expr::Record(fs) => {
            for (_, v) in fs {
                sub(v);
            }
        }
        Expr::List(es) => {
            for e in es {
                sub(e);
            }
        }
        // `%main` refers to `labelled`, a top-level name, never a binding
        Expr::TypeName(_)
        | Expr::Int(_)
        | Expr::Text(_)
        | Expr::IdLit(_)
        | Expr::Id(_)
        | Expr::NewId
        | Expr::LabelLit(_)
        | Expr::PathLit(_)
        | Expr::Bool(_)
        | Expr::SelectorFun(_)
        | Expr::Crash => {}
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
        // `%main` is `labelled "main"` (§4.11), evaluated where it stands
        Expr::LabelLit(_) => {
            out.insert("labelled".to_string());
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
        // the value itself, one level deep, against the whole signature
        // (§4.13): under a function type, including an alias of one, it must
        // be a function, however it was built (`conflicts : Revset` is a
        // partial application, `myEdit : Edit` with `myEdit = 5` is refused)
        if let Some(ty) = cfg.sigs.get(name) {
            if let Err(msg) = crate::shape::check(&interp.shapes, ty, &v) {
                return Err(Crash::new(format!("contract: {}: {}", name, msg)));
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
