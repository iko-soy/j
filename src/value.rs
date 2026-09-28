//! Values of the j language (§2).

use crate::ast::{Expr, Pattern};
use num_bigint::BigInt;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

pub type RecordMap = BTreeMap<String, Value>;

#[derive(Clone)]
pub enum Value {
    Int(BigInt),
    Text(Rc<String>),
    Bool(bool),
    List(ListVal),
    Record(Rc<RecordMap>),
    Fun(Rc<FunVal>),
    Id(Rc<String>),
    Blob(Rc<BlobVal>),
    Shape(Rc<ShapeVal>),
    /// a value computed on first use (§2): currently used for a commit's
    /// `files` list, so loading a repo does not materialize every commit's
    /// file list. Forced transparently by `Value::field` and `value_eq`.
    Thunk(Rc<ThunkVal>),
}

/// A list: shared elements plus the offset of its first one, so dropping a
/// prefix (`tail`, `drop`) shares the storage instead of copying it. The
/// zipper walks — `up` is `tail repo.context`, and `top`/`ancestors` apply it
/// once per level — made a copying `tail` quadratic in the depth of history.
/// A tail keeps the elements before it alive; `j` is a single short run, so
/// holding them costs nothing.
#[derive(Clone)]
pub struct ListVal {
    items: Rc<Vec<Value>>,
    start: usize,
}

impl ListVal {
    pub fn new(items: Vec<Value>) -> ListVal {
        ListVal {
            items: Rc::new(items),
            start: 0,
        }
    }
    /// the list without its first `n` elements, sharing the storage
    pub fn skip(&self, n: usize) -> ListVal {
        ListVal {
            items: self.items.clone(),
            start: self.items.len().min(self.start.saturating_add(n)),
        }
    }
    pub fn as_slice(&self) -> &[Value] {
        &self.items[self.start..]
    }
}

impl std::ops::Deref for ListVal {
    type Target = [Value];
    fn deref(&self) -> &[Value] {
        self.as_slice()
    }
}

pub struct ThunkVal {
    state: std::cell::RefCell<ThunkState>,
    /// for a commit's `files` as the backend built them: the change id of the
    /// stored commit whose tree this loads. An edit that changes the files
    /// replaces the thunk, so a commit still holding the one tagged with its
    /// own id holds exactly the stored tree — the only case in which the
    /// backend's answers about that tree describe the value (§7.2).
    origin: Option<String>,
    /// for a commit's `files` as the backend built them: a name for the
    /// stored tree this loads. Two lists loaded under the same name are
    /// equal, and a list the backend loads from a tree is a snapshot (§7.3),
    /// so persisting compares and validates a commit the program left as
    /// loaded without reading its files (§7.5).
    tree: Option<String>,
}

enum ThunkState {
    Pending(Box<dyn FnOnce() -> Result<Value, Crash>>),
    /// the computation has been taken out of `Pending` and is running
    Forcing,
    Ready(Value),
    /// the computation crashed (a store read failed). Every later force
    /// crashes the same way (§7.2): a crash caught by `or`, or swallowed by
    /// rendering, must not leave some other value behind.
    Failed(Crash),
}

impl std::fmt::Debug for ThunkVal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &*self.state.borrow() {
            ThunkState::Pending(_) => write!(f, "Thunk(pending)"),
            ThunkState::Forcing => write!(f, "Thunk(forcing)"),
            ThunkState::Ready(v) => write!(f, "Thunk({:?})", v),
            ThunkState::Failed(c) => write!(f, "Thunk(failed: {})", c.msg),
        }
    }
}

impl ThunkVal {
    pub fn new(f: impl FnOnce() -> Result<Value, Crash> + 'static) -> Self {
        ThunkVal {
            state: std::cell::RefCell::new(ThunkState::Pending(Box::new(f))),
            origin: None,
            tree: None,
        }
    }

    /// the lazy `files` of the stored commit with change id `origin`, which
    /// load the stored tree named `tree`, if the backend names its trees
    pub fn stored(
        origin: String,
        tree: Option<String>,
        f: impl FnOnce() -> Result<Value, Crash> + 'static,
    ) -> Self {
        ThunkVal {
            origin: Some(origin),
            tree,
            ..ThunkVal::new(f)
        }
    }

    /// the change id of the stored commit whose tree this loads, if any
    pub fn origin(&self) -> Option<&str> {
        self.origin.as_deref()
    }

    /// the name of the stored tree this loads, if the backend gave one
    pub fn tree(&self) -> Option<&str> {
        self.tree.as_deref()
    }

    /// the value, computing it on first call and memoizing the outcome, a
    /// crash included
    pub fn force(&self) -> Result<Value, Crash> {
        match &*self.state.borrow() {
            ThunkState::Ready(v) => return Ok(v.clone()),
            ThunkState::Failed(c) => return Err(c.clone()),
            ThunkState::Forcing => {
                return Err(Crash::new("internal: a lazy value was forced re-entrantly"))
            }
            ThunkState::Pending(_) => {}
        }
        let thunk = match std::mem::replace(&mut *self.state.borrow_mut(), ThunkState::Forcing) {
            ThunkState::Pending(t) => t,
            _ => unreachable!("the state was Pending just above"),
        };
        let out = thunk();
        *self.state.borrow_mut() = match &out {
            Ok(v) => ThunkState::Ready(v.clone()),
            Err(c) => ThunkState::Failed(c.clone()),
        };
        out
    }
}

impl Value {
    /// force a thunk; every other value is returned unchanged
    pub fn forced(&self) -> Result<Value, Crash> {
        match self {
            Value::Thunk(t) => t.force(),
            _ => Ok(self.clone()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlobKind {
    Regular,
    Executable,
    Symlink,
}

#[derive(Clone)]
pub enum BlobContent {
    Resolved(Rc<Vec<u8>>),
    /// not yet read from the store; `force` reads the bytes on first use and
    /// memoizes them. `id` is the content hash (jj FileId hex), which is
    /// enough for equality and for persisting an unchanged blob without ever
    /// reading its bytes.
    Lazy(Rc<LazyBlob>),
    /// Conflict sides, jj order: alternating adds and removes, starting and
    /// ending with an add: [add, (remove, add)*]. For the in-memory backend:
    /// [to, from, onto]-style three sides as [add, remove, add]. `None` is a
    /// side on which the path is absent (a deletion conflict, §7.3).
    Conflict(Vec<Option<ConflictSide>>),
}

/// One present side of a conflict: its content and its own file type, which
/// jj keeps per side (§7.3)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictSide {
    pub kind: BlobKind,
    pub bytes: Rc<Vec<u8>>,
    /// a side on which jj holds a directory, in a conflict between a file
    /// and a directory: the hex id of that tree, kept opaque so the side is
    /// written back as the directory it was. Its `bytes` are empty (§7.3).
    pub tree: Option<String>,
}

impl ConflictSide {
    pub fn regular(bytes: &[u8]) -> ConflictSide {
        ConflictSide {
            kind: BlobKind::Regular,
            bytes: Rc::new(bytes.to_vec()),
            tree: None,
        }
    }
}

/// A blob whose bytes are read from the store only on first use (§7.2):
/// building the Repo value must not inflate every file of every commit.
pub struct LazyBlob {
    /// content hash (jj FileId hex)
    pub id: String,
    state: std::cell::RefCell<LazyState>,
}

enum LazyState {
    Pending(Box<dyn FnOnce() -> Result<Rc<Vec<u8>>, Crash>>),
    /// the read has been taken out of `Pending` and is running
    Forcing,
    Ready(Rc<Vec<u8>>),
    /// the read failed: every later force fails the same way, so the blob
    /// never reads as empty (§7.2), whoever caught or swallowed the crash
    Failed(Crash),
}

impl std::fmt::Debug for LazyBlob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Lazy({})", self.id)
    }
}

impl std::fmt::Debug for BlobContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlobContent::Resolved(b) => write!(f, "Resolved({} bytes)", b.len()),
            BlobContent::Lazy(l) => write!(f, "{:?}", l),
            BlobContent::Conflict(s) => write!(f, "Conflict({} sides)", s.len()),
        }
    }
}

impl LazyBlob {
    pub fn new(id: String, force: impl FnOnce() -> Result<Rc<Vec<u8>>, Crash> + 'static) -> Self {
        LazyBlob {
            id,
            state: std::cell::RefCell::new(LazyState::Pending(Box::new(force))),
        }
    }

    /// the bytes, reading from the store on first call and memoizing the
    /// outcome, a failed read included
    pub fn force(&self) -> Result<Rc<Vec<u8>>, Crash> {
        // fast path: already forced
        match &*self.state.borrow() {
            LazyState::Ready(b) => return Ok(b.clone()),
            LazyState::Failed(c) => return Err(c.clone()),
            LazyState::Forcing => {
                return Err(Crash::new("internal: a lazy blob was read re-entrantly"))
            }
            LazyState::Pending(_) => {}
        }
        let thunk = match std::mem::replace(&mut *self.state.borrow_mut(), LazyState::Forcing) {
            LazyState::Pending(t) => t,
            _ => unreachable!("the state was Pending just above"),
        };
        let out = thunk();
        *self.state.borrow_mut() = match &out {
            Ok(b) => LazyState::Ready(b.clone()),
            Err(c) => LazyState::Failed(c.clone()),
        };
        out
    }
}

#[derive(Clone, Debug)]
pub struct BlobVal {
    pub kind: BlobKind,
    pub content: BlobContent,
}

impl BlobVal {
    pub fn text_blob(s: &str) -> Value {
        Value::Blob(Rc::new(BlobVal {
            kind: BlobKind::Regular,
            content: BlobContent::Resolved(Rc::new(s.as_bytes().to_vec())),
        }))
    }
    pub fn is_unresolved(&self) -> bool {
        matches!(self.content, BlobContent::Conflict(_))
    }
    /// size in bytes; a lazy blob reports the size of its (memoized) bytes,
    /// forcing on first call
    pub fn size(&self) -> usize {
        match &self.content {
            BlobContent::Resolved(b) => b.len(),
            BlobContent::Lazy(l) => l.force().map(|b| b.len()).unwrap_or(0),
            BlobContent::Conflict(sides) => sides.iter().flatten().map(|s| s.bytes.len()).sum(),
        }
    }
    /// content as bytes, forcing a lazy blob; conflicts render with jj-style
    /// conflict markers
    pub fn bytes(&self) -> Result<Vec<u8>, Crash> {
        match &self.content {
            BlobContent::Resolved(b) => Ok(b.as_ref().clone()),
            BlobContent::Lazy(l) => Ok(l.force()?.as_ref().clone()),
            BlobContent::Conflict(sides) => Ok(render_conflict(sides)),
        }
    }
    /// number of lines, counted over the bytes in place. Equivalent to
    /// `String::from_utf8_lossy(&self.bytes()?).lines().count()` — a `\n`
    /// byte is always a `\n` character in UTF-8, and lossy replacement never
    /// introduces one — but without copying the content out.
    pub fn line_count(&self) -> usize {
        fn count(b: &[u8]) -> usize {
            let nl = bytecount(b);
            if b.is_empty() || b.ends_with(b"\n") {
                nl
            } else {
                nl + 1
            }
        }
        fn bytecount(b: &[u8]) -> usize {
            b.iter().filter(|c| **c == b'\n').count()
        }
        match &self.content {
            BlobContent::Resolved(b) => count(b),
            BlobContent::Lazy(l) => l.force().map(|b| count(&b)).unwrap_or(1),
            BlobContent::Conflict(sides) => count(&render_conflict(sides)),
        }
    }
    /// the content hash of a lazy blob, if it is one (used to persist an
    /// unchanged blob without reading its bytes)
    pub fn lazy_id(&self) -> Option<&str> {
        match &self.content {
            BlobContent::Lazy(l) => Some(&l.id),
            _ => None,
        }
    }
}

/// Render a conflict with jj-style markers; a side the path is absent on
/// renders as empty content, as jj materialises it.
pub fn render_conflict(sides: &[Option<ConflictSide>]) -> Vec<u8> {
    // sides: [add0, remove0, add1, remove1, ... addN]
    let mut out = Vec::new();
    out.extend_from_slice(b"<<<<<<<\n");
    let mut i = 0;
    while i < sides.len() {
        let bytes: &[u8] = sides[i].as_ref().map_or(&[], |s| &s.bytes);
        if i % 2 == 0 {
            out.extend_from_slice(b"+++++++\n");
            out.extend_from_slice(bytes);
            ensure_newline(&mut out);
        } else {
            out.extend_from_slice(b"%%%%%%%\n");
            out.extend_from_slice(bytes);
            ensure_newline(&mut out);
        }
        i += 1;
    }
    out.extend_from_slice(b">>>>>>>\n");
    out
}

fn ensure_newline(v: &mut Vec<u8>) {
    if !v.is_empty() && !v.ends_with(b"\n") {
        v.push(b'\n');
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrimKind {
    Int,
    Text,
    Bool,
    Id,
    Blob,
}

impl PrimKind {
    pub fn name(&self) -> &'static str {
        match self {
            PrimKind::Int => "Int",
            PrimKind::Text => "Text",
            PrimKind::Bool => "Bool",
            PrimKind::Id => "Id",
            PrimKind::Blob => "Blob",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ShapeKind {
    Record(BTreeSet<String>),
    Prim(PrimKind),
}

#[derive(Clone, Debug)]
pub struct ShapeVal {
    pub name: String,
    pub kind: ShapeKind,
}

impl PartialEq for ShapeVal {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

pub type BuiltinFn = fn(&mut crate::eval::Interp, &[Value]) -> Result<Value, Crash>;

pub enum FunVal {
    Builtin {
        name: String,
        arity: usize,
        args: Vec<Value>,
        f: BuiltinFn,
        /// the name this function is under, its contract as further
        /// arguments arrive, and how many of `args` it held when it was
        /// given that name, which are its value's, not the arguments a
        /// signed definition was given (§5.2)
        pending: Option<(String, Rc<crate::shape::ContractExpr>, usize)>,
    },
    Closure {
        name: Option<String>, // top-level definition name, if any
        params: Vec<Pattern>,
        applied: usize,
        /// the arguments supplied so far, for rendering (§5.2)
        applied_args: Vec<Value>,
        body: Rc<Expr>,
        env: Env,
        /// the lambda's source, which it renders as (§5.2)
        src: crate::ast::Source,
        /// contract of this function as further arguments arrive
        pending: Option<(String, Rc<crate::shape::ContractExpr>)>,
    },
    /// `f or g` lifted pointwise over functions (§4.6)
    OrFun(Value, Value),
    /// a composition `f . g` applied as `f (g x)` (§4.9); `(.)` itself
    /// builds a builtin node instead (builtins::compose_values)
    ComposeLazy(Value, Value),
    /// a label literal %name: applies as `labelled "name"`
    Labelled(String, Value),
}

/// What a function holds that may hold further functions: a closure's
/// environment and arguments, a builtin's arguments, the operands of an
/// `or` or a composition, a label's `labelled`. It is only ever dropped.
#[allow(dead_code)]
enum Held {
    Closure(Env, Vec<Value>),
    Args(Vec<Value>),
    Two(Value, Value),
    One(Value),
}

/// How many function drops run inside one another before the next one's
/// contents are put off to the outermost.
const INLINE_FUN_DROPS: usize = 64;

struct FunDrops {
    depth: std::cell::Cell<usize>,
    put_off: std::cell::RefCell<Vec<Held>>,
}

thread_local! {
    static FUN_DROPS: FunDrops = const {
        FunDrops {
            depth: std::cell::Cell::new(0),
            put_off: std::cell::RefCell::new(Vec::new()),
        }
    };
}

/// A function is dropped with what it holds, which may be a function holding
/// a function, and so on: a closure over a closure, a composition of
/// compositions, a named wrapper around another (named_apply). Dropped the
/// usual way, such a chain recursed once per link on the native stack, and a
/// chain a recursion had built a million links long aborted the process
/// (§4.1). Past a small depth, what a function holds is put off, and the
/// outermost function drop drops it, so the stack a drop takes is bounded.
impl Drop for FunVal {
    fn drop(&mut self) {
        let held = match self {
            FunVal::Closure {
                env, applied_args, ..
            } => Held::Closure(std::mem::replace(env, Env::empty()), std::mem::take(applied_args)),
            FunVal::Builtin { args, .. } => Held::Args(std::mem::take(args)),
            FunVal::OrFun(a, b) | FunVal::ComposeLazy(a, b) => Held::Two(
                std::mem::replace(a, Value::Bool(false)),
                std::mem::replace(b, Value::Bool(false)),
            ),
            FunVal::Labelled(_, v) => Held::One(std::mem::replace(v, Value::Bool(false))),
        };
        // once the thread's locals are gone, `held` simply drops here
        let _ = FUN_DROPS.try_with(move |d| {
            let depth = d.depth.get();
            if depth >= INLINE_FUN_DROPS {
                d.put_off.borrow_mut().push(held);
                return;
            }
            d.depth.set(depth + 1);
            drop(held);
            if depth == 0 {
                loop {
                    let next = d.put_off.borrow_mut().pop();
                    match next {
                        Some(h) => drop(h),
                        None => break,
                    }
                }
            }
            d.depth.set(depth);
        });
    }
}

impl Value {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "Int",
            Value::Text(_) => "Text",
            Value::Bool(_) => "Bool",
            Value::List(_) => "list",
            Value::Record(_) => "record",
            Value::Fun(_) => "function",
            Value::Id(_) => "Id",
            Value::Blob(_) => "Blob",
            Value::Shape(_) => "Shape",
            // a thunk stands in for its eventual value (currently a list of
            // file entries); kind_name is used in error messages
            Value::Thunk(_) => "list",
        }
    }

    pub fn record(fields: &[(&str, Value)]) -> Value {
        let mut m = RecordMap::new();
        for (k, v) in fields {
            m.insert(k.to_string(), v.clone());
        }
        Value::Record(Rc::new(m))
    }

    pub fn field(&self, name: &str) -> Result<Value, Crash> {
        match self {
            Value::Record(m) => m
                .get(name)
                .cloned()
                .ok_or_else(|| Crash::new(format!("record has no field `{}`", name)))
                .and_then(|v| v.forced()),
            _ => Err(Crash::new(format!(
                "cannot select field `{}` from a {}",
                name,
                self.kind_name()
            ))),
        }
    }

    pub fn field_set(&self) -> Option<BTreeSet<String>> {
        match self {
            Value::Record(m) => Some(m.keys().cloned().collect()),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Result<&[Value], Crash> {
        match self {
            Value::List(xs) => Ok(xs.as_slice()),
            _ => Err(Crash::new(format!("expected a list, got a {}", self.kind_name()))),
        }
    }

    pub fn as_text(&self) -> Result<&str, Crash> {
        match self {
            Value::Text(t) => Ok(t),
            _ => Err(Crash::new(format!("expected Text, got a {}", self.kind_name()))),
        }
    }

    pub fn as_int(&self) -> Result<&BigInt, Crash> {
        match self {
            Value::Int(n) => Ok(n),
            _ => Err(Crash::new(format!("expected Int, got a {}", self.kind_name()))),
        }
    }

    pub fn text(s: impl Into<String>) -> Value {
        Value::Text(Rc::new(s.into()))
    }

    pub fn int(n: i64) -> Value {
        Value::Int(BigInt::from(n))
    }

    pub fn list(xs: Vec<Value>) -> Value {
        Value::List(ListVal::new(xs))
    }

    pub fn bool(b: bool) -> Value {
        Value::Bool(b)
    }
}

/// Attach a definition name and contract to a function value (§4.13).
pub fn attach_pending(v: &Value, name: &str, contract: Rc<crate::shape::Contract>) -> Value {
    let cexpr = Rc::new(crate::shape::ContractExpr::Known { contract, at: 0 });
    let pending = Some((name.to_string(), cexpr.clone()));
    match v {
        Value::Fun(fv) => match fv.as_ref() {
            FunVal::Closure {
                params,
                applied,
                body,
                env,
                src,
                ..
            } => Value::Fun(Rc::new(FunVal::Closure {
                name: Some(name.to_string()),
                params: params.clone(),
                applied: *applied,
                // the definition renders as its name, followed only by the
                // arguments it is given, not those its value holds: `prev`,
                // not `prev (parents)` (§5.2)
                applied_args: Vec::new(),
                body: body.clone(),
                env: env.clone(),
                src: src.clone(),
                pending,
            })),
            FunVal::Builtin {
                name: bname,
                arity,
                args,
                f,
                ..
            } => Value::Fun(Rc::new(FunVal::Builtin {
                name: bname.clone(),
                arity: *arity,
                args: args.clone(),
                f: *f,
                // the arguments it holds now are its value's, and those
                // after them the definition's own: `k3 ((+)) 0` with
                // `k3 = foldl`, past a signature that ends in a type
                // variable and so stops counting (§5.2)
                pending: Some((name.to_string(), cexpr, args.len())),
            })),
            // `f or g` and `%name` have nowhere to hold a contract, so the
            // definition becomes `\x -> v x` under its name, which the
            // closure machinery checks at every argument the signature lists
            // and at the result. The check is around the lifted `or` as a
            // whole (§4.6): a result that violates the signature is a crash,
            // not a reason to try the other side.
            FunVal::OrFun(_, _) | FunVal::ComposeLazy(_, _) | FunVal::Labelled(_, _) => {
                named_apply(v.clone(), Vec::new(), name, cexpr)
            }
        },
        _ => v.clone(),
    }
}

/// `\x -> f x` under a definition's name and contract: it renders as the
/// name followed by `applied_args` (§5.2), and its further arguments and its
/// result are checked against the contract (§4.13)
pub fn named_apply(
    f: Value,
    applied_args: Vec<Value>,
    name: &str,
    cexpr: Rc<crate::shape::ContractExpr>,
) -> Value {
    Value::Fun(Rc::new(FunVal::Closure {
        name: Some(name.to_string()),
        params: vec![Pattern::Var("x".to_string())],
        applied: 0,
        applied_args,
        body: NAMED_APPLY_BODY.with(Rc::clone),
        env: Env::empty().extend(vec![("f".to_string(), f)]),
        // a named closure renders by its name, not its source (§5.2)
        src: crate::ast::Source::new(Rc::from(""), 0, 0),
        pending: Some((name.to_string(), cexpr)),
    }))
}

thread_local! {
    /// `f x`, the body every named_apply wrapper shares, by which one is
    /// told from a lambda written in the language
    static NAMED_APPLY_BODY: Rc<Expr> = {
        let var = |n: &str| Rc::new(Expr::Var(n.to_string()));
        Rc::new(Expr::App(var("f"), var("x")))
    };
}

/// The function `v` wraps when it is a named_apply wrapper under `name` and a
/// contract the same as `cexpr`. Naming that function afresh under them
/// checks all the wrapper would, so a definition that names the value of its
/// own recursive call wraps it once, not once per call (§4.13).
pub fn named_inner(v: &Value, name: &str, cexpr: &crate::shape::ContractExpr) -> Option<Value> {
    match v {
        Value::Fun(fv) => match fv.as_ref() {
            FunVal::Closure {
                body,
                env,
                pending: Some((n, c)),
                ..
            } if n == name
                && c.same_as(cexpr)
                && NAMED_APPLY_BODY.with(|b| Rc::ptr_eq(b, body)) =>
            {
                env.lookup("f")
            }
            _ => None,
        },
        _ => None,
    }
}

/// A crash (§4.7).
#[derive(Debug, Clone)]
pub struct Crash {
    pub msg: String,
    /// innermost base definition executing, if any
    pub def: Option<String>,
}

impl Crash {
    pub fn new(msg: impl Into<String>) -> Crash {
        Crash {
            msg: msg.into(),
            def: None,
        }
    }
}

// ----------------------------------------------------------------------
// Environments (persistent, for closures)
// ----------------------------------------------------------------------

#[derive(Clone)]
pub struct Env {
    frame: Option<Rc<Frame>>,
}

pub struct Frame {
    kind: FrameKind,
    parent: Env,
}

enum FrameKind {
    Small(Vec<(String, Value)>),
    Big(HashMap<String, Value>),
    /// recursive let bindings: the names the block binds, and their values
    /// in that order, filled incrementally and read through the cell so
    /// closures capturing the frame see later bindings (§4.1)
    Rec(Rc<[String]>, Rc<std::cell::RefCell<Vec<Value>>>),
}

impl Env {
    pub fn empty() -> Env {
        Env { frame: None }
    }

    pub fn with_globals(globals: HashMap<String, Value>) -> Env {
        Env {
            frame: Some(Rc::new(Frame {
                kind: FrameKind::Big(globals),
                parent: Env::empty(),
            })),
        }
    }

    pub fn extend(&self, bindings: Vec<(String, Value)>) -> Env {
        Env {
            frame: Some(Rc::new(Frame {
                kind: FrameKind::Small(bindings),
                parent: self.clone(),
            })),
        }
    }

    /// extend with a recursive frame (for `let` blocks) binding `names`,
    /// whose values are pushed to the returned cell in the same order
    pub fn extend_rec(&self, names: Rc<[String]>) -> (Env, Rc<std::cell::RefCell<Vec<Value>>>) {
        let cell = Rc::new(std::cell::RefCell::new(Vec::with_capacity(names.len())));
        (
            Env {
                frame: Some(Rc::new(Frame {
                    kind: FrameKind::Rec(names, cell.clone()),
                    parent: self.clone(),
                })),
            },
            cell,
        )
    }

    pub fn lookup(&self, name: &str) -> Option<Value> {
        let mut cur = self.frame.as_ref();
        while let Some(f) = cur {
            match &f.kind {
                FrameKind::Small(bs) => {
                    for (n, v) in bs.iter().rev() {
                        if n == name {
                            return Some(v.clone());
                        }
                    }
                }
                FrameKind::Rec(names, cell) => {
                    // the frame binds its names before it holds their values:
                    // one not evaluated yet is unbound here, not a same-named
                    // definition further out, which config.j's binders may
                    // reuse (§4.2)
                    if let Some(i) = names.iter().rposition(|n| n == name) {
                        return cell.borrow().get(i).cloned();
                    }
                }
                FrameKind::Big(m) => {
                    if let Some(v) = m.get(name) {
                        return Some(v.clone());
                    }
                }
            }
            cur = f.parent.frame.as_ref();
        }
        None
    }

    /// True if this environment captures anything beyond the globals frame
    /// (i.e. the closure was created inside a lambda/let body).
    pub fn is_closure_capture(&self) -> bool {
        let mut cur = self.frame.as_ref();
        let mut small_frames = 0;
        while let Some(f) = cur {
            match &f.kind {
                FrameKind::Small(_) => small_frames += 1,
                FrameKind::Big(_) | FrameKind::Rec(..) => {}
            }
            cur = f.parent.frame.as_ref();
        }
        small_frames > 0
    }

    pub fn names(&self, out: &mut BTreeSet<String>) {
        let mut cur = self.frame.as_ref();
        while let Some(f) = cur {
            match &f.kind {
                FrameKind::Small(bs) => {
                    for (n, _) in bs {
                        out.insert(n.clone());
                    }
                }
                FrameKind::Rec(names, _) => {
                    for n in names.iter() {
                        out.insert(n.clone());
                    }
                }
                FrameKind::Big(m) => {
                    for n in m.keys() {
                        out.insert(n.clone());
                    }
                }
            }
            cur = f.parent.frame.as_ref();
        }
    }
}

// ----------------------------------------------------------------------
// structural equality (§4.5); comparing functions is a crash
// ----------------------------------------------------------------------

pub fn value_eq(a: &Value, b: &Value) -> Result<bool, Crash> {
    // force thunks at the boundary so callers never see them
    if let Value::Thunk(_) = a {
        return value_eq(&a.forced()?, b);
    }
    if let Value::Thunk(_) = b {
        return value_eq(a, &b.forced()?);
    }
    Ok(match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Text(x), Value::Text(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Id(x), Value::Id(y)) => x == y,
        (Value::List(x), Value::List(y)) => {
            if x.len() != y.len() {
                return Ok(false);
            }
            for (u, v) in x.iter().zip(y.iter()) {
                if !value_eq(u, v)? {
                    return Ok(false);
                }
            }
            true
        }
        (Value::Record(x), Value::Record(y)) => {
            if x.len() != y.len() {
                return Ok(false);
            }
            for ((k1, v1), (k2, v2)) in x.iter().zip(y.iter()) {
                if k1 != k2 || !value_eq(v1, v2)? {
                    return Ok(false);
                }
            }
            true
        }
        (Value::Blob(x), Value::Blob(y)) => {
            x.kind == y.kind && blob_content_eq(&x.content, &y.content)?
        }
        (Value::Shape(x), Value::Shape(y)) => x == y,
        (Value::Fun(_), Value::Fun(_)) => {
            return Err(Crash::new("cannot compare functions for equality"))
        }
        _ => false,
    })
}

/// content equality; two lazy blobs compare by content hash, without forcing
fn blob_content_eq(a: &BlobContent, b: &BlobContent) -> Result<bool, Crash> {
    Ok(match (a, b) {
        (BlobContent::Resolved(x), BlobContent::Resolved(y)) => x == y,
        (BlobContent::Lazy(x), BlobContent::Lazy(y)) => x.id == y.id,
        (BlobContent::Lazy(x), BlobContent::Resolved(y))
        | (BlobContent::Resolved(y), BlobContent::Lazy(x)) => x.force()? == *y,
        (BlobContent::Conflict(x), BlobContent::Conflict(y)) => x == y,
        _ => false,
    })
}

impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // cheap debug rendering without an interpreter
        match self {
            Value::Int(n) => write!(f, "Int({})", n),
            Value::Text(t) => write!(f, "Text({:?})", t),
            Value::Bool(b) => write!(f, "Bool({})", b),
            Value::Id(i) => write!(f, "Id(@{})", i),
            Value::List(xs) => f.debug_list().entries(xs.iter()).finish(),
            Value::Record(m) => f.debug_map().entries(m.iter()).finish(),
            Value::Fun(_) => write!(f, "Fun(..)"),
            Value::Blob(b) => write!(f, "Blob({:?})", b),
            Value::Shape(s) => write!(f, "Shape({})", s.name),
            Value::Thunk(t) => write!(f, "{:?}", t),
        }
    }
}
