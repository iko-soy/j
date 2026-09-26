//! Shape computation from typedecls (§4.12) and runtime contracts (§4.13).

use crate::ast::TypeExpr;
use crate::value::{PrimKind, ShapeKind, ShapeVal, Value};
use std::collections::{BTreeSet, HashMap, HashSet};

#[derive(Default, Clone)]
pub struct Shapes {
    /// name -> its declared type
    pub decls: HashMap<String, TypeExpr>,
    /// Memo for `shape_of`. A contract check against a record type asks for
    /// the shape on every call, and computing it builds the field set afresh;
    /// the declarations never change once the config is loaded.
    memo: std::cell::RefCell<HashMap<String, Option<ShapeVal>>>,
}

impl Shapes {
    pub fn new() -> Shapes {
        Shapes::default()
    }

    /// Compute the ShapeVal for a type name used as a value (§4.12).
    /// Returns None if the name is undeclared, or its shape is a function,
    /// a list, or otherwise not usable.
    pub fn shape_of(&self, name: &str) -> Option<ShapeVal> {
        if let Some(hit) = self.memo.borrow().get(name) {
            return hit.clone();
        }
        let computed = self.shape_of_uncached(name);
        self.memo
            .borrow_mut()
            .insert(name.to_string(), computed.clone());
        computed
    }

    fn shape_of_uncached(&self, name: &str) -> Option<ShapeVal> {
        match name {
            "Int" => Some(ShapeVal {
                name: name.into(),
                kind: ShapeKind::Prim(PrimKind::Int),
            }),
            "Text" => Some(ShapeVal {
                name: name.into(),
                kind: ShapeKind::Prim(PrimKind::Text),
            }),
            "Bool" => Some(ShapeVal {
                name: name.into(),
                kind: ShapeKind::Prim(PrimKind::Bool),
            }),
            "Id" => Some(ShapeVal {
                name: name.into(),
                kind: ShapeKind::Prim(PrimKind::Id),
            }),
            "Blob" => Some(ShapeVal {
                name: name.into(),
                kind: ShapeKind::Prim(PrimKind::Blob),
            }),
            "Shape" => None, // Shape itself is primitive but extract Shape is odd; crash
            _ => {
                let ty = self.decls.get(name)?;
                self.shape_of_type(ty).map(|kind| ShapeVal {
                    name: name.into(),
                    kind,
                })
            }
        }
    }

    fn shape_of_type(&self, ty: &TypeExpr) -> Option<ShapeKind> {
        // an alias takes its type's shape (§4.12); `unfold` is bounded, so a
        // cycle of aliases (`Self = Self`) ends on a name like any other
        match self.unfold(ty) {
            TypeExpr::Con(n) if is_primitive(n) => self.shape_of(n).map(|s| s.kind),
            // undeclared, a type variable, or an alias cycle: not a usable
            // shape, and a crash when used (§4.12)
            TypeExpr::Con(_) => None,
            TypeExpr::Record(fields) => Some(ShapeKind::Record(
                fields.iter().map(|(n, _)| n.clone()).collect(),
            )),
            TypeExpr::List(_) | TypeExpr::Fun(_, _) => None,
        }
    }

    /// Unfold an alias chain by reference. Contracts are checked at every
    /// call, so cloning the type each time — a `Vec` of `String`s for a record
    /// type — was a per-application allocation.
    pub fn unfold<'a>(&'a self, ty: &'a TypeExpr) -> &'a TypeExpr {
        let mut cur = ty;
        // a declaration cycle would otherwise spin here; the bound is far
        // above any real alias chain
        for _ in 0..64 {
            match cur {
                TypeExpr::Con(n) if !is_primitive(n) && !is_type_var(n) => {
                    match self.decls.get(n) {
                        Some(decl) => cur = decl,
                        None => return cur,
                    }
                }
                _ => return cur,
            }
        }
        cur
    }
}

fn is_primitive(n: &str) -> bool {
    matches!(n, "Int" | "Text" | "Bool" | "Id" | "Blob" | "Shape")
}

fn is_type_var(n: &str) -> bool {
    n.chars().next().map(|c| c.is_ascii_lowercase()).unwrap_or(false)
}

/// A compiled signature: a chain of parameter types and a final result type.
/// The types are shared, so handing one out is a refcount bump rather than a
/// deep copy — contracts are consulted on every call.
#[derive(Clone, Debug)]
pub struct Contract {
    pub params: Vec<Rc2<TypeExpr>>,
    pub result: Rc2<TypeExpr>,
}

pub fn compile_contract(shapes: &Shapes, ty: &TypeExpr) -> Contract {
    let mut params = Vec::new();
    let mut cur = ty;
    // an alias met again on this walk refers to itself (`Stream = Int ->
    // Stream`, or `A = B` with `B = A`) and would unfold forever; the walk
    // stops there, and the result is checked as that alias
    let mut unfolded: HashSet<&str> = HashSet::new();
    loop {
        match cur {
            TypeExpr::Fun(a, b) => {
                params.push(a.clone());
                cur = b;
            }
            TypeExpr::Con(n) => {
                // alias of a function type is unfolded (§4.13)
                if !is_primitive(n) && !is_type_var(n) && unfolded.insert(n.as_str()) {
                    if let Some(decl) = shapes.decls.get(n) {
                        cur = decl;
                        continue;
                    }
                }
                break;
            }
            _ => break,
        }
    }
    Contract {
        params,
        result: Rc2::new(cur.clone()),
    }
}

/// Check one level deep (§4.13). On failure, describe expected and got.
pub fn check(shapes: &Shapes, ty: &TypeExpr, v: &Value) -> Result<(), String> {
    let ty = shapes.unfold(ty);
    match ty {
        TypeExpr::Con(n) => {
            if is_type_var(n) {
                return Ok(());
            }
            match n.as_str() {
                "Int" => kind_check(matches!(v, Value::Int(_)), "Int", v),
                "Text" => kind_check(matches!(v, Value::Text(_)), "Text", v),
                "Bool" => kind_check(matches!(v, Value::Bool(_)), "Bool", v),
                "Id" => kind_check(matches!(v, Value::Id(_)), "Id", v),
                "Blob" => kind_check(matches!(v, Value::Blob(_)), "Blob", v),
                "Shape" => kind_check(matches!(v, Value::Shape(_)), "Shape", v),
                _ => {
                    // shape name: exact field set (aliases resolved by shape_of)
                    match shapes.shape_of(n) {
                        Some(ShapeVal {
                            kind: ShapeKind::Record(fields),
                            ..
                        }) => record_check(&fields, n, v),
                        Some(ShapeVal {
                            kind: ShapeKind::Prim(p),
                            ..
                        }) => {
                            let ok = matches!(
                                (p, v),
                                (PrimKind::Int, Value::Int(_))
                                    | (PrimKind::Text, Value::Text(_))
                                    | (PrimKind::Bool, Value::Bool(_))
                                    | (PrimKind::Id, Value::Id(_))
                                    | (PrimKind::Blob, Value::Blob(_))
                            );
                            kind_check(ok, n, v)
                        }
                        _ => Ok(()),
                    }
                }
            }
        }
        TypeExpr::List(_) => kind_check(matches!(v, Value::List(_)), "a list", v),
        TypeExpr::Record(fields) => {
            let set: BTreeSet<String> = fields.iter().map(|(n, _)| n.clone()).collect();
            record_check(&set, "record", v)
        }
        TypeExpr::Fun(_, _) => kind_check(matches!(v, Value::Fun(_)), "a function", v),
    }
}

fn kind_check(ok: bool, expected: &str, v: &Value) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(format!("expected {}, got {}", expected, v.kind_name()))
    }
}

fn record_check(expected: &BTreeSet<String>, name: &str, v: &Value) -> Result<(), String> {
    match v {
        Value::Record(m) => {
            let got: BTreeSet<String> = m.keys().cloned().collect();
            if *expected == got {
                Ok(())
            } else {
                Err(format!(
                    "expected {} {}, got {}",
                    if name == "record" { "" } else { name },
                    fmt_fields(expected),
                    fmt_fields(&got)
                ))
            }
        }
        _ => Err(format!("expected {}, got {}", name, v.kind_name())),
    }
}

pub fn fmt_fields(s: &BTreeSet<String>) -> String {
    format!("{{ {} }}", s.iter().cloned().collect::<Vec<_>>().join(", "))
}

/// A human-readable name for a type, used in contract messages.
pub fn type_name(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Con(n) => n.clone(),
        TypeExpr::List(t) => format!("[{}]", type_name(t)),
        TypeExpr::Record(fields) => {
            let parts: Vec<String> = fields
                .iter()
                .map(|(n, t)| format!("{} : {}", n, type_name(t)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
        TypeExpr::Fun(a, b) => format!("{} -> {}", type_name(a), type_name(b)),
    }
}

// ----------------------------------------------------------------------
// contract expressions: the type of a (possibly partially applied) function
// ----------------------------------------------------------------------

use std::rc::Rc as Rc2;

#[derive(Clone, Debug)]
pub enum ContractExpr {
    /// no contract information; arg checks pass through
    Unknown,
    /// a signature with its first `at` parameters already supplied; advancing
    /// it is an index bump, where rebuilding the shortened parameter list
    /// copied every remaining type on every application
    Known { contract: Rc2<Contract>, at: usize },
    /// compose(f, g): (f . g); both contract expressions
    Compose(Box<ContractExpr>, Box<ContractExpr>),
    /// apply e to an argument that checked against e's first parameter
    ApplyFirst(Box<ContractExpr>),
}

impl ContractExpr {
    /// The type of the next parameter this function expects, if known.
    pub fn next_param(&self) -> Option<Rc2<TypeExpr>> {
        match self {
            ContractExpr::Unknown => None,
            ContractExpr::Known { contract, at } => contract.params.get(*at).cloned(),
            ContractExpr::Compose(f, g) => g
                .next_param()
                .or_else(|| f.next_param())
                .or_else(|| f.result_after()),
            ContractExpr::ApplyFirst(e) => {
                let advanced = ContractExpr::apply_first(e.clone());
                advanced.next_param()
            }

        }
    }

    /// The result type once all parameters are supplied.
    pub fn result_after(&self) -> Option<Rc2<TypeExpr>> {
        match self {
            ContractExpr::Unknown => None,
            ContractExpr::Known { contract, at } => {
                if *at >= contract.params.len() {
                    Some(contract.result.clone())
                } else {
                    None
                }
            }
            ContractExpr::Compose(f, g) => {
                if g.is_exhausted() {
                    f.result_after()
                } else {
                    None
                }
            }
            ContractExpr::ApplyFirst(_) => None,
        }
    }

    /// True when no parameters remain (value-level).
    pub fn is_exhausted(&self) -> bool {
        match self {
            ContractExpr::Unknown => false,
            ContractExpr::Known { contract, at } => *at >= contract.params.len(),
            ContractExpr::Compose(_, g) => g.is_exhausted(),
            ContractExpr::ApplyFirst(_) => false,
        }
    }

    /// Advance after one argument has been supplied and checked.
    pub fn apply_first_rc(self: Rc2<ContractExpr>) -> ContractExpr {
        // the common case advances without allocating anything
        match &*self {
            ContractExpr::Unknown => ContractExpr::Unknown,
            ContractExpr::Known { contract, at } => ContractExpr::Known {
                contract: contract.clone(),
                at: contract.params.len().min(at + 1),
            },
            _ => ContractExpr::apply_first(Box::new((*self).clone())),
        }
    }

    /// Advance after one argument has been supplied and checked.
    pub fn apply_first(self: Box<ContractExpr>) -> ContractExpr {
        match *self {
            ContractExpr::Unknown => ContractExpr::Unknown,
            ContractExpr::Known { contract, at } => {
                let at = contract.params.len().min(at + 1);
                ContractExpr::Known { contract, at }
            }
            ContractExpr::Compose(f, g) => {
                if !g.is_exhausted() {
                    ContractExpr::Compose(f, Box::new(ContractExpr::apply_first(g)))
                } else {
                    // x flowed into f
                    ContractExpr::ApplyFirst(f)
                }
            }
            ContractExpr::ApplyFirst(e) => ContractExpr::ApplyFirst(Box::new(ContractExpr::apply_first(e))),
        }
    }

    /// Number of remaining parameters, if known and not nested.
    pub fn params_len(&self) -> usize {
        match self {
            ContractExpr::Known { contract, at } => contract.params.len() - at,
            _ => 99,
        }
    }

    /// True if the next parameter is definitely a function type.
    pub fn next_param_is_function(&self, shapes: &Shapes) -> bool {
        match self.next_param() {
            Some(t) => matches!(shapes.unfold(&t), TypeExpr::Fun(_, _)),
            None => false,
        }
    }

    /// Check an argument to this function. The outer ApplyFirst layer (if
    /// any) accounts for the argument currently being consumed.
    pub fn check_arg(&self, shapes: &Shapes, fname: &str, v: &Value) -> Result<(), Crash2> {
        self.check_arg_at(shapes, fname, 0, v)
    }

    /// like check_arg, with the 0-based position of the argument for messages
    pub fn check_arg_at(
        &self,
        shapes: &Shapes,
        fname: &str,
        position: usize,
        v: &Value,
    ) -> Result<(), Crash2> {
        let peeled = match self {
            ContractExpr::ApplyFirst(e) => ContractExpr::apply_first(e.clone()),
            other => other.clone(),
        };
        if let Some(param) = peeled.next_param() {
            if let Err(msg) = check(shapes, &param, v) {
                // `check` reports "expected T, got V". Show the argument
                // position with the declared type, then the got-kind; when the
                // declared type is a type variable or alias whose inner
                // description adds information (e.g. "[a]" vs "a list"), the
                // inner "expected" is more helpful, so prefer it (§4.13).
                let (exp, got) = match msg.split_once(", got ") {
                    Some((e, g)) => (
                        e.strip_prefix("expected ").unwrap_or(e).to_string(),
                        g.to_string(),
                    ),
                    None => (type_name(&param), v.kind_name().to_string()),
                };
                let declared = type_name(&param);
                let shown = if declared == exp { declared } else { format!("{} ({})", declared, exp) };
                return Err(Crash2::new(format!(
                    "contract: {} expected {} as argument {}, got {}",
                    fname,
                    shown,
                    position + 1,
                    got
                )));
            }
        }
        Ok(())
    }

    /// Check a result. Only called when this expression is exhausted.
    pub fn check_result(&self, shapes: &Shapes, fname: &str, v: &Value) -> Result<(), Crash2> {
        if !self.is_exhausted() {
            // more parameters remain: the result must be a function; further
            // checking happens when it is applied
            if !matches!(v, Value::Fun(_)) {
                return Err(Crash2::new(format!(
                    "contract: {} expected a function, got {}",
                    fname,
                    v.kind_name()
                )));
            }
            return Ok(());
        }
        if let Some(res) = self.result_after() {
            if let Err(msg) = check(shapes, &res, v) {
                return Err(Crash2::new(format!(
                    "contract: {}: result {}",
                    fname, msg
                )));
            }
        }
        Ok(())
    }
}

pub type Crash2 = crate::value::Crash;
