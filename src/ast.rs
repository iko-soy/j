//! AST for the j language (§3.4).

use num_bigint::BigInt;
use std::rc::Rc;

#[derive(Debug, Clone, PartialEq)]
pub enum TypeExpr {
    Con(String),           // type name or variable
    List(Rc<TypeExpr>),
    Record(Vec<(String, Rc<TypeExpr>)>),
    Fun(Rc<TypeExpr>, Rc<TypeExpr>),
}

#[derive(Debug, Clone)]
pub enum Pattern {
    Var(String),
    Wildcard,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Var(String),
    TypeName(String),
    Int(BigInt),
    Text(String),
    IdLit(String), // unresolved prefix text; resolved to Expr::Id before eval
    Id(String),    // resolved full id
    NewId,
    LabelLit(String),
    PathLit(Vec<String>),
    Bool(bool),
    Lambda(Vec<Pattern>, Rc<Expr>, Source), // params, body, source text
    If(Rc<Expr>, Rc<Expr>, Rc<Expr>),
    Let(Vec<(String, Rc<Expr>)>, Rc<Expr>),
    App(Rc<Expr>, Rc<Expr>),
    BinOp(&'static str, Rc<Expr>, Rc<Expr>),
    Or(Rc<Expr>, Rc<Expr>),
    Select(Rc<Expr>, String),
    Update(Rc<Expr>, Vec<(String, Rc<Expr>)>),
    Record(Vec<(String, Rc<Expr>)>),
    List(Vec<Rc<Expr>>),
    SelectorFun(String),
    /// an explicitly parenthesised expression (for non-assoc chain checks)
    Paren(Rc<Expr>),
    Crash, // internal marker if needed
}

/// The source text of a lambda or section, exactly as written, which is how
/// its closure renders (§5.2): a span of the program text it was parsed
/// from. The lambdas of one program share that text instead of each holding
/// a copy, so nested lambdas cost a span each, not their text again.
#[derive(Clone)]
pub struct Source {
    text: Rc<str>,
    start: usize,
    end: usize,
}

impl Source {
    /// The bytes `start..end` of `text`, which must fall on char boundaries.
    pub fn new(text: Rc<str>, start: usize, end: usize) -> Self {
        Source { text, start, end }
    }

    pub fn as_str(&self) -> &str {
        &self.text[self.start..self.end]
    }
}

impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}

#[derive(Debug, Clone)]
pub enum Item {
    TypeDecl(String, TypeExpr),
    Signature(String, TypeExpr, usize), // name, type, line
    Definition(String, Rc<Expr>, usize),
}

impl Expr {
    /// Collect free variables of an expression (names not bound within it).
    pub fn free_vars(&self, bound: &mut Vec<String>, out: &mut std::collections::BTreeSet<String>) {
        match self {
            Expr::Var(n) => {
                if !bound.iter().any(|b| b == n) {
                    out.insert(n.clone());
                }
            }
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
            Expr::Paren(e) => e.free_vars(bound, out),
            Expr::Lambda(ps, body, _) => {
                let start = bound.len();
                for p in ps {
                    if let Pattern::Var(n) = p {
                        bound.push(n.clone());
                    }
                }
                body.free_vars(bound, out);
                bound.truncate(start);
            }
            Expr::If(a, b, c) => {
                a.free_vars(bound, out);
                b.free_vars(bound, out);
                c.free_vars(bound, out);
            }
            Expr::Let(bs, body) => {
                let start = bound.len();
                for (n, _) in bs {
                    bound.push(n.clone());
                }
                for (_, e) in bs {
                    e.free_vars(bound, out);
                }
                body.free_vars(bound, out);
                bound.truncate(start);
            }
            Expr::App(f, x) => {
                f.free_vars(bound, out);
                x.free_vars(bound, out);
            }
            Expr::BinOp(_, a, b) | Expr::Or(a, b) => {
                a.free_vars(bound, out);
                b.free_vars(bound, out);
            }
            Expr::Select(e, _) => e.free_vars(bound, out),
            Expr::Update(e, fs) => {
                e.free_vars(bound, out);
                for (_, v) in fs {
                    v.free_vars(bound, out);
                }
            }
            Expr::Record(fs) => {
                for (_, v) in fs {
                    v.free_vars(bound, out);
                }
            }
            Expr::List(es) => {
                for e in es {
                    e.free_vars(bound, out);
                }
            }
        }
    }
}
