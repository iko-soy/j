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
}

enum ThunkState {
    Pending(Box<dyn FnOnce() -> Result<Value, Crash>>),
    Ready(Value),
}

impl std::fmt::Debug for ThunkVal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &*self.state.borrow() {
            ThunkState::Pending(_) => write!(f, "Thunk(pending)"),
            ThunkState::Ready(v) => write!(f, "Thunk({:?})", v),
        }
    }
}

impl ThunkVal {
    pub fn new(f: impl FnOnce() -> Result<Value, Crash> + 'static) -> Self {
        ThunkVal {
            state: std::cell::RefCell::new(ThunkState::Pending(Box::new(f))),
        }
    }

    /// the value, computing it on first call and memoizing
    pub fn force(&self) -> Result<Value, Crash> {
        if let ThunkState::Ready(v) = &*self.state.borrow() {
            return Ok(v.clone());
        }
        let thunk = {
            let mut st = self.state.borrow_mut();
            match std::mem::replace(&mut *st, ThunkState::Ready(Value::Bool(false))) {
                ThunkState::Pending(t) => t,
                ThunkState::Ready(v) => {
                    *st = ThunkState::Ready(v.clone());
                    return Ok(v);
                }
            }
        };
        let v = thunk()?;
        *self.state.borrow_mut() = ThunkState::Ready(v.clone());
        Ok(v)
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
}

impl ConflictSide {
    pub fn regular(bytes: &[u8]) -> ConflictSide {
        ConflictSide {
            kind: BlobKind::Regular,
            bytes: Rc::new(bytes.to_vec()),
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
    Ready(Rc<Vec<u8>>),
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

    /// the bytes, reading from the store on first call and memoizing
    pub fn force(&self) -> Result<Rc<Vec<u8>>, Crash> {
        // fast path: already forced
        if let LazyState::Ready(b) = &*self.state.borrow() {
            return Ok(b.clone());
        }
        let thunk = {
            let mut st = self.state.borrow_mut();
            match std::mem::replace(&mut *st, LazyState::Ready(Rc::new(Vec::new()))) {
                LazyState::Pending(t) => t,
                LazyState::Ready(b) => {
                    *st = LazyState::Ready(b.clone());
                    return Ok(b);
                }
            }
        };
        let bytes = thunk()?;
        *self.state.borrow_mut() = LazyState::Ready(bytes.clone());
        Ok(bytes)
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
    /// an alias that could not be resolved to a record shape yet (function,
    /// list, or a typedecl processed later); resolved lazily at check time
    Aliased(String),
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
        pending: Option<(String, Rc<crate::shape::ContractExpr>)>,
    },
    Closure {
        name: Option<String>, // top-level definition name, if any
        params: Vec<Pattern>,
        applied: usize,
        /// the arguments supplied so far, for rendering (§5.2)
        applied_args: Vec<Value>,
        /// the body has not been evaluated yet (the definition's contract is
        /// not exhausted): the next application evaluates it, then applies
        deferred: bool,
        body: Rc<Expr>,
        env: Env,
        src: String,
        /// contract of this function as further arguments arrive
        pending: Option<(String, Rc<crate::shape::ContractExpr>)>,
    },
    /// `f or g` lifted pointwise over functions (§4.6)
    OrFun(Value, Value),
    /// unevaluated composition: unfolds only when applied to a non-function
    /// (§4.1 note — `abandon . contract everything` is a value before it is
    /// applied, and must not run at load)
    ComposeLazy(Value, Value),
    /// a label literal %name: applies as `labelled "name"`
    Labelled(String, Value),
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
    let pending = Some((
        name.to_string(),
        Rc::new(crate::shape::ContractExpr::Known { contract, at: 0 }),
    ));
    match v {
        Value::Fun(fv) => match fv.as_ref() {
            FunVal::Closure {
                params,
                applied,
                applied_args,
                deferred,
                body,
                env,
                src,
                ..
            } => Value::Fun(Rc::new(FunVal::Closure {
                name: Some(name.to_string()),
                params: params.clone(),
                applied: *applied,
                applied_args: applied_args.clone(),
                deferred: *deferred,
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
                pending,
            })),
            FunVal::OrFun(_, _) | FunVal::ComposeLazy(_, _) | FunVal::Labelled(_, _) => v.clone(),
        },
        _ => v.clone(),
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
    /// recursive let bindings: filled incrementally, read through the cell so
    /// closures capturing the frame see later bindings (§4.1)
    Rec(Rc<std::cell::RefCell<Vec<(String, Value)>>>),
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

    /// extend with a recursive frame (for `let` blocks)
    pub fn extend_rec(&self) -> (Env, Rc<std::cell::RefCell<Vec<(String, Value)>>>) {
        let cell = Rc::new(std::cell::RefCell::new(Vec::new()));
        (
            Env {
                frame: Some(Rc::new(Frame {
                    kind: FrameKind::Rec(cell.clone()),
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
                FrameKind::Rec(cell) => {
                    for (n, v) in cell.borrow().iter().rev() {
                        if n == name {
                            return Some(v.clone());
                        }
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
                FrameKind::Big(_) | FrameKind::Rec(_) => {}
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
                FrameKind::Rec(cell) => {
                    for (n, _) in cell.borrow().iter() {
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
