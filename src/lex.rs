//! Lexer for the j language (§3.1).

use num_bigint::BigInt;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Ident(String),
    TypeName(String),
    Int(BigInt),
    Text(String),
    IdLit(String), // existing id, full or unique prefix
    NewId,         // bare `@`
    LabelLit(String),
    PathLit(Vec<String>),
    Selector(String), // `.name` with no whitespace
    Op(&'static str), // symbolic operator
    // keywords / punctuation
    Let,
    In,
    If,
    Then,
    Else,
    Or,
    True,
    False,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Semi,
    Colon,
    Backslash,
    Equals,
    Arrow,
    Underscore,
    Newline, // layout marker
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpTok {
    pub tok: Tok,
    pub line: usize, // 1-based
    pub col: usize,  // 1-based
}

#[derive(Debug)]
pub struct LexError {
    pub msg: String,
    pub line: usize,
}

const SYMBOLIC_OPS: &[&str] = &[
    "++", "::", "==", "/=", "<=", ">=", "&&", "||", ".", "*", "+", "-", "<", ">", 
];

fn is_ident_start(c: char) -> bool {
    c.is_ascii_lowercase() || c == '_'
}
fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '\''
}
fn is_type_start(c: char) -> bool {
    c.is_ascii_uppercase()
}
fn is_id_char(c: char) -> bool {
    ('k'..='z').contains(&c)
}

fn valid_label(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
        && !s.starts_with('.')
        && !s.starts_with('-')
        && !s.contains("..")
        && !s.ends_with('/')
        && !s.ends_with(".lock")
}

pub fn lex(src: &str) -> Result<Vec<SpTok>, LexError> {
    let mut toks: Vec<SpTok> = Vec::new();
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0usize;
    let mut line = 1usize;
    let mut col = 1usize; // column of next char

    macro_rules! err {
        ($msg:expr) => {
            return Err(LexError {
                msg: $msg.into(),
                line,
            })
        };
    }

    // push helper
    macro_rules! push {
        ($t:expr, $l:expr, $c:expr) => {
            toks.push(SpTok {
                tok: $t,
                line: $l,
                col: $c,
            })
        };
    }

    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            push!(Tok::Newline, line, col);
            i += 1;
            line += 1;
            col = 1;
            continue;
        }
        if c == ' ' || c == '\t' || c == '\r' {
            i += 1;
            col += 1;
            continue;
        }
        let start_line = line;
        let start_col = col;
        // comments
        if c == '-' && i + 1 < chars.len() && chars[i + 1] == '-' {
            // could be `--` comment, but `->`? no: `->` is dash-gt, distinct.
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
                col += 1;
            }
            continue;
        }
        if c == '{' && i + 1 < chars.len() && chars[i + 1] == '-' {
            let mut depth = 1usize;
            i += 2;
            col += 2;
            while i < chars.len() && depth > 0 {
                if chars[i] == '{' && i + 1 < chars.len() && chars[i + 1] == '-' {
                    depth += 1;
                    i += 2;
                    col += 2;
                } else if chars[i] == '-' && i + 1 < chars.len() && chars[i + 1] == '}' {
                    depth -= 1;
                    i += 2;
                    col += 2;
                } else if chars[i] == '\n' {
                    push!(Tok::Newline, line, col);
                    i += 1;
                    line += 1;
                    col = 1;
                } else {
                    i += 1;
                    col += 1;
                }
            }
            if depth > 0 {
                err!("unterminated block comment");
            }
            // a comment is whitespace (§3.1): one spanning lines has already
            // ended the line it started on, with the Newlines pushed above,
            // and one that does not must leave its line unbroken
            continue;
        }
        // arrow `->` (check before operator `-`)
        if c == '-' && i + 1 < chars.len() && chars[i + 1] == '>' {
            push!(Tok::Arrow, start_line, start_col);
            i += 2;
            col += 2;
            continue;
        }
        // path literal: `./` at start of token
        if c == '.' && i + 1 < chars.len() && chars[i + 1] == '/' {
            i += 2;
            col += 2;
            let mut comp = String::new();
            let mut comps: Vec<String> = Vec::new();
            while i < chars.len() {
                let d = chars[i];
                if d.is_whitespace() || matches!(d, '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';') {
                    break;
                }
                if d == '/' {
                    comps.push(std::mem::take(&mut comp));
                    i += 1;
                    col += 1;
                    // trailing slash: peek—if next is terminator, stop (drop empty comp)
                    if i >= chars.len()
                        || chars[i].is_whitespace()
                        || matches!(chars[i], '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';')
                    {
                        break;
                    }
                    continue;
                }
                comp.push(d);
                i += 1;
                col += 1;
            }
            if !comp.is_empty() {
                comps.push(comp);
            }
            push!(Tok::PathLit(comps), start_line, start_col);
            continue;
        }
        // selector: `.` immediately followed by identifier-start
        if c == '.' && i + 1 < chars.len() && is_ident_start(chars[i + 1]) {
            let mut name = String::new();
            i += 1;
            col += 1;
            while i < chars.len() && is_ident_char(chars[i]) {
                name.push(chars[i]);
                i += 1;
                col += 1;
            }
            push!(Tok::Selector(name), start_line, start_col);
            continue;
        }
        // `@` id literals
        if c == '@' {
            if i + 1 >= chars.len() || !is_ident_char(chars[i + 1]) {
                push!(Tok::NewId, start_line, start_col);
                i += 1;
                col += 1;
                continue;
            }
            if is_id_char(chars[i + 1]) {
                let mut s = String::new();
                i += 1;
                col += 1;
                while i < chars.len() && is_id_char(chars[i]) {
                    s.push(chars[i]);
                    i += 1;
                    col += 1;
                }
                // if the next char is still an identifier char, it's a lexical error
                if i < chars.len() && is_ident_char(chars[i]) {
                    err!("invalid id literal");
                }
                push!(Tok::IdLit(s), start_line, start_col);
                continue;
            }
            err!("`@` followed by an identifier character outside k-z is not a valid id literal");
        }
        // `%` label literals
        if c == '%' {
            let mut s = String::new();
            i += 1;
            col += 1;
            while i < chars.len() {
                let d = chars[i];
                if d.is_ascii_alphanumeric() || matches!(d, '.' | '_' | '/' | '-') {
                    s.push(d);
                    i += 1;
                    col += 1;
                } else {
                    break;
                }
            }
            if !valid_label(&s) {
                err!("invalid label literal");
            }
            push!(Tok::LabelLit(s), start_line, start_col);
            continue;
        }
        // text literal, double- or single-quoted
        if c == '"' || c == '\'' {
            let quote = c;
            i += 1;
            col += 1;
            let mut s = String::new();
            loop {
                if i >= chars.len() {
                    err!("unterminated text literal");
                }
                let d = chars[i];
                if d == quote {
                    i += 1;
                    col += 1;
                    break;
                }
                match d {
                    '\\' => {
                        if i + 1 >= chars.len() {
                            err!("unterminated text literal");
                        }
                        let e = chars[i + 1];
                        let r = match e {
                            '"' => '"',
                            '\'' => '\'',
                            '\\' => '\\',
                            'n' => '\n',
                            't' => '\t',
                            'r' => '\r',
                            _ => err!("unknown escape in text literal"),
                        };
                        s.push(r);
                        i += 2;
                        col += 2;
                    }
                    '\n' => err!("newline in text literal"),
                    _ => {
                        s.push(d);
                        i += 1;
                        col += 1;
                    }
                }
            }
            push!(Tok::Text(s), start_line, start_col);
            continue;
        }
        // integer
        if c.is_ascii_digit() {
            let mut s = String::new();
            while i < chars.len() && chars[i].is_ascii_digit() {
                s.push(chars[i]);
                i += 1;
                col += 1;
            }
            push!(Tok::Int(s.parse::<BigInt>().unwrap()), start_line, start_col);
            continue;
        }
        // identifier / keyword
        if is_ident_start(c) {
            let mut s = String::new();
            while i < chars.len() && is_ident_char(chars[i]) {
                s.push(chars[i]);
                i += 1;
                col += 1;
            }
            let t = match s.as_str() {
                "let" => Tok::Let,
                "in" => Tok::In,
                "if" => Tok::If,
                "then" => Tok::Then,
                "else" => Tok::Else,
                "or" => Tok::Or,
                "true" => Tok::True,
                "false" => Tok::False,
                "_" => {
                    if s == "_" {
                        Tok::Underscore
                    } else {
                        Tok::Ident(s)
                    }
                }
                _ => Tok::Ident(s),
            };
            push!(t, start_line, start_col);
            continue;
        }
        // type name
        if is_type_start(c) {
            let mut s = String::new();
            while i < chars.len() {
                let d = chars[i];
                if d.is_ascii_alphanumeric() || d == '_' {
                    s.push(d);
                    i += 1;
                    col += 1;
                } else {
                    break;
                }
            }
            push!(Tok::TypeName(s), start_line, start_col);
            continue;
        }
        // symbolic operators: longest match
        let rest: String = chars[i..].iter().take(2).collect();
        let mut matched: Option<&'static str> = None;
        for op in SYMBOLIC_OPS {
            if op.len() == 2 && rest == *op {
                matched = Some(op);
                break;
            }
        }
        if matched.is_none() {
            // compare by character: `rest` is built from chars, so slicing it
            // by byte would panic on any non-ASCII character reaching here
            for op in SYMBOLIC_OPS {
                if op.len() == 1 && op.starts_with(c) {
                    matched = Some(op);
                    break;
                }
            }
        }
        if let Some(op) = matched {
            push!(Tok::Op(op), start_line, start_col);
            i += op.len();
            col += op.len();
            continue;
        }
        // punctuation
        let t = match c {
            '(' => Tok::LParen,
            ')' => Tok::RParen,
            '[' => Tok::LBracket,
            ']' => Tok::RBracket,
            '{' => Tok::LBrace,
            '}' => Tok::RBrace,
            ',' => Tok::Comma,
            ';' => Tok::Semi,
            ':' => Tok::Colon,
            '\\' => Tok::Backslash,
            '=' => Tok::Equals,
            _ => err!(format!("unexpected character `{}`", c)),
        };
        push!(t, start_line, start_col);
        i += 1;
        col += 1;
    }
    push!(Tok::Eof, line, col);
    Ok(toks)
}

impl Tok {
    pub fn describe(&self) -> String {
        match self {
            Tok::Ident(s) => format!("`{}`", s),
            Tok::TypeName(s) => format!("`{}`", s),
            Tok::Int(n) => format!("`{}`", n),
            Tok::Text(_) => "a text literal".into(),
            Tok::IdLit(s) => format!("`@{}`", s),
            Tok::NewId => "`@`".into(),
            Tok::LabelLit(s) => format!("`%{}`", s),
            Tok::PathLit(_) => "a path literal".into(),
            Tok::Selector(s) => format!("`.{}`", s),
            Tok::Op(o) => format!("`{}`", o),
            other => format!("`{:?}`", other).to_lowercase(),
        }
    }
}
