//! Lexer tests (§3.1).

use j::lex::{lex, Tok};

fn toks(src: &str) -> Vec<Tok> {
    lex(src)
        .unwrap()
        .into_iter()
        .map(|t| t.tok)
        .filter(|t| !matches!(t, Tok::Newline | Tok::Eof))
        .collect()
}

fn err(src: &str) -> String {
    match lex(src) {
        Ok(_) => panic!("expected a lex error for {:?}", src),
        Err(e) => e.msg,
    }
}

#[test]
fn identifiers_and_keywords() {
    assert_eq!(
        toks("foo bar' _x x1"),
        vec![
            Tok::Ident("foo".into()),
            Tok::Ident("bar'".into()),
            Tok::Ident("_x".into()),
            Tok::Ident("x1".into()),
        ]
    );
    assert_eq!(toks("let in if then else or true false"), vec![
        Tok::Let, Tok::In, Tok::If, Tok::Then, Tok::Else, Tok::Or, Tok::True, Tok::False,
    ]);
}

#[test]
fn type_names() {
    assert_eq!(
        toks("Commit Edit A1 B_c"),
        vec![
            Tok::TypeName("Commit".into()),
            Tok::TypeName("Edit".into()),
            Tok::TypeName("A1".into()),
            Tok::TypeName("B_c".into()),
        ]
    );
}

#[test]
fn integers() {
    assert_eq!(
        toks("0 42 999999999999999999999999999999"),
        vec![
            Tok::Int(0.into()),
            Tok::Int(42.into()),
            Tok::Int("999999999999999999999999999999".parse().unwrap()),
        ]
    );
}

#[test]
fn text_literals_and_escapes() {
    assert_eq!(toks(r#""hello""#), vec![Tok::Text("hello".into())]);
    assert_eq!(
        toks(r#""a\"b\\c\nd\te\rf""#),
        vec![Tok::Text("a\"b\\c\nd\te\rf".into())]
    );
    assert!(err(r#""unterminated"#).contains("unterminated"));
    assert!(err("\"a\nb\"").contains("newline"));
    assert!(err(r#""\x""#).contains("escape"));
}

#[test]
fn single_quoted_text_literals() {
    // single quotes are an alternative delimiter with the same escapes
    assert_eq!(toks("'hello'"), vec![Tok::Text("hello".into())]);
    // an unescaped " is fine inside single quotes
    assert_eq!(toks(r#"'say "hi"'"#), vec![Tok::Text("say \"hi\"".into())]);
    // an unescaped ' is fine inside double quotes
    assert_eq!(toks("\"don't\""), vec![Tok::Text("don't".into())]);
    // \' escapes the single quote
    assert_eq!(toks(r"'don\'t'"), vec![Tok::Text("don't".into())]);
    // \" works in single-quoted literals too
    assert_eq!(toks(r#"'a\"b'"#), vec![Tok::Text("a\"b".into())]);
    // other escapes work the same
    assert_eq!(toks(r"'\n\t\r\\'"), vec![Tok::Text("\n\t\r\\".into())]);
    // empty
    assert_eq!(toks("''"), vec![Tok::Text("".into())]);
    // errors
    assert!(err("'unterminated").contains("unterminated"));
    assert!(err("'a\nb'").contains("newline"));
    assert!(err(r"'\x'").contains("escape"));
    // adjacent strings are two tokens (application), not one string
    assert_eq!(
        toks("'it' 's'"),
        vec![Tok::Text("it".into()), Tok::Text("s".into())]
    );
}

#[test]
fn id_literals() {
    assert_eq!(toks("@wqzt"), vec![Tok::IdLit("wqzt".into())]);
    assert_eq!(toks("@kpqxmnrv"), vec![Tok::IdLit("kpqxmnrv".into())]);
    // bare @ followed by non-ident char is NEWID
    assert_eq!(toks("@"), vec![Tok::NewId]);
    assert_eq!(toks("@)"), vec![Tok::NewId, Tok::RParen]);
    assert_eq!(toks("@ "), vec![Tok::NewId]);
    // @ followed by ident char outside k-z is a lexical error
    assert!(err("@main").contains("id literal"));
    assert!(err("@abcd").contains("id literal"));
    assert!(err("@j").contains("id literal"));
    // id literal stopped at non-id ident char
    assert!(err("@kqA").contains("invalid id literal"));
}

#[test]
fn label_literals() {
    assert_eq!(toks("%main"), vec![Tok::LabelLit("main".into())]);
    assert_eq!(
        toks("%feature/x-y_z.1"),
        vec![Tok::LabelLit("feature/x-y_z.1".into())]
    );
    assert!(err("%-bad").contains("invalid label"));
    assert!(err("%.bad").contains("invalid label"));
    assert!(err("%a..b").contains("invalid label"));
    assert!(err("%a/").contains("invalid label"));
    assert!(err("%a.lock").contains("invalid label"));
    assert!(err("%").contains("invalid label"));
}

#[test]
fn path_literals() {
    assert_eq!(
        toks("./src/lexer.rs"),
        vec![Tok::PathLit(vec!["src".into(), "lexer.rs".into()])]
    );
    assert_eq!(toks("./"), vec![Tok::PathLit(vec![])]);
    // trailing slash ignored
    assert_eq!(toks("./a/b/"), vec![Tok::PathLit(vec!["a".into(), "b".into()])]);
    // stops at punctuation
    assert_eq!(
        toks("./a,)"),
        vec![Tok::PathLit(vec!["a".into()]), Tok::Comma, Tok::RParen]
    );
    assert_eq!(
        toks("./a ./b"),
        vec![Tok::PathLit(vec!["a".into()]), Tok::PathLit(vec!["b".into()])]
    );
}

#[test]
fn selectors_and_dot() {
    assert_eq!(toks(".foo"), vec![Tok::Selector("foo".into())]);
    // `.` followed by whitespace or non-ident is the operator
    assert_eq!(toks(". "), vec![Tok::Op(".")]);
    assert_eq!(
        toks("f . g"),
        vec![Tok::Ident("f".into()), Tok::Op("."), Tok::Ident("g".into())]
    );
    // `.` followed by digit is operator (path rule doesn't apply without /)
    assert_eq!(
        toks("a.2"),
        vec![Tok::Ident("a".into()), Tok::Op("."), Tok::Int(2.into())]
    );
}

#[test]
fn operators_longest_match() {
    assert_eq!(
        toks("++ :: == /= <= >= && || . * + - < >"),
        vec![
            Tok::Op("++"),
            Tok::Op("::"),
            Tok::Op("=="),
            Tok::Op("/="),
            Tok::Op("<="),
            Tok::Op(">="),
            Tok::Op("&&"),
            Tok::Op("||"),
            Tok::Op("."),
            Tok::Op("*"),
            Tok::Op("+"),
            Tok::Op("-"),
            Tok::Op("<"),
            Tok::Op(">"),
        ]
    );
    // `->` is punctuation, not minus-gt
    assert_eq!(toks("a -> b"), vec![Tok::Ident("a".into()), Tok::Arrow, Tok::Ident("b".into())]);
    // `-` alone is the operator
    assert_eq!(toks("a - b"), vec![Tok::Ident("a".into()), Tok::Op("-"), Tok::Ident("b".into())]);
}

#[test]
fn punctuation() {
    assert_eq!(
        toks("( ) [ ] { } , ; : \\ = _"),
        vec![
            Tok::LParen,
            Tok::RParen,
            Tok::LBracket,
            Tok::RBracket,
            Tok::LBrace,
            Tok::RBrace,
            Tok::Comma,
            Tok::Semi,
            Tok::Colon,
            Tok::Backslash,
            Tok::Equals,
            Tok::Underscore,
        ]
    );
}

#[test]
fn line_comments() {
    assert_eq!(toks("a -- comment\nb"), vec![Tok::Ident("a".into()), Tok::Ident("b".into())]);
    assert_eq!(toks("-- only a comment"), Vec::<Tok>::new());
    // dashes inside expressions still work
    assert_eq!(
        toks("a - b"),
        vec![Tok::Ident("a".into()), Tok::Op("-"), Tok::Ident("b".into())]
    );
}

#[test]
fn block_comments_nest() {
    assert_eq!(
        toks("a {- x {- y -} z -} b"),
        vec![Tok::Ident("a".into()), Tok::Ident("b".into())]
    );
    assert!(err("{- unterminated").contains("unterminated"));
    // the whole nested form is one comment
    assert_eq!(toks("{- a {- b -} c -}"), Vec::<Tok>::new());
    // without nesting, the first `-}` would end it and `c -}` would remain
    assert_eq!(toks("{- a -} c"), vec![Tok::Ident("c".into())]);
}

#[test]
fn block_comment_ends_the_line_only_if_it_spans_one() {
    // a comment is whitespace (§3.1): layout sees the line breaks inside it
    // and nothing else, so `toks` (which drops Newline) cannot test this
    let raw = |src: &str| -> Vec<Tok> { lex(src).unwrap().into_iter().map(|t| t.tok).collect() };
    let (a, b) = (Tok::Ident("a".into()), Tok::Ident("b".into()));
    assert_eq!(raw("a {- x -} b"), vec![a.clone(), b.clone(), Tok::Eof]);
    assert_eq!(raw("a {- x -}.b"), vec![a.clone(), Tok::Selector("b".into()), Tok::Eof]);
    assert_eq!(raw("a {- x\n y -} b"), vec![a.clone(), Tok::Newline, b.clone(), Tok::Eof]);
    assert_eq!(raw("a {- x -}\nb"), vec![a, Tok::Newline, b, Tok::Eof]);
}

#[test]
fn columns_tracked() {
    let t = lex("a\n  b\nc").unwrap();
    let cols: Vec<usize> = t
        .iter()
        .filter(|s| matches!(s.tok, Tok::Ident(_)))
        .map(|s| s.col)
        .collect();
    assert_eq!(cols, vec![1, 3, 1]);
}

#[test]
fn unexpected_characters() {
    assert!(err("!").contains("unexpected"));
    assert!(err("?").contains("unexpected"));
    assert!(err("^").contains("unexpected"));
    assert!(err("&").contains("unexpected"));
    assert!(err("|").contains("unexpected"));
}

#[test]
fn underscore_is_wildcard_not_ident() {
    assert_eq!(toks("_"), vec![Tok::Underscore]);
    assert_eq!(toks("_a"), vec![Tok::Ident("_a".into())]);
}

#[test]
fn apostrophes_in_identifiers() {
    assert_eq!(toks("x'"), vec![Tok::Ident("x'".into())]);
    assert_eq!(toks("x''y"), vec![Tok::Ident("x''y".into())]);
}

#[test]
fn non_ascii_characters_are_a_clean_error() {
    // unrecognised characters fell through to operator matching, which sliced
    // the lookahead by byte — a panic on anything outside ASCII
    for src in ["—", "λ", "é", "·", "→", "1 — 2", "x ≤ y", "🙂", "\u{a0}"] {
        let m = err(src);
        assert!(m.contains("unexpected"), "{:?} => {}", src, m);
    }
    // and they are still fine inside a text literal
    assert_eq!(toks("\"— λ é 🙂\""), vec![Tok::Text("— λ é 🙂".into())]);
}
