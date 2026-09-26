//! Builtins (§4.9).

use crate::eval::Interp;
use crate::value::{value_eq, BlobContent, BlobKind, BlobVal, Crash, FunVal, Value};
use num_bigint::BigInt;
use std::collections::BTreeSet;
use std::rc::Rc;

pub type BResult = Result<Value, Crash>;

fn builtin(name: &str, arity: usize, f: crate::value::BuiltinFn) -> (String, Value) {
    (
        name.to_string(),
        Value::Fun(Rc::new(FunVal::Builtin {
            name: name.to_string(),
            arity,
            args: Vec::new(),
            f,
            pending: None,
        })),
    )
}

pub fn make_selector(field: &str) -> Value {
    let name = format!(".{}", field);
    // A selector is a closure-free builtin of arity 1; the field is baked
    // into the function via a wrapper table.
    Value::Fun(Rc::new(FunVal::Builtin {
        name,
        arity: 2, // baked-in field name plus the record
        args: vec![Value::text(field)],
        f: selector_apply,
        pending: None,
    }))
}

fn selector_apply(_i: &mut Interp, args: &[Value]) -> BResult {
    let field = args[0].as_text()?.to_string();
    args[1].field(&field)
}

macro_rules! want {
    ($args:expr, $i:expr, $pat:pat, $what:expr) => {
        match &$args[$i] {
            $pat => (),
            v => {
                return Err(Crash::new(format!(
                    "expected {}, got a {}",
                    $what,
                    v.kind_name()
                )))
            }
        }
    };
}

pub fn all_builtins() -> Vec<(String, Value)> {
    vec![
        builtin(".", 2, b_compose),
        builtin("id", 1, b_id),
        builtin("const", 2, b_const),
        builtin("crash", 1, b_crash),
        builtin("==", 2, b_eq),
        builtin("/=", 2, b_ne),
        builtin("&&", 2, b_and),
        builtin("||", 2, b_or),
        builtin("not", 1, b_not),
        builtin("+", 2, b_add),
        builtin("-", 2, b_sub),
        builtin("*", 2, b_mul),
        builtin("<", 2, b_lt),
        builtin("<=", 2, b_le),
        builtin(">", 2, b_gt),
        builtin(">=", 2, b_ge),
        builtin("show", 1, b_show),
        builtin("::", 2, b_cons),
        builtin("map", 2, b_map),
        builtin("filter", 2, b_filter),
        builtin("length", 1, b_length),
        builtin("null", 1, b_null),
        builtin("head", 1, b_head),
        builtin("tail", 1, b_tail),
        builtin("last", 1, b_last),
        builtin("nth", 2, b_nth),
        builtin("take", 2, b_take),
        builtin("drop", 2, b_drop),
        builtin("member", 2, b_member),
        builtin("range", 2, b_range),
        builtin("foldl", 3, b_foldl),
        builtin("concat", 1, b_concat),
        builtin("++", 2, b_append),
        builtin("startsWith", 2, b_starts_with),
        builtin("endsWith", 2, b_ends_with),
        builtin("splitOn", 2, b_split_on),
        builtin("replay", 2, b_replay),
        builtin("unresolved", 1, b_unresolved),
        builtin("touchedPaths", 1, b_touched_paths),
        builtin("blob", 1, b_blob),
        builtin("text", 1, b_text),
        builtin("by", 2, b_by),
        builtin("meta", 1, b_meta),
        builtin("validate", 1, b_validate),
        builtin("diff", 2, b_diff),
        builtin("difft", 3, b_difft),
        builtin("treeWith", 2, b_tree_with),
        builtin("extract", 2, b_extract),
    ]
}

fn b_compose(_i: &mut Interp, args: &[Value]) -> BResult {
    want!(args, 0, Value::Fun(_), "a function");
    want!(args, 1, Value::Fun(_), "a function");
    // f . g — build a closure-like builtin pair via a nested builtin
    let f = args[0].clone();
    let g = args[1].clone();
    compose_values(_i, f, g)
}

fn compose_apply(i: &mut Interp, args: &[Value]) -> BResult {
    let f = args[0].clone();
    let g = args[1].clone();
    let x = args[2].clone();
    // (f . g) x = f (g x) (§4.9), whatever x is: a function argument is
    // applied like any other, and g's contract checks it (§4.13). `squash =
    // abandon . contract everything` waits for the repository because it is
    // this node, not because anything composes on a function argument.
    let gx = i.apply(g, x)?;
    i.apply(f, gx)
}

/// f . g as a value, with contract propagation
pub fn compose_values(i: &mut Interp, f: Value, g: Value) -> BResult {
    let pending = match (pending_of(&f), pending_of(&g)) {
        (Some((nf, cf)), Some((ng, cg))) => Some((
            format!("({} . {})", nf, ng),
            Rc::new(crate::shape::ContractExpr::Compose(
                Box::new((*cf).clone()),
                Box::new((*cg).clone()),
            )),
        )),
        _ => None,
    };
    let _ = &pending;
    let _ = i;
    Ok(Value::Fun(Rc::new(FunVal::Builtin {
        name: "(.)".into(),
        arity: 3,
        args: vec![f, g],
        f: compose_apply,
        pending,
    })))
}

pub fn pending_of(v: &Value) -> Option<(String, Rc<crate::shape::ContractExpr>)> {
    if let Value::Fun(fv) = v {
        match fv.as_ref() {
            crate::value::FunVal::Closure {
                name: Some(n),
                pending: Some(p),
                ..
            } => Some((n.clone(), p.1.clone())),
            crate::value::FunVal::Closure {
                name: Some(n),
                pending: None,
                ..
            } => Some((n.clone(), Rc::new(crate::shape::ContractExpr::Unknown))),
            crate::value::FunVal::Builtin {
                pending: Some(p), ..
            } => Some(p.clone()),
            crate::value::FunVal::Builtin {
                name, pending: None, ..
            } => Some((name.clone(), Rc::new(crate::shape::ContractExpr::Unknown))),
            _ => None,
        }
    } else {
        None
    }
}

fn b_id(_i: &mut Interp, args: &[Value]) -> BResult {
    Ok(args[0].clone())
}

fn b_const(_i: &mut Interp, args: &[Value]) -> BResult {
    Ok(args[0].clone())
}

fn b_crash(_i: &mut Interp, args: &[Value]) -> BResult {
    let msg = args[0].as_text()?.to_string();
    Err(Crash::new(msg))
}

fn b_eq(_i: &mut Interp, args: &[Value]) -> BResult {
    Ok(Value::Bool(value_eq(&args[0], &args[1])?))
}

fn b_ne(_i: &mut Interp, args: &[Value]) -> BResult {
    Ok(Value::Bool(!value_eq(&args[0], &args[1])?))
}

// `&&` and `||` short-circuit when written infix (the evaluator handles
// `Expr::BinOp` directly). These are the same operators used as *values* —
// `foldl (&&) true xs`, `map ((||) b) xs` — where both arguments are already
// evaluated, so they must still combine them rather than return the second.
fn b_and(_i: &mut Interp, args: &[Value]) -> BResult {
    want!(args, 0, Value::Bool(_), "Bool");
    want!(args, 1, Value::Bool(_), "Bool");
    match (&args[0], &args[1]) {
        (Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(*a && *b)),
        _ => unreachable!("checked above"),
    }
}

fn b_or(_i: &mut Interp, args: &[Value]) -> BResult {
    want!(args, 0, Value::Bool(_), "Bool");
    want!(args, 1, Value::Bool(_), "Bool");
    match (&args[0], &args[1]) {
        (Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(*a || *b)),
        _ => unreachable!("checked above"),
    }
}

fn b_not(_i: &mut Interp, args: &[Value]) -> BResult {
    match &args[0] {
        Value::Bool(b) => Ok(Value::Bool(!b)),
        v => Err(Crash::new(format!("not: expected Bool, got a {}", v.kind_name()))),
    }
}

fn int2(args: &[Value]) -> Result<(&BigInt, &BigInt), Crash> {
    match (&args[0], &args[1]) {
        (Value::Int(a), Value::Int(b)) => Ok((a, b)),
        (Value::Int(_), v) | (v, Value::Int(_)) => Err(Crash::new(format!(
            "expected Int, got a {}",
            v.kind_name()
        ))),
        (v, _) => Err(Crash::new(format!("expected Int, got a {}", v.kind_name()))),
    }
}

fn b_add(_i: &mut Interp, args: &[Value]) -> BResult {
    let (a, b) = int2(args)?;
    Ok(Value::Int(a + b))
}
fn b_sub(_i: &mut Interp, args: &[Value]) -> BResult {
    let (a, b) = int2(args)?;
    Ok(Value::Int(a - b))
}
fn b_mul(_i: &mut Interp, args: &[Value]) -> BResult {
    let (a, b) = int2(args)?;
    Ok(Value::Int(a * b))
}
fn b_lt(_i: &mut Interp, args: &[Value]) -> BResult {
    let (a, b) = int2(args)?;
    Ok(Value::Bool(a < b))
}
fn b_le(_i: &mut Interp, args: &[Value]) -> BResult {
    let (a, b) = int2(args)?;
    Ok(Value::Bool(a <= b))
}
fn b_gt(_i: &mut Interp, args: &[Value]) -> BResult {
    let (a, b) = int2(args)?;
    Ok(Value::Bool(a > b))
}
fn b_ge(_i: &mut Interp, args: &[Value]) -> BResult {
    let (a, b) = int2(args)?;
    Ok(Value::Bool(a >= b))
}

fn b_show(i: &mut Interp, args: &[Value]) -> BResult {
    Ok(Value::text(crate::show::show(i, &args[0])))
}

fn b_cons(_i: &mut Interp, args: &[Value]) -> BResult {
    let xs = args[1].as_list()?;
    let mut v = Vec::with_capacity(xs.len() + 1);
    v.push(args[0].clone());
    v.extend_from_slice(xs);
    Ok(Value::list(v))
}

fn b_map(i: &mut Interp, args: &[Value]) -> BResult {
    want!(args, 0, Value::Fun(_), "a function");
    let xs = args[1].as_list()?;
    let mut out = Vec::with_capacity(xs.len());
    for x in xs.iter() {
        out.push(i.apply(args[0].clone(), x.clone())?);
    }
    Ok(Value::list(out))
}

fn b_filter(i: &mut Interp, args: &[Value]) -> BResult {
    want!(args, 0, Value::Fun(_), "a function");
    let xs = args[1].as_list()?;
    let mut out = Vec::new();
    for x in xs.iter() {
        match i.apply(args[0].clone(), x.clone())? {
            Value::Bool(true) => out.push(x.clone()),
            Value::Bool(false) => (),
            v => {
                return Err(Crash::new(format!(
                    "filter: predicate returned a {}",
                    v.kind_name()
                )))
            }
        }
    }
    Ok(Value::list(out))
}

fn b_length(_i: &mut Interp, args: &[Value]) -> BResult {
    Ok(Value::int(args[0].as_list()?.len() as i64))
}

fn b_null(_i: &mut Interp, args: &[Value]) -> BResult {
    Ok(Value::Bool(args[0].as_list()?.is_empty()))
}

fn b_head(_i: &mut Interp, args: &[Value]) -> BResult {
    let xs = args[0].as_list()?;
    xs.first()
        .cloned()
        .ok_or_else(|| Crash::new("head: empty list"))
}

fn b_tail(_i: &mut Interp, args: &[Value]) -> BResult {
    if args[0].as_list()?.is_empty() {
        return Err(Crash::new("tail: empty list"));
    }
    // share the elements rather than copying them: `up` is `tail
    // repo.context`, so a copying tail made every zipper walk quadratic
    match &args[0] {
        Value::List(xs) => Ok(Value::List(xs.skip(1))),
        v => Err(Crash::new(format!("expected a list, got a {}", v.kind_name()))),
    }
}

fn b_last(_i: &mut Interp, args: &[Value]) -> BResult {
    let xs = args[0].as_list()?;
    xs.last()
        .cloned()
        .ok_or_else(|| Crash::new("last: empty list"))
}

fn b_nth(_i: &mut Interp, args: &[Value]) -> BResult {
    let n = args[0].as_int()?;
    let xs = args[1].as_list()?;
    if n.sign() == num_bigint::Sign::Minus {
        return Err(Crash::new("nth: negative index"));
    }
    // a value that does not fit in usize is necessarily out of range
    let idx: usize = match n.to_string().parse() {
        Ok(i) => i,
        Err(_) => {
            return Err(Crash::new(format!("nth: index {} out of range", n)));
        }
    };
    xs.get(idx)
        .cloned()
        .ok_or_else(|| Crash::new(format!("nth: index {} out of range", idx)))
}

fn clamp(n: &BigInt, len: usize) -> Result<usize, Crash> {
    let s = n.to_string();
    if s.starts_with('-') {
        return Ok(0);
    }
    let v: usize = s.parse().unwrap_or(len);
    Ok(v.min(len))
}

fn b_take(_i: &mut Interp, args: &[Value]) -> BResult {
    let xs = args[1].as_list()?;
    let n = clamp(args[0].as_int()?, xs.len())?;
    Ok(Value::list(xs[..n].to_vec()))
}

fn b_drop(_i: &mut Interp, args: &[Value]) -> BResult {
    let n = clamp(args[0].as_int()?, args[1].as_list()?.len())?;
    match &args[1] {
        Value::List(xs) => Ok(Value::List(xs.skip(n))),
        v => Err(Crash::new(format!("expected a list, got a {}", v.kind_name()))),
    }
}

fn b_member(_i: &mut Interp, args: &[Value]) -> BResult {
    let xs = args[1].as_list()?;
    for x in xs.iter() {
        if value_eq(&args[0], x)? {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

fn b_range(_i: &mut Interp, args: &[Value]) -> BResult {
    let (a, b) = int2(args)?;
    let mut out = Vec::new();
    let mut x = a.clone();
    while x < *b {
        out.push(Value::Int(x.clone()));
        x += 1;
    }
    Ok(Value::list(out))
}

fn b_foldl(i: &mut Interp, args: &[Value]) -> BResult {
    want!(args, 0, Value::Fun(_), "a function");
    let xs = args[2].as_list()?;
    let mut acc = args[1].clone();
    for x in xs.iter() {
        let f = i.apply(args[0].clone(), acc)?;
        acc = i.apply(f, x.clone())?;
    }
    Ok(acc)
}

fn b_concat(_i: &mut Interp, args: &[Value]) -> BResult {
    let xs = args[0].as_list()?;
    if xs.is_empty() {
        return Err(Crash::new("concat: empty list"));
    }
    match &xs[0] {
        Value::List(_) => {
            let mut out = Vec::new();
            for x in xs {
                out.extend_from_slice(x.as_list()?);
            }
            Ok(Value::list(out))
        }
        Value::Text(_) => {
            let mut out = String::new();
            for x in xs {
                out.push_str(x.as_text()?);
            }
            Ok(Value::text(out))
        }
        v => Err(Crash::new(format!(
            "concat: cannot concatenate a list of {}",
            v.kind_name()
        ))),
    }
}

fn b_append(_i: &mut Interp, args: &[Value]) -> BResult {
    match (&args[0], &args[1]) {
        (Value::List(a), Value::List(b)) => {
            let mut out = a.to_vec();
            out.extend_from_slice(b);
            Ok(Value::list(out))
        }
        (Value::Text(a), Value::Text(b)) => Ok(Value::text(format!("{}{}", a, b))),
        (Value::List(_), v) | (v, Value::List(_)) => Err(Crash::new(format!(
            "++: expected a list, got a {}",
            v.kind_name()
        ))),
        (Value::Text(_), v) | (v, Value::Text(_)) => Err(Crash::new(format!(
            "++: expected Text, got a {}",
            v.kind_name()
        ))),
        (v, _) => Err(Crash::new(format!(
            "++: cannot append a {}",
            v.kind_name()
        ))),
    }
}

fn b_starts_with(_i: &mut Interp, args: &[Value]) -> BResult {
    let p = args[0].as_text()?;
    let t = args[1].as_text()?;
    Ok(Value::Bool(t.starts_with(p)))
}

fn b_ends_with(_i: &mut Interp, args: &[Value]) -> BResult {
    let p = args[0].as_text()?;
    let t = args[1].as_text()?;
    Ok(Value::Bool(t.ends_with(p)))
}

fn b_split_on(_i: &mut Interp, args: &[Value]) -> BResult {
    let sep = args[0].as_text()?;
    let t = args[1].as_text()?;
    if sep.is_empty() {
        return Err(Crash::new("splitOn: empty separator"));
    }
    let parts: Vec<Value> = t.split(sep).map(Value::text).collect();
    Ok(Value::list(parts))
}

// ----------------------------------------------------------------------
// repository builtins
// ----------------------------------------------------------------------

fn b_replay(i: &mut Interp, args: &[Value]) -> BResult {
    let onto = args[0].as_list()?;
    let ch = &args[1];
    let from = ch.field("from")?;
    let to = ch.field("to")?;
    let from = from.as_list()?;
    let to = to.as_list()?;
    let out = i.backend.replay(onto, from, to)?;
    Ok(Value::list(out))
}

fn b_unresolved(_i: &mut Interp, args: &[Value]) -> BResult {
    match &args[0] {
        Value::Blob(b) => Ok(Value::Bool(b.is_unresolved())),
        v => Err(Crash::new(format!(
            "unresolved: expected a Blob, got a {}",
            v.kind_name()
        ))),
    }
}

/// the paths whose content differs between `from` and `to` snapshots (§7.3):
/// one pass over both path-keyed maps, comparing blobs by content id — never
/// reading file bytes. Replaces the quadratic `entryAt`-per-path scan.
fn b_touched_paths(_i: &mut Interp, args: &[Value]) -> BResult {
    let ch = &args[0];
    let from = crate::domain::snapshot_map(ch.field("from")?.as_list()?)?;
    let to = crate::domain::snapshot_map(ch.field("to")?.as_list()?)?;
    let mut paths: std::collections::BTreeSet<Vec<String>> = std::collections::BTreeSet::new();
    paths.extend(from.keys().cloned());
    paths.extend(to.keys().cloned());
    let mut out = Vec::new();
    for p in paths {
        let changed = match (from.get(&p), to.get(&p)) {
            (None, Some(_)) | (Some(_), None) => true,
            (Some(a), Some(b)) => !crate::value::value_eq(a, b)?,
            (None, None) => false,
        };
        if changed {
            out.push(Value::list(p.into_iter().map(Value::text).collect()));
        }
    }
    Ok(Value::list(out))
}

fn b_blob(_i: &mut Interp, args: &[Value]) -> BResult {
    let t = args[0].as_text()?;
    Ok(Value::Blob(Rc::new(BlobVal {
        kind: BlobKind::Regular,
        content: BlobContent::Resolved(Rc::new(t.as_bytes().to_vec())),
    })))
}

fn b_text(_i: &mut Interp, args: &[Value]) -> BResult {
    match &args[0] {
        Value::Blob(b) => {
            let bytes = b.bytes()?;
            let s = String::from_utf8(bytes)
                .map_err(|_| Crash::new("text: blob content is not UTF-8"))?;
            Ok(Value::text(s))
        }
        v => Err(Crash::new(format!(
            "text: expected a Blob, got a {}",
            v.kind_name()
        ))),
    }
}

fn b_by(_i: &mut Interp, args: &[Value]) -> BResult {
    // Implemented via the generic walker (agrees with §10's reference).
    let id = match &args[0] {
        Value::Id(id) => id.to_string(),
        v => {
            return Err(Crash::new(format!(
                "by: expected an Id, got a {}",
                v.kind_name()
            )))
        }
    };
    let repo = &args[1];
    match crate::repo::by_id(repo, &id)? {
        Some(r) => Ok(r),
        None => Err(Crash::new("by: no such commit")),
    }
}

fn b_meta(i: &mut Interp, args: &[Value]) -> BResult {
    let id = match &args[0] {
        Value::Id(id) => id.to_string(),
        v => {
            return Err(Crash::new(format!(
                "meta: expected an Id, got a {}",
                v.kind_name()
            )))
        }
    };
    let m = i.backend.meta(&id)?;
    Ok(Value::record(&[
        ("hash", Value::text(m.hash)),
        ("author", Value::text(m.author)),
        ("email", Value::text(m.email)),
        ("time", Value::Int(BigInt::from(m.time))),
    ]))
}

fn b_validate(i: &mut Interp, args: &[Value]) -> BResult {
    crate::repo::validate_repo(i, &args[0])?;
    Ok(args[0].clone())
}

fn b_diff(_i: &mut Interp, args: &[Value]) -> BResult {
    let a = blob_text_utf8(&args[0])?;
    let b = blob_text_utf8(&args[1])?;
    if a == b {
        return Ok(Value::text(""));
    }
    Ok(Value::text(crate::show::unified_diff(&a, &b)))
}

fn blob_text_utf8(v: &Value) -> Result<String, Crash> {
    match v {
        Value::Blob(b) => String::from_utf8(b.bytes()?)
            .map_err(|_| Crash::new("diff: blob content is not UTF-8")),
        v => Err(Crash::new(format!(
            "diff: expected a Blob, got a {}",
            v.kind_name()
        ))),
    }
}

fn b_difft(_i: &mut Interp, args: &[Value]) -> BResult {
    crate::show::difft(&args[0], &args[1], &args[2])
}

fn b_tree_with(i: &mut Interp, args: &[Value]) -> BResult {
    crate::render::tree_with(i, &args[0], &args[1])
}

fn b_extract(_i: &mut Interp, args: &[Value]) -> BResult {
    let shape = match &args[0] {
        Value::Shape(s) => s.clone(),
        v => {
            return Err(Crash::new(format!(
                "extract: expected a Shape, got a {}",
                v.kind_name()
            )))
        }
    };
    let mut out = Vec::new();
    extract_into(&shape, &args[1], &mut out)?;
    Ok(Value::list(out))
}

fn extract_into(
    shape: &Rc<crate::value::ShapeVal>,
    v: &Value,
    out: &mut Vec<Value>,
) -> Result<(), Crash> {
    use crate::value::{PrimKind, ShapeKind};
    // a commit's `files` is a thunk until something asks for it; the language
    // must not see the difference, so force before matching and recursing —
    // otherwise `extract Entry repo` finds nothing on a real repository while
    // finding everything on the in-memory backend
    let forced = v.forced()?;
    let v = &forced;
    let matches = match (&shape.kind, v) {
        (ShapeKind::Prim(PrimKind::Int), Value::Int(_)) => true,
        (ShapeKind::Prim(PrimKind::Text), Value::Text(_)) => true,
        (ShapeKind::Prim(PrimKind::Bool), Value::Bool(_)) => true,
        (ShapeKind::Prim(PrimKind::Id), Value::Id(_)) => true,
        (ShapeKind::Prim(PrimKind::Blob), Value::Blob(_)) => true,
        (ShapeKind::Record(fields), Value::Record(m)) => {
            let got: BTreeSet<String> = m.keys().cloned().collect();
            *fields == got
        }
        _ => false,
    };
    if matches {
        out.push(v.clone());
    }
    match v {
        Value::List(xs) => {
            for x in xs.iter() {
                extract_into(shape, x, out)?;
            }
        }
        Value::Record(m) => {
            for (_, x) in m.iter() {
                extract_into(shape, x, out)?;
            }
        }
        _ => {}
    }
    Ok(())
}
