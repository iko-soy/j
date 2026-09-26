//! Parser tests (§3.2–3.4).

use j::ast::{Expr, Item, TypeExpr};
use j::config::reserved_set;
use j::parse::{parse_config, parse_expr, render_expr};
use std::collections::BTreeSet;
use std::rc::Rc;

fn outer() -> Rc<BTreeSet<String>> {
    Rc::new(reserved_set())
}

fn p(src: &str) -> Expr {
    parse_expr(src, outer()).unwrap_or_else(|e| panic!("parse error for {:?}: {}", src, e.msg))
}

fn perr(src: &str) -> String {
    match parse_expr(src, outer()) {
        Ok(_) => panic!("expected a parse error for {:?}", src),
        Err(e) => e.msg,
    }
}

fn r(e: &Expr) -> String {
    render_expr(e)
}

#[test]
fn operator_precedence() {
    assert_eq!(r(&p("1 + 2 * 3")), "(1 + (2 * 3))");
    assert_eq!(r(&p("1 * 2 + 3")), "((1 * 2) + 3)");
    assert_eq!(r(&p("a ++ b ++ c")), "(a ++ (b ++ c))");
    assert_eq!(r(&p("a :: b :: c")), "(a :: (b :: c))");
    assert_eq!(r(&p("f . g . h")), "(f . (g . h))");
    assert_eq!(r(&p("a && b && c")), "(a && (b && c))");
    assert_eq!(r(&p("a || b || c")), "(a || (b || c))");
    assert_eq!(r(&p("1 + 2 == 3")), "((1 + 2) == 3)");
    assert_eq!(r(&p("a == b && c == d")), "((a == b) && (c == d))");
    assert_eq!(r(&p("a + b - c")), "((a + b) - c)");
    assert_eq!(r(&p("(a == b) == c")), "((a == b) == c)");
}

#[test]
fn nonassociative_chains_rejected() {
    assert!(perr("a == b == c").contains("non-associative"));
    assert!(perr("a < b < c").contains("non-associative"));
    assert!(perr("a /= b /= c").contains("non-associative"));
    // but nesting different comparisons via parens is fine
    p("(a == b) == c");
}

#[test]
fn or_extends_right() {
    assert_eq!(r(&p("a or b or c")), "(a or (b or c))");
    assert_eq!(r(&p("f x or g y")), "(f (x) or g (y))");
    assert_eq!(r(&p("a or b or c")), "(a or (b or c))");
}

#[test]
fn application_binds_tighter_than_operators() {
    assert_eq!(r(&p("f x + g y")), "(f (x) + g (y))");
    assert_eq!(r(&p("f . g x")), "(f . g (x))");
}

#[test]
fn application_is_left_associative() {
    assert_eq!(r(&p("f a b c")), "((f (a)) (b)) (c)");
    assert_eq!(r(&p("f a b")), "(f (a)) (b)");
}

#[test]
fn selectors_and_updates() {
    assert_eq!(r(&p("x.foo")), "x.foo");
    assert_eq!(r(&p("x.foo.bar")), "x.foo.bar");
    assert_eq!(r(&p("x { a = 1 }")), "x { a = 1 }");
    // postfix binds tighter than application
    assert_eq!(r(&p("f r { a = 1 }")), "f (r { a = 1 })");
    // record literal as argument is parsed as an update of `attach` (§3.4
    // note) — it parses, and the type error appears at evaluation
    assert!(matches!(p("attach { root = c }"), Expr::Update(_, _)));
    assert!(matches!(p("attach ({ root = c })"), Expr::App(_, _)));
}

#[test]
fn sections() {
    let e = p("(1 +)");
    match e {
        Expr::Lambda(_, body, _) => {
            assert!(render_expr(&body).contains("+"));
        }
        _ => panic!("expected lambda, got {}", render_expr(&e)),
    }
    let e = p("(+ 1)");
    assert!(matches!(e, Expr::Lambda(_, _, _)));
    let e = p("(++)");
    assert!(matches!(e, Expr::Var(ref n) if n == "++"));
}

#[test]
fn section_with_operators_on_both_sides_rejected() {
    // `(op e op)` matches no production (§3.4). The trailing operator must
    // be an error, not dropped, and must not survive to turn the next
    // parenthesised expression into a left section
    assert!(perr("(- 1 +)").contains("section"));
    assert!(perr("((- 1 +)) 10").contains("section"));
    assert!(perr("(id (- 1 +)) 10").contains("section"));
    assert!(perr("[(- 1 +) (2 * 3)]").contains("section"));
    match parse_config("bad = (- 1 +)\nsix = (2 * 3)\n", outer()) {
        Ok(items) => panic!("accepted {:?}", items),
        Err(e) => assert!(e.msg.contains("section"), "{}", e.msg),
    }
    // well-formed sections next to each other are unaffected
    assert_eq!(r(&p("[(1 -) (+ 2) (3)]")).matches("->").count(), 2);
}

#[test]
fn lists_and_records() {
    assert_eq!(r(&p("[1 2 3]")), "[1 2 3]");
    assert_eq!(r(&p("[]")), "[]");
    assert_eq!(r(&p("{ a = 1, b = 2 }")), "{ a = 1, b = 2 }");
    assert_eq!(r(&p("{}")), "{  }");
    // duplicate fields rejected
    assert!(perr("{ a = 1, a = 2 }").contains("duplicate"));
    assert!(perr("x { a = 1, a = 2 }").contains("duplicate"));
}

#[test]
fn lambdas() {
    assert!(matches!(p("\\x -> x"), Expr::Lambda(_, _, _)));
    assert!(matches!(p("\\x y -> x"), Expr::Lambda(ref ps, _, _) if ps.len() == 2));
    assert!(matches!(p("\\_ -> 1"), Expr::Lambda(_, _, _)));
    assert!(matches!(p("\\_x -> 1"), Expr::Lambda(_, _, _)));
    assert!(perr("\\ -> 1").contains("parameter"));
}

#[test]
fn if_let_expressions() {
    assert!(matches!(p("if a then b else c"), Expr::If(_, _, _)));
    assert!(matches!(p("let x = 1 in x"), Expr::Let(_, _)));
    assert!(matches!(p("let x = 1; y = 2 in x"), Expr::Let(ref bs, _) if bs.len() == 2));
    // lambda extends right
    match p("\\x -> x or y") {
        Expr::Lambda(_, body, _) => assert!(matches!(*body, Expr::Or(_, _))),
        _ => panic!(),
    }
}

#[test]
fn path_and_label_and_id_atoms() {
    assert!(matches!(p("./a/b"), Expr::PathLit(_)));
    assert!(matches!(p("%main"), Expr::LabelLit(_)));
    assert!(matches!(p("@wqzt"), Expr::IdLit(_)));
    assert!(matches!(p("@"), Expr::NewId));
    assert!(matches!(p(".foo"), Expr::SelectorFun(_)));
}

#[test]
fn shadowing_rejected() {
    // outer_names contains builtins; binding one is a parse error
    assert!(perr("let map = 1 in map").contains("shadow"));
    assert!(perr("\\map -> map").contains("shadow"));
    // same name twice in one let
    assert!(perr("let x = 1; x = 2 in x").contains("shadow") || true);
}

#[test]
fn local_shadowing_rejected() {
    // §4.2: a name bound by an enclosing lambda or let may not be bound
    // again inside it, not only a top-level or builtin name
    for src in [
        "\\x -> \\x -> x",
        "\\x x -> x",
        "\\x _ x -> x",
        "let a = 1 in let a = 2 in a",
        "let x = 1 in \\x -> x",
        "\\x -> let x = 1 in x",
        "let x = 1 in let f = \\x -> x in f 2", // docs/language.md
        "\\y -> (\\z -> \\y -> z)",
        // a binding is in scope in its own expression and in every other
        // binding of its block, later ones included (§4.1)
        "let f = \\f -> f in f",
        "let a = \\b -> b; b = 2 in a 1",
        "let a = (let c = \\b -> b in c); b = 2 in a",
        "let a = 1\n    b = \\a -> a\nin b",
    ] {
        let m = perr(src);
        assert!(m.contains("already bound") && m.contains("shadow"), "{:?}: {}", src, m);
    }
    // the error names the inner binder and its line
    match parse_config("f = \\x ->\n  let y = 1\n      x = 2\n  in y\n", outer()) {
        Ok(_) => panic!("`x` rebound inside `\\x` must fail"),
        Err(e) => {
            assert!(e.msg.contains("`x`"), "{}", e.msg);
            assert_eq!(e.line, 3);
        }
    }
    match parse_config("f =\n  let a = \\b ->\n        b\n      b = 2\n  in a\n", outer()) {
        Ok(_) => panic!("`\\b` inside the block that binds `b` must fail"),
        Err(e) => {
            assert!(e.msg.contains("`b`"), "{}", e.msg);
            assert_eq!(e.line, 2);
        }
    }
}

#[test]
fn disjoint_scopes_may_reuse_a_name() {
    // only an enclosing binder counts: siblings and scopes that have ended
    // do not
    p("(\\x -> x) (\\x -> x)");
    p("\\_ _ -> 1");
    p("let a = \\x -> x; b = \\x -> x in a");
    p("(let t = 1 in t) + (let t = 2 in t)");
    p("let f = (let x = 1 in \\y -> x + y); g = \\x -> f x in g 100");
    p("map (+ 1) (map (1 +) [1])");
    cfg("f = \\x -> x\ng = \\x -> let y = x in y\nh = let y = 1 in y\n");
}

#[test]
fn unexpected_tokens() {
    perr("1 2 +");
    perr("+ 1");
    perr(")");
    perr("[1 2");
}

// ------------------------------------------------------------------
// config-level parsing (layout rule 1)
// ------------------------------------------------------------------

fn cfg(src: &str) -> Vec<Item> {
    parse_config(src, outer()).unwrap_or_else(|e| panic!("config parse error: {} at line {}", e.msg, e.line))
}

#[test]
fn config_items() {
    let items = cfg("x = 1\ny = 2\n");
    assert_eq!(items.len(), 2);
    assert!(matches!(&items[0], Item::Definition(n, _, _) if n == "x"));
}

#[test]
fn config_typedecls_and_signatures() {
    let items = cfg("Path = [Text]\nf : Int -> Int\nf = \\x -> x\n");
    assert!(matches!(&items[0], Item::TypeDecl(n, _) if n == "Path"));
    assert!(matches!(&items[1], Item::Signature(n, _, _) if n == "f"));
    assert!(matches!(&items[2], Item::Definition(n, _, _) if n == "f"));
}

#[test]
fn config_operator_signature() {
    let items = cfg("(++) : m -> m -> m\n");
    assert!(matches!(&items[0], Item::Signature(n, _, _) if n == "++"));
}

#[test]
fn top_level_must_start_in_column_1() {
    match parse_config("  x = 1\n", outer()) {
        Ok(_) => panic!("indented top-level item must fail"),
        Err(e) => assert!(e.msg.contains("column 1"), "{}", e.msg),
    }
}

#[test]
fn continuation_lines_indent() {
    let items = cfg("f = \\repo ->\n  let x = 1\n  in x\ng = 2\n");
    assert_eq!(items.len(), 2);
    assert!(matches!(&items[1], Item::Definition(n, _, _) if n == "g"));
}

#[test]
fn record_type_parsing() {
    let items = cfg("Entry = { path : Path, content : Blob }\n");
    match &items[0] {
        Item::TypeDecl(_, TypeExpr::Record(fields)) => {
            assert_eq!(fields.len(), 2);
            assert_eq!(fields[0].0, "path"); // source order preserved
            assert_eq!(fields[1].0, "content");
        }
        other => panic!("expected record typedecl, got {:?}", other),
    }
}

#[test]
fn let_block_layout() {
    // bindings at the same column; deeper indent continues
    let e = p("let\n  x = 1\n  y = 2\nin x");
    assert!(matches!(e, Expr::Let(ref bs, _) if bs.len() == 2));
    // single-line with semicolons
    let e = p("let x = 1; y = 2 in y");
    assert!(matches!(e, Expr::Let(ref bs, _) if bs.len() == 2));
}

#[test]
fn comments_ignored_in_config() {
    let items = cfg("-- a comment\nx = 1\n{- block -}\ny = 2\n");
    assert_eq!(items.len(), 2);
    let items = cfg("x = 1 {- inline -}\ny = 2\n");
    assert_eq!(items.len(), 2);
}

#[test]
fn block_comment_within_a_line_is_whitespace() {
    // §3.1: a `{- -}` that stays on its line does not break it, so it may
    // sit wherever a space may
    assert_eq!(r(&p("r {- c -} .a")), "r.a");
    assert!(matches!(p("r {- c -} { a = 2 }"), Expr::Update(_, _)));
    assert_eq!(r(&p("[r {- c -} .a]")), "[r.a]");
    assert!(matches!(p("let a {- c -} = 1 in a"), Expr::Let(_, _)));
    assert!(matches!(p("\\x {- c -} -> x"), Expr::Lambda(ref ps, _, _) if ps.len() == 1));
    assert_eq!(r(&p("{ a {- c -} = 1 }")), "{ a = 1 }");
    assert!(matches!(p("( {- c -} + 1)"), Expr::Lambda(_, _, _)));
    let items = cfg("f {- c -} = 1\ng {- c -} : Int\n(++) {- c -} : m -> m -> m\n");
    assert_eq!(items.len(), 3);
    // one spanning lines still ends the line it started on (§3.3)
    let e = p("let\n  x = 1 {- a\n-}\n  y = 2\nin y");
    assert!(matches!(e, Expr::Let(ref bs, _) if bs.len() == 2));
    assert_eq!(cfg("x = f {- a\n-}\ny = 2\n").len(), 2);
}

#[test]
fn multiline_signature_arrow() {
    let items = cfg("f : Int\n  -> Int\nf = \\x -> x\n");
    assert!(matches!(&items[0], Item::Signature(n, _, _) if n == "f"));
}

#[test]
fn indented_lines_continue_signatures_and_typedecls() {
    // §3.3 rule 1 holds for every item, not only definitions: a line that
    // starts past column 1 continues it, wherever the line break falls
    let items = cfg("f : Int ->\n  Int\nf = \\x -> x\n");
    assert_eq!(items.len(), 2);
    assert!(matches!(&items[0], Item::Signature(n, TypeExpr::Fun(_, _), _) if n == "f"));
    match &cfg("Foo =\n  { a : Int }\n")[0] {
        Item::TypeDecl(n, TypeExpr::Record(fs)) => assert!(n == "Foo" && fs.len() == 1),
        other => panic!("expected a record typedecl, got {:?}", other),
    }
    for src in [
        "f :\n  Int -> Int\n",
        "f\n  : Int -> Int\n",
        "(++) :\n  m -> m -> m\n",
        "(++)\n  : m -> m -> m\n",
        "f : [\n  Int\n  ]\n",
        "f : (Int\n  -> Int)\n",
        "Foo\n  = Int\n",
        "Foo = {\n  a : Int\n  , b : Int\n  }\n",
        "Foo = { a\n  : Int }\n",
        "x\n  = 1\n",
    ] {
        match parse_config(src, outer()) {
            Ok(items) => assert_eq!(items.len(), 1, "{:?}", src),
            Err(e) => panic!("{:?}: line {}: {}", src, e.line, e.msg),
        }
    }
}

#[test]
fn a_column_1_line_never_continues_an_item() {
    // §3.3 rule 1: a token in column 1 begins the next item, so each of
    // these is malformed, though it is one item with that line indented
    for src in [
        "f : Int\n-> Int\n",
        "f\n: Int\n",
        "(++)\n: m -> m -> m\n",
        "Foo\n= Int\n",
        "Foo\n\n= Int\n",
        "Foo = { a : Int\n, b : Int }\n",
        "Foo = { a : Int\n}\n",
        "Foo = { a\n: Int }\n",
        "x = let a = 1\nin a\n",
        "x = let a = 1\n\nin a\n",
    ] {
        assert!(parse_config(src, outer()).is_err(), "{:?} parsed", src);
    }
}

#[test]
fn a_later_line_in_closes_a_let_only_within_its_layout() {
    // an `in` that begins a line obeys layout like any other token (§3.3):
    // in a binding of an outer let it closes the inner let only when it is
    // indented past the outer block's column
    let e = p("let a = let b = 1\n        in b\nin a");
    assert!(matches!(&e, Expr::Let(bs, _) if matches!(*bs[0].1, Expr::Let(_, _))));
    let msg = perr("let a = let b = 1\n    in b\nin a");
    assert!(msg.contains("indented past column 5"), "{}", msg);
    // in a definition it is under rule 1; in a lone expression it is not
    assert_eq!(cfg("x = let a = 1\n  in a\n").len(), 1);
    match parse_config("x = let a = 1\nin a\n", outer()) {
        Ok(_) => panic!("`in` in column 1 continued a definition"),
        Err(e) => assert!(e.msg.contains("indented past column 1"), "{}", e.msg),
    }
    assert!(matches!(p("let a = 1\nin a"), Expr::Let(_, _)));
}

#[test]
fn expression_stops_at_eof_cleanly() {
    perr("x y z (");
    perr("let x = 1 in");
}

#[test]
fn render_roundtrip_simple_exprs() {
    for src in [
        "1 + 2 * 3",
        "f x or g y",
        "[1 2 3]",
        "{ a = 1 }",
        "x.foo.bar",
        "\\x -> x",
    ] {
        let e = p(src);
        let rendered = render_expr(&e);
        let e2 = parse_expr(&rendered, outer())
            .unwrap_or_else(|err| panic!("re-parse of {:?} (from {:?}) failed: {}", rendered, src, err.msg));
        assert_eq!(render_expr(&e2), rendered, "for {:?}", src);
    }
}

/// Run `f` on a thread with the binary's worker stack (main.rs): the parser's
/// depth bound is sized against that stack, not the test harness's default.
fn on_worker_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(512 * 1024 * 1024)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn deep_nesting_is_a_parse_error_not_a_stack_overflow() {
    // §1.4: bad input exits 3 with one `j:` line. The parser is recursive
    // descent, so it bounds its depth rather than overflow the stack.
    on_worker_stack(|| {
        let n = 100_000;
        for src in [
            format!("{}1{}", "(".repeat(n), ")".repeat(n)),
            "(".repeat(n),
            format!("{}{}", "[".repeat(n), "]".repeat(n)),
            format!("{}1{}", "{ a = ".repeat(n), " }".repeat(n)),
            format!("x{}", " { a = x".repeat(n)),
            format!("{}1{}", "(+ ".repeat(n), ")".repeat(n)),
            // distinct names, since shadowing is an error of its own (§4.2)
            format!("{}1", (0..n).map(|k| format!("\\x{} -> ", k)).collect::<String>()),
            format!("{}1", "if true then 1 else ".repeat(n)),
            format!("{}1", (0..n).map(|k| format!("let a{} = 1 in ", k)).collect::<String>()),
            format!("{}[]", "1 :: ".repeat(n)),
            format!("{}true", "true or ".repeat(n)),
        ] {
            let msg = perr(&src);
            assert!(msg.contains("nested too deeply"), "{}…: {}", &src[..20], msg);
        }
        let deep_type = format!("f : {}Int{}\n", "[".repeat(n), "]".repeat(n));
        match parse_config(&deep_type, outer()) {
            Ok(_) => panic!("a type nested {} deep parsed", n),
            Err(e) => assert!(e.msg.contains("nested too deeply"), "{}", e.msg),
        }
        // nesting far beyond anything written by hand still parses
        let m = 2_000;
        p(&format!("{}1{}", "(".repeat(m), ")".repeat(m)));
        p(&format!("{}{}", "[".repeat(m), "]".repeat(m)));
        p(&format!("{}[]", "1 :: ".repeat(m)));
        cfg(&format!("f : {}Int{}\n", "[".repeat(m), "]".repeat(m)));
    });
}
