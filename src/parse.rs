//! Parser for the j language (§3.2–3.4), including the two layout rules.

use crate::ast::{Expr, Item, Pattern, Source, TypeExpr};
use crate::lex::{lex, LexError, SpTok, Tok};
use num_bigint::BigInt;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

#[derive(Debug)]
pub struct ParseError {
    pub msg: String,
    pub line: usize,
}

impl ParseError {
    fn new(msg: impl Into<String>, line: usize) -> Self {
        ParseError {
            msg: msg.into(),
            line,
        }
    }
}

impl From<LexError> for ParseError {
    fn from(e: LexError) -> Self {
        ParseError {
            msg: e.msg,
            line: e.line,
        }
    }
}

pub struct Parser {
    /// the text being parsed, of which each lambda keeps its span (§5.2)
    src: Rc<str>,
    toks: Vec<SpTok>,
    pos: usize,
    /// Names in scope (top-level + builtins) for the no-shadowing rule;
    /// in config.j the builtins alone (§4.2).
    outer_names: Rc<BTreeSet<String>>,
    /// Names bound by the lambdas and `let` blocks around the current
    /// position, for the same rule (§4.2). Since shadowing is refused, no
    /// name is in here twice, and a construct removes the names it added
    /// when its scope ends.
    locals: HashSet<String>,
    /// How many local binders have been parsed, and for each name the
    /// ordinal and line of the latest one: the `let` check that `locals`
    /// cannot make (see `parse_let`).
    binders: usize,
    last_binder: HashMap<String, (usize, usize)>,
    /// set when an operator expression ends with a trailing operator before
    /// `)`, i.e. a left section
    section_op: Option<&'static str>,
    /// levels of recursive descent currently open, bounded by `MAX_DEPTH`
    depth: usize,
    /// how many levels high (see `MAX_DEPTH`) the expression the last parse
    /// function returned is, which each leaves here for its caller
    height: usize,
}

/// How many levels deep an expression may be (§3.4): a leaf is one level, and
/// any other node one more than its highest part, or two more for a
/// parenthesis (or section) and for the braces of a record or update. Deeper
/// input is a parse error (§1.4) instead of an overflow of the worker
/// thread's stack (main.rs). The recursive descent opens at most one level of
/// its own (`nested`) per level of the expression around the point it has
/// reached, each taking up to about 6 KB of stack in a debug build (a
/// parenthesis takes two) and 1.3 KB in a release build, so the deepest parse
/// takes under half of the worker's 512 MB. The passes over the tree after the
/// parse, and dropping it, recurse on its height at about 1.2 KB a level.
const MAX_DEPTH: usize = 40_000;

/// fixity table (§3.2): (precedence, right-assoc)
fn fixity(op: &str) -> Option<(u8, bool)> {
    Some(match op {
        "." => (9, true),
        "*" => (7, false),
        "+" | "-" => (6, false),
        "++" | "::" => (5, true),
        "==" | "/=" | "<" | "<=" | ">" | ">=" => (4, false), // non-assoc
        "&&" => (3, true),
        "||" => (2, true),
        _ => return None,
    })
}

/// non-associative operators cannot be chained
fn is_nonassoc(op: &str) -> bool {
    matches!(op, "==" | "/=" | "<" | "<=" | ">" | ">=")
}

impl Parser {
    pub fn new(src: &str, outer_names: Rc<BTreeSet<String>>) -> Result<Self, ParseError> {
        Ok(Parser {
            src: Rc::from(src),
            toks: lex(src)?,
            pos: 0,
            outer_names,
            locals: HashSet::new(),
            binders: 0,
            last_binder: HashMap::new(),
            section_op: None,
            depth: 0,
            height: 0,
        })
    }

    /// Run `f` one level deeper in the recursive descent. Every cycle of
    /// mutually recursive parse functions passes through a call made here,
    /// which is what bounds the parser's stack use. Each call is matched by a
    /// level of the expression around what `f` parses, so on input that
    /// parses, this bound is reached no sooner than `within`'s: it is what
    /// stops input that never closes, like `((((…`.
    fn nested<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, ParseError>,
    ) -> Result<T, ParseError> {
        if self.depth >= MAX_DEPTH {
            return Err(ParseError::new("nested too deeply", self.line()));
        }
        self.depth += 1;
        let r = f(self);
        self.depth -= 1;
        r
    }

    /// `h`, the height of an expression just built, while it is within
    /// `MAX_DEPTH`. Every node is checked here as it is built, since a loop
    /// builds a left-associative chain (`a + b + …`, `f x y …`, `r.a.b …`,
    /// `r { … } { … }`) as tall as it is long without recursing, and the
    /// passes over the tree after the parse, and dropping it, recurse on its
    /// height.
    fn within(&self, h: usize) -> Result<usize, ParseError> {
        if h > MAX_DEPTH {
            return Err(ParseError::new("nested too deeply", self.line()));
        }
        Ok(h)
    }

    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }
    fn line(&self) -> usize {
        self.toks[self.pos].line
    }
    fn col(&self) -> usize {
        self.toks[self.pos].col
    }
    fn bump(&mut self) -> Tok {
        let t = self.toks[self.pos].tok.clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }
    fn skip_newlines(&mut self) {
        while matches!(self.peek(), Tok::Newline) {
            self.bump();
        }
    }
    fn expect(&mut self, t: &Tok) -> Result<(), ParseError> {
        if self.peek() == t {
            self.bump();
            Ok(())
        } else {
            Err(ParseError::new(
                format!("expected {}, found {}", t.describe(), self.peek().describe()),
                self.line(),
            ))
        }
    }

    // ------------------------------------------------------------------
    // top-level: config := item*  (layout rule 1)
    // ------------------------------------------------------------------
    pub fn parse_config(&mut self) -> Result<Vec<Item>, ParseError> {
        let mut items = Vec::new();
        self.skip_newlines();
        while !matches!(self.peek(), Tok::Eof) {
            if self.col() != 1 {
                return Err(ParseError::new(
                    "top-level items must begin in column 1",
                    self.line(),
                ));
            }
            let item = self.parse_item()?;
            items.push(item);
            // after an item, expect newline(s) or eof
            self.skip_newlines();
        }
        Ok(items)
    }

    /// One item. Under rule 1 (§3.3) a line starting past column 1 continues
    /// it, so a line break before its `=` or `:` is skipped when the next
    /// line starts past column 1, like one inside its type or expression.
    fn parse_item(&mut self) -> Result<Item, ParseError> {
        match self.peek().clone() {
            Tok::TypeName(name) => {
                // typedecl: TYPENAME '=' type
                self.bump();
                self.skip_layout_newlines(1);
                self.expect(&Tok::Equals)?;
                let ty = self.parse_type()?;
                Ok(Item::TypeDecl(name, ty))
            }
            Tok::Ident(name) => {
                let line = self.line();
                self.bump();
                self.skip_layout_newlines(1);
                match self.peek().clone() {
                    Tok::Colon => {
                        self.bump();
                        let ty = self.parse_type()?;
                        Ok(Item::Signature(name, ty, line))
                    }
                    Tok::Equals => {
                        self.bump();
                        // inside nothing, so it opens no level (see `nested`)
                        let e = self.parse_expr_inner(1)?;
                        Ok(Item::Definition(name, Rc::new(e), line))
                    }
                    other => Err(ParseError::new(
                        format!("expected `:` or `=` after `{}`, found {}", name, other.describe()),
                        self.line(),
                    )),
                }
            }
            Tok::LParen => {
                // operator signature: (op) : type
                let line = self.line();
                self.bump();
                let op = match self.bump() {
                    Tok::Op(o) => o,
                    other => {
                        return Err(ParseError::new(
                            format!("expected operator, found {}", other.describe()),
                            self.line(),
                        ))
                    }
                };
                self.expect(&Tok::RParen)?;
                self.skip_layout_newlines(1);
                self.expect(&Tok::Colon)?;
                let ty = self.parse_type()?;
                Ok(Item::Signature(op.to_string(), ty, line))
            }
            other => Err(ParseError::new(
                format!("unexpected {} at top level", other.describe()),
                self.line(),
            )),
        }
    }

    // ------------------------------------------------------------------
    // types
    // ------------------------------------------------------------------
    fn parse_type(&mut self) -> Result<TypeExpr, ParseError> {
        self.nested(Self::parse_type_inner)
    }

    /// Types occur only in top-level items, so their layout limit is always
    /// rule 1's column 1 (§3.3): a line break may fall anywhere in a type,
    /// before or after `->` alike, when the next line starts past column 1.
    fn parse_type_inner(&mut self) -> Result<TypeExpr, ParseError> {
        self.skip_layout_newlines(1);
        let lhs = self.parse_atype()?;
        self.skip_layout_newlines(1);
        if matches!(self.peek(), Tok::Arrow) {
            self.bump();
            let rhs = self.parse_type()?;
            Ok(TypeExpr::Fun(Rc::new(lhs), Rc::new(rhs)))
        } else {
            Ok(lhs)
        }
    }

    fn parse_atype(&mut self) -> Result<TypeExpr, ParseError> {
        match self.peek().clone() {
            Tok::TypeName(n) => {
                self.bump();
                Ok(TypeExpr::Con(n))
            }
            Tok::Ident(n) => {
                self.bump();
                Ok(TypeExpr::Con(n))
            }
            Tok::LBracket => {
                self.bump();
                let t = self.parse_type()?;
                self.expect(&Tok::RBracket)?;
                Ok(TypeExpr::List(Rc::new(t)))
            }
            Tok::LBrace => {
                self.bump();
                let mut fields = Vec::new();
                loop {
                    self.skip_layout_newlines(1);
                    if matches!(self.peek(), Tok::RBrace) {
                        self.bump();
                        break;
                    }
                    let name = match self.bump() {
                        Tok::Ident(n) => n,
                        other => {
                            return Err(ParseError::new(
                                format!("expected field name, found {}", other.describe()),
                                self.line(),
                            ))
                        }
                    };
                    self.skip_layout_newlines(1);
                    self.expect(&Tok::Colon)?;
                    let t = self.parse_type()?;
                    fields.push((name, Rc::new(t)));
                    self.skip_layout_newlines(1);
                    match self.peek() {
                        Tok::Comma => {
                            self.bump();
                        }
                        Tok::RBrace => {
                            self.bump();
                            break;
                        }
                        other => {
                            return Err(ParseError::new(
                                format!("expected `,` or `}}`, found {}", other.describe()),
                                self.line(),
                            ))
                        }
                    }
                }
                Ok(TypeExpr::Record(fields))
            }
            Tok::LParen => {
                self.bump();
                let t = self.parse_type()?;
                self.expect(&Tok::RParen)?;
                Ok(t)
            }
            other => Err(ParseError::new(
                format!("expected a type, found {}", other.describe()),
                self.line(),
            )),
        }
    }

    // ------------------------------------------------------------------
    // expressions
    // ------------------------------------------------------------------

    /// Parse a full expression. `min_col` is the layout column limit: the
    /// expression stops at a Newline whose following token is in a column
    /// <= min_col (it belongs to an outer construct).
    pub fn parse_expr(&mut self, min_col: usize) -> Result<Expr, ParseError> {
        self.nested(|p| p.parse_expr_inner(min_col))
    }

    /// Each construct here and in `parse_atom_inner` is parsed by a function
    /// of its own: nested input stacks a frame of every function on its path
    /// for each level (see `MAX_DEPTH`), and in a debug build a function's
    /// frame holds the temporaries of all its arms.
    fn parse_expr_inner(&mut self, min_col: usize) -> Result<Expr, ParseError> {
        self.skip_layout_newlines(min_col);
        match self.peek().clone() {
            Tok::Backslash => self.parse_lambda(min_col),
            Tok::If => self.parse_if(min_col),
            Tok::Let => self.parse_let(min_col),
            _ => self.parse_or(min_col),
        }
    }

    fn parse_lambda(&mut self, min_col: usize) -> Result<Expr, ParseError> {
        let src_start = self.toks[self.pos].clone();
        self.bump();
        let mut params = Vec::new();
        loop {
            match self.peek().clone() {
                Tok::Ident(n) => {
                    // in scope from here on, so a later parameter
                    // may not reuse it either
                    self.check_shadow(&n)?;
                    self.locals.insert(n.clone());
                    self.note_binder(&n, self.line());
                    params.push(Pattern::Var(n));
                    self.bump();
                }
                Tok::Underscore => {
                    params.push(Pattern::Wildcard);
                    self.bump();
                }
                Tok::Arrow => {
                    self.bump();
                    break;
                }
                other => {
                    return Err(ParseError::new(
                        format!(
                            "expected a parameter or `->`, found {}",
                            other.describe()
                        ),
                        self.line(),
                    ))
                }
            }
        }
        if params.is_empty() {
            return Err(ParseError::new("lambda needs at least one parameter", self.line()));
        }
        let body = self.parse_expr(min_col)?;
        self.height = self.within(self.height + 1)?;
        for p in &params {
            if let Pattern::Var(n) = p {
                self.locals.remove(n);
            }
        }
        let src = self.source_slice(&src_start);
        Ok(Expr::Lambda(params, Rc::new(body), src))
    }

    fn parse_if(&mut self, min_col: usize) -> Result<Expr, ParseError> {
        self.bump();
        let c = self.parse_expr(min_col)?;
        let mut h = self.height;
        self.skip_layout_newlines(min_col);
        self.expect(&Tok::Then)?;
        let t = self.parse_expr(min_col)?;
        h = h.max(self.height);
        self.skip_layout_newlines(min_col);
        self.expect(&Tok::Else)?;
        let e = self.parse_expr(min_col)?;
        self.height = self.within(h.max(self.height) + 1)?;
        Ok(Expr::If(Rc::new(c), Rc::new(t), Rc::new(e)))
    }

    /// skip Newlines whose next real token is in a column > min_col
    fn skip_layout_newlines(&mut self, min_col: usize) {
        loop {
            if !matches!(self.peek(), Tok::Newline) {
                return;
            }
            // lookahead past consecutive newlines
            let mut p = self.pos;
            while matches!(self.toks[p].tok, Tok::Newline) {
                p += 1;
            }
            if matches!(self.toks[p].tok, Tok::Eof) {
                return;
            }
            if self.toks[p].col > min_col {
                self.pos = p;
            } else {
                return;
            }
        }
    }

    fn parse_let(&mut self, min_col: usize) -> Result<Expr, ParseError> {
        self.bump(); // let
        // skip newlines to find the block column
        let mut p = self.pos;
        while matches!(self.toks[p].tok, Tok::Newline) {
            p += 1;
        }
        let block_col = self.toks[p].col;
        self.pos = p;
        if block_col <= min_col {
            return Err(ParseError::new(
                "let bindings must be indented",
                self.line(),
            ));
        }
        let mut bindings: Vec<(String, Rc<Expr>)> = Vec::new();
        let mut names: Vec<(String, usize)> = Vec::new();
        let first_binder = self.binders;
        let mut h = 0;
        loop {
            // binding: ident '=' expr, expr limited to block_col
            let name = match self.peek().clone() {
                Tok::Ident(n) => n,
                other => {
                    return Err(ParseError::new(
                        format!("expected a binding name, found {}", other.describe()),
                        self.line(),
                    ))
                }
            };
            if names.iter().any(|(n, _)| *n == name) {
                return Err(ParseError::new(
                    format!("`{}` is bound twice in this let", name),
                    self.line(),
                ));
            }
            self.check_shadow(&name)?;
            // in scope from here on: in its own expression, the later ones
            // and the body (§4.1)
            self.locals.insert(name.clone());
            names.push((name.clone(), self.line()));
            self.bump();
            self.expect(&Tok::Equals)?;
            let e = self.parse_expr(block_col)?;
            h = h.max(self.height);
            bindings.push((name, Rc::new(e)));
            // separator: `;` or newline at exactly block_col, then `in` ends
            match self.peek().clone() {
                Tok::Semi => {
                    self.bump();
                    self.skip_layout_newlines(block_col - 1); // any continuation
                    if matches!(self.peek(), Tok::In) {
                        self.bump();
                        break;
                    }
                    continue;
                }
                Tok::In => {
                    self.bump();
                    break;
                }
                Tok::Newline => {
                    // find next real token
                    let mut q = self.pos;
                    while matches!(self.toks[q].tok, Tok::Newline) {
                        q += 1;
                    }
                    let next_col = self.toks[q].col;
                    if matches!(self.toks[q].tok, Tok::In) {
                        // like any token that begins a line, an `in` at or
                        // left of the limit belongs to an outer construct
                        // (§3.3): a new top-level item or an outer let's
                        // binding line, never this let
                        if next_col <= min_col {
                            return Err(ParseError::new(
                                format!("`in` must be indented past column {}", min_col),
                                self.toks[q].line,
                            ));
                        }
                        self.pos = q;
                        self.bump();
                        break;
                    }
                    if next_col == block_col {
                        self.pos = q;
                        continue;
                    }
                    if next_col < block_col {
                        // block ends without `in`? `in` must appear on an
                        // outer line; treat as error unless next is In handled above
                        if matches!(self.toks[q].tok, Tok::In) {
                            self.pos = q;
                            self.bump();
                            break;
                        }
                        // The `in` could be at a smaller column: look for it
                        return Err(ParseError::new(
                            "expected `in` or another binding",
                            self.toks[q].line,
                        ));
                    }
                    // deeper indentation continues the expression; but the
                    // expression parser should have consumed it. Error.
                    return Err(ParseError::new(
                        "unexpected indentation in let block",
                        self.toks[q].line,
                    ));
                }
                other => {
                    return Err(ParseError::new(
                        format!("expected `;`, newline, or `in`, found {}", other.describe()),
                        self.line(),
                    ))
                }
            }
        }
        // A binding is in scope in the expressions before it too (§4.1), but
        // it was not yet in `locals` while they were parsed: a binder in one
        // of them that reuses its name is caught here instead.
        for (n, _) in &names {
            if let Some(&(k, line)) = self.last_binder.get(n) {
                if k >= first_binder {
                    return Err(shadowed(n, line));
                }
            }
        }
        for (n, line) in &names {
            self.note_binder(n, *line);
        }
        let body = self.parse_expr(min_col)?;
        self.height = self.within(h.max(self.height) + 1)?;
        for (n, _) in &names {
            self.locals.remove(n);
        }
        Ok(Expr::Let(bindings, Rc::new(body)))
    }

    /// §4.2: a binder may not reuse a top-level or builtin name, nor one
    /// bound by an enclosing lambda or `let`.
    fn check_shadow(&self, name: &str) -> Result<(), ParseError> {
        if self.outer_names.contains(name) || self.locals.contains(name) {
            return Err(shadowed(name, self.line()));
        }
        Ok(())
    }

    /// Record a local binder for the check at the end of each enclosing
    /// `let` block's bindings.
    fn note_binder(&mut self, name: &str, line: usize) {
        self.last_binder.insert(name.to_string(), (self.binders, line));
        self.binders += 1;
    }

    fn parse_or(&mut self, min_col: usize) -> Result<Expr, ParseError> {
        // `or` is infixl 0 and extends as far right as possible
        let mut lhs = self.parse_opexpr(1, min_col)?;
        let mut h = self.height;
        loop {
            self.skip_layout_newlines(min_col);
            if matches!(self.peek(), Tok::Or) {
                self.bump();
                let rhs = self.nested(|p| p.parse_or(min_col))?;
                h = self.within(h.max(self.height) + 1)?;
                lhs = Expr::Or(Rc::new(lhs), Rc::new(rhs));
            } else {
                break;
            }
        }
        self.height = h;
        Ok(lhs)
    }

    /// precedence-climbing over the fixity table
    fn parse_opexpr(&mut self, min_prec: u8, min_col: usize) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_app(min_col)?;
        let mut h = self.height;
        loop {
            self.skip_op_newline(min_col);
            let op: &'static str = match self.peek() {
                Tok::Op(o) => o,
                _ => break,
            };
            let (prec, right) = match fixity(op) {
                Some(f) => f,
                None => break,
            };
            if prec < min_prec {
                break;
            }
            self.bump();
            // left section: `(e op)` — a trailing operator before `)`
            if matches!(self.peek(), Tok::RParen) {
                self.section_op = Some(op);
                self.height = h;
                return Ok(lhs);
            }
            let next_min = if right { prec } else { prec + 1 };
            let rhs = self.nested(|p| p.parse_opexpr(next_min, min_col))?;
            // non-assoc check: if lhs is the same operator, reject chaining
            if is_nonassoc(op) {
                // a non-associative operator cannot be chained; explicit
                // parentheses break the chain
                if let Expr::BinOp(o2, _, _) = &lhs {
                    if is_nonassoc(o2) {
                        return Err(ParseError::new(
                            format!("`{}` is non-associative and cannot be chained", op),
                            self.line(),
                        ));
                    }
                }
                if let Expr::BinOp(o2, _, _) = &rhs {
                    if is_nonassoc(o2) && *o2 == op {
                        return Err(ParseError::new(
                            format!("`{}` is non-associative and cannot be chained", op),
                            self.line(),
                        ));
                    }
                }
            }
            h = self.within(h.max(self.height) + 1)?;
            lhs = Expr::BinOp(op, Rc::new(lhs), Rc::new(rhs));
        }
        self.height = h;
        Ok(lhs)
    }

    /// allow an operator at the start of a continuation line (column > min_col)
    fn skip_op_newline(&mut self, min_col: usize) {
        if !matches!(self.peek(), Tok::Newline) {
            return;
        }
        let mut p = self.pos;
        while matches!(self.toks[p].tok, Tok::Newline) {
            p += 1;
        }
        if matches!(self.toks[p].tok, Tok::Op(_)) && self.toks[p].col > min_col {
            self.pos = p;
        }
    }

    fn starts_atom(t: &Tok) -> bool {
        matches!(
            t,
            Tok::Ident(_)
                | Tok::TypeName(_)
                | Tok::Int(_)
                | Tok::Text(_)
                | Tok::IdLit(_)
                | Tok::NewId
                | Tok::LabelLit(_)
                | Tok::PathLit(_)
                | Tok::True
                | Tok::False
                | Tok::LParen
                | Tok::LBracket
                | Tok::LBrace
                | Tok::Selector(_)
        )
    }

    fn parse_app(&mut self, min_col: usize) -> Result<Expr, ParseError> {
        let mut e = self.parse_postfix(min_col)?;
        let mut h = self.height;
        loop {
            // application across a newline requires deeper indentation
            if matches!(self.peek(), Tok::Newline) {
                let mut p = self.pos;
                while matches!(self.toks[p].tok, Tok::Newline) {
                    p += 1;
                }
                if Self::starts_atom(&self.toks[p].tok) && self.toks[p].col > min_col {
                    self.pos = p;
                } else {
                    break;
                }
            }
            if Self::starts_atom(self.peek()) {
                let arg = self.parse_postfix(min_col)?;
                h = self.within(h.max(self.height) + 1)?;
                e = Expr::App(Rc::new(e), Rc::new(arg));
            } else {
                break;
            }
        }
        self.height = h;
        Ok(e)
    }

    fn parse_postfix(&mut self, min_col: usize) -> Result<Expr, ParseError> {
        let mut e = self.parse_atom(min_col)?;
        let mut h = self.height;
        loop {
            match self.peek().clone() {
                Tok::Selector(f) => {
                    self.bump();
                    h = self.within(h + 1)?;
                    e = Expr::Select(Rc::new(e), f);
                }
                Tok::LBrace => {
                    // record update (or a record literal argument? no —
                    // postfix binds tighter than application, so `{` after an
                    // atom is an update)
                    self.bump();
                    // two levels, like a record literal's braces, so a second
                    // level of the descent to match (see `MAX_DEPTH`)
                    let fields = self.nested(|p| p.parse_fields(min_col, true))?;
                    h = self.within(h.max(self.height) + 2)?;
                    e = Expr::Update(Rc::new(e), fields);
                }
                // a Newline ends the chain too: a selector or `{` that
                // begins a line is the next postfix, not this one's (§3.3)
                _ => break,
            }
        }
        self.height = h;
        Ok(e)
    }

    /// parse `ident = expr, ...` inside braces. `update` controls the error message.
    fn parse_fields(
        &mut self,
        min_col: usize,
        _update: bool,
    ) -> Result<Vec<(String, Rc<Expr>)>, ParseError> {
        let mut fields: Vec<(String, Rc<Expr>)> = Vec::new();
        self.skip_layout_newlines(min_col);
        if matches!(self.peek(), Tok::RBrace) {
            self.bump();
            // nothing inside the braces
            self.height = 0;
            return Ok(fields);
        }
        // the height of the highest field
        let mut h = 0;
        loop {
            let name = match self.bump() {
                Tok::Ident(n) => n,
                other => {
                    return Err(ParseError::new(
                        format!("expected a field name, found {}", other.describe()),
                        self.line(),
                    ))
                }
            };
            if fields.iter().any(|(n, _)| *n == name) {
                return Err(ParseError::new(
                    format!("duplicate field `{}`", name),
                    self.line(),
                ));
            }
            self.expect(&Tok::Equals)?;
            let v = self.parse_expr(min_col)?;
            h = h.max(self.height);
            fields.push((name, Rc::new(v)));
            self.skip_layout_newlines(min_col);
            match self.peek() {
                Tok::Comma => {
                    self.bump();
                    self.skip_layout_newlines(min_col);
                }
                Tok::RBrace => {
                    self.bump();
                    break;
                }
                other => {
                    return Err(ParseError::new(
                        format!("expected `,` or `}}`, found {}", other.describe()),
                        self.line(),
                    ))
                }
            }
        }
        self.height = h;
        Ok(fields)
    }

    fn parse_atom(&mut self, min_col: usize) -> Result<Expr, ParseError> {
        self.nested(|p| p.parse_atom_inner(min_col))
    }

    fn parse_atom_inner(&mut self, min_col: usize) -> Result<Expr, ParseError> {
        // one level, unless an arm below builds more around what it parses
        self.height = 1;
        match self.peek().clone() {
            Tok::Ident(n) => {
                self.bump();
                Ok(Expr::Var(n))
            }
            Tok::TypeName(n) => {
                self.bump();
                Ok(Expr::TypeName(n))
            }
            Tok::Int(n) => {
                self.bump();
                Ok(Expr::Int(n))
            }
            Tok::Text(s) => {
                self.bump();
                Ok(Expr::Text(s))
            }
            Tok::IdLit(s) => {
                self.bump();
                Ok(Expr::IdLit(s))
            }
            Tok::NewId => {
                self.bump();
                Ok(Expr::NewId)
            }
            Tok::LabelLit(s) => {
                self.bump();
                Ok(Expr::LabelLit(s))
            }
            Tok::PathLit(p) => {
                self.bump();
                Ok(Expr::PathLit(p))
            }
            Tok::True => {
                self.bump();
                Ok(Expr::Bool(true))
            }
            Tok::False => {
                self.bump();
                Ok(Expr::Bool(false))
            }
            Tok::Selector(f) => {
                self.bump();
                Ok(Expr::SelectorFun(f))
            }
            Tok::LBracket => self.parse_list(min_col),
            Tok::LBrace => {
                self.bump();
                let fields = self.parse_fields(min_col, false)?;
                // the braces are two levels, as they open two of the
                // descent: this atom and the field (see `MAX_DEPTH`)
                self.height = self.within(self.height + 2)?;
                Ok(Expr::Record(fields))
            }
            Tok::LParen => self.parse_paren(min_col),
            other => Err(ParseError::new(
                format!("unexpected {}", other.describe()),
                self.line(),
            )),
        }
    }

    fn parse_list(&mut self, min_col: usize) -> Result<Expr, ParseError> {
        self.bump();
        let mut elems = Vec::new();
        let mut h = 0;
        loop {
            self.skip_layout_newlines(min_col);
            if matches!(self.peek(), Tok::RBracket) {
                self.bump();
                break;
            }
            let e = self.parse_postfix(min_col)?;
            h = h.max(self.height);
            elems.push(Rc::new(e));
        }
        self.height = self.within(h + 1)?;
        Ok(Expr::List(elems))
    }

    fn parse_paren(&mut self, min_col: usize) -> Result<Expr, ParseError> {
        let open = self.toks[self.pos].clone();
        self.bump();
        // (op) | (op expr) | (expr op) | (expr)
        if let Tok::Op(o) = self.peek().clone() {
            self.bump();
            self.skip_layout_newlines(min_col);
            if matches!(self.peek(), Tok::RParen) {
                self.bump();
                return Ok(Expr::Var(o.to_string()));
            }
            // right section: (op e) = \x -> x op e
            let e = self.parse_expr(min_col)?;
            // `(op e op)` is no production (§3.4); unchecked, the
            // trailing operator would be dropped here and left set
            // for the next `(e)` to take as a left section
            if let Some(o2) = self.section_op.take() {
                return Err(ParseError::new(
                    format!(
                        "a section cannot have operators on both sides (`{}` and `{}`)",
                        o, o2
                    ),
                    self.line(),
                ));
            }
            self.expect(&Tok::RParen)?;
            // a lambda around an operator: two levels
            self.height = self.within(self.height + 2)?;
            let var = fresh_var();
            let body = Expr::BinOp(
                o,
                Rc::new(Expr::Var(var.clone())),
                Rc::new(e),
            );
            return Ok(Expr::Lambda(
                vec![Pattern::Var(var)],
                Rc::new(body),
                self.source_slice(&open),
            ));
        }
        let first = self.parse_expr(min_col)?;
        // a parenthesis is two levels, as is a section's lambda
        // around its operator
        self.height = self.within(self.height + 2)?;
        self.skip_layout_newlines(min_col);
        if let Some(o) = self.section_op.take() {
            // left section: (e op) = \x -> e op x
            self.expect(&Tok::RParen)?;
            let var = fresh_var();
            let body = Expr::BinOp(
                o,
                Rc::new(first),
                Rc::new(Expr::Var(var.clone())),
            );
            return Ok(Expr::Lambda(
                vec![Pattern::Var(var)],
                Rc::new(body),
                self.source_slice(&open),
            ));
        }
        self.expect(&Tok::RParen)?;
        Ok(Expr::Paren(Rc::new(first)))
    }

    /// The source of a lambda or section, from its first token `start` to
    /// the last token consumed, for its rendering (§5.2: exactly as
    /// written). Line breaks the layout rules skipped looking for more of it
    /// are not part of it, nor is a comment after its last token.
    fn source_slice(&self, start: &SpTok) -> Source {
        let mut last = self.pos - 1;
        while matches!(self.toks[last].tok, Tok::Newline) {
            last -= 1;
        }
        Source::new(self.src.clone(), start.start, self.toks[last].end)
    }

    /// Parse a single command-line expression. Layout rule 1 does not apply.
    pub fn parse_cli_expr(&mut self) -> Result<Expr, ParseError> {
        self.skip_newlines();
        // inside nothing, so it opens no level (see `nested`)
        let e = self.parse_expr_inner(0)?;
        self.skip_newlines();
        if !matches!(self.peek(), Tok::Eof) {
            return Err(ParseError::new(
                format!("unexpected {} after the expression", self.peek().describe()),
                self.line(),
            ));
        }
        Ok(e)
    }
}

fn shadowed(name: &str, line: usize) -> ParseError {
    ParseError::new(format!("`{}` is already bound and may not be shadowed", name), line)
}

use std::sync::atomic::{AtomicUsize, Ordering};
static FRESH: AtomicUsize = AtomicUsize::new(0);
fn fresh_var() -> String {
    format!("$sec{}", FRESH.fetch_add(1, Ordering::Relaxed))
}

pub fn parse_config(src: &str, outer_names: Rc<BTreeSet<String>>) -> Result<Vec<Item>, ParseError> {
    let mut p = Parser::new(src, outer_names)?;
    p.parse_config()
}

pub fn parse_expr(src: &str, outer_names: Rc<BTreeSet<String>>) -> Result<Expr, ParseError> {
    let mut p = Parser::new(src, outer_names)?;
    p.parse_cli_expr()
}

/// Re-render a lambda as source text (used for display of closures).
pub fn render_lambda(params: &[Pattern], body: &Expr) -> String {
    let ps: Vec<String> = params
        .iter()
        .map(|p| match p {
            Pattern::Var(n) => n.clone(),
            Pattern::Wildcard => "_".into(),
        })
        .collect();
    format!("\\{} -> {}", ps.join(" "), render_expr(body))
}

pub fn render_expr(e: &Expr) -> String {
    match e {
        Expr::Var(n) => n.clone(),
        Expr::TypeName(n) => n.clone(),
        Expr::Int(n) => n.to_string(),
        Expr::Text(s) => format!("{:?}", s),
        Expr::IdLit(s) => format!("@{}", s),
        Expr::Id(s) => format!("@{}", s),
        Expr::NewId => "@".into(),
        Expr::LabelLit(s) => format!("%{}", s),
        Expr::PathLit(p) => {
            if p.is_empty() {
                "./".into()
            } else {
                format!("./{}", p.join("/"))
            }
        }
        Expr::Bool(b) => b.to_string(),
        Expr::Lambda(ps, b, _) => render_lambda(ps, b),
        Expr::If(c, t, f) => format!(
            "if {} then {} else {}",
            render_expr(c),
            render_expr(t),
            render_expr(f)
        ),
        Expr::Let(bs, b) => {
            let parts: Vec<String> = bs
                .iter()
                .map(|(n, e)| format!("{} = {}", n, render_expr(e)))
                .collect();
            format!("let {} in {}", parts.join("; "), render_expr(b))
        }
        Expr::App(f, x) => {
            let fs = match f.as_ref() {
                Expr::App(_, _) => format!("({})", render_expr(f)),
                Expr::Paren(_) => render_expr(f),
                _ => render_expr(f),
            };
            format!("{} ({})", fs, render_expr(x))
        }
        Expr::BinOp(o, a, b) => format!("({} {} {})", render_expr(a), o, render_expr(b)),
        Expr::Or(a, b) => format!("({} or {})", render_expr(a), render_expr(b)),
        Expr::Select(e, f) => format!("{}.{}", render_expr(e), f),
        Expr::Update(e, fs) => {
            let parts: Vec<String> = fs
                .iter()
                .map(|(n, v)| format!("{} = {}", n, render_expr(v)))
                .collect();
            format!("{} {{ {} }}", render_expr(e), parts.join(", "))
        }
        Expr::Record(fs) => {
            let parts: Vec<String> = fs
                .iter()
                .map(|(n, v)| format!("{} = {}", n, render_expr(v)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
        Expr::List(es) => {
            let parts: Vec<String> = es.iter().map(|e| render_expr(e)).collect();
            format!("[{}]", parts.join(" "))
        }
        Expr::SelectorFun(f) => format!(".{}", f),
        Expr::Paren(e) => render_expr(e),
        Expr::Crash => "crash".into(),
    }
}

/// helper used by tests
pub fn int(n: i64) -> Expr {
    Expr::Int(BigInt::from(n))
}
