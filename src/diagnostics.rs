//! Read-only guidance for common CLI mistakes. The original error remains the
//! authority: these helpers neither retry an expression nor change its meaning.

use crate::ast::{Expr, TypeExpr};
use crate::config::Config;
use crate::parse::parse_expr;
use crate::shape::{compile_contract, type_name};
use crate::value::Crash;
use std::collections::BTreeSet;
use std::rc::Rc;
use unicode_width::UnicodeWidthChar;

/// Add a location and the offending source line without inventing a column
/// that the parser does not currently retain.
pub fn format_source_error(label: &str, src: &str, line: usize, msg: &str) -> String {
    let mut output = format!("{}: line {}: {}", label, line, msg);
    if let Some(source_line) = line.checked_sub(1).and_then(|n| src.split('\n').nth(n)) {
        output.push_str(&format!("\n  {} | {}", line, source_excerpt(source_line.trim_end_matches('\r'))));
    }
    output
}

/// Leave room for an ellipsis, count wide characters, and render terminal
/// controls as visible escapes. Charge even zero-width characters one column
/// so a malformed line of combining marks cannot produce unlimited output.
fn source_excerpt(line: &str) -> String {
    const MAX_COLUMNS: usize = 160;
    let mut excerpt = String::new();
    let mut columns = 0;
    for character in line.chars() {
        let visible = if character.is_control() {
            character.escape_default().to_string()
        } else {
            character.to_string()
        };
        let width: usize = visible.chars().map(|c| c.width().unwrap_or(0).max(1)).sum();
        if columns + width > MAX_COLUMNS - 1 {
            excerpt.push('…');
            break;
        }
        excerpt.push_str(&visible);
        columns += width;
    }
    excerpt
}

/// Suggest a next step only when the reported failure agrees with the user's
/// expression. In particular, an error inside a config definition must not be
/// misdiagnosed as a missing argument on the command line.
pub fn expression_hint(text: &str, cfg: &Config, crash: &Crash) -> Option<String> {
    expression_hint_for(text, cfg, crash, Invocation::Expression)
}

/// Diagnose the expression following `push`, preserving that reserved command
/// in copyable examples so following a hint cannot turn a push into a local edit.
pub fn push_expression_hint(text: &str, cfg: &Config, crash: &Crash) -> Option<String> {
    expression_hint_for(text, cfg, crash, Invocation::Push)
}

#[derive(Clone, Copy)]
enum Invocation {
    Expression,
    Push,
}

fn expression_hint_for(text: &str, cfg: &Config, crash: &Crash, invocation: Invocation) -> Option<String> {
    let expr = parse_expr(text, Rc::new(cfg.global_names.clone())).ok()?;
    let names = vocabulary(cfg);
    if let Some(name) = crash.msg.strip_prefix("unbound name `").and_then(|s| s.strip_suffix('`')) {
        if crash.def.is_some() {
            return None;
        }
        let mut free = BTreeSet::new();
        expr.free_vars(&mut Vec::new(), &mut free);
        if !free.contains(name) {
            return None;
        }
        if let Some(hint) = text_argument_hint(&expr, cfg, &names, name, invocation) {
            return Some(hint);
        }
        let mut suggestions: Vec<_> = names
            .iter()
            .filter(|candidate| candidate.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_'))
            .filter_map(|candidate| {
                let limit = if name.chars().count() > 4 { 2 } else { 1 };
                let distance = edit_distance(name, candidate, limit)?;
                (distance > 0).then_some((distance, *candidate))
            })
            .collect();
        suggestions.sort_unstable();
        if let Some((distance, best)) = suggestions.first().copied() {
            let close: Vec<_> = suggestions
                .iter()
                .take_while(|(d, _)| *d == distance)
                .take(3)
                .map(|(_, n)| format!("`{}`", n))
                .collect();
            return Some(if close.len() == 1 {
                format!("Did you mean `{}`? See `{}`.", best, help_command(best))
            } else {
                format!("Did you mean {}? Use `j help NAME` for details.", close.join(" or "))
            });
        }
        return Some("Run `j commands` to browse the names available in your config.".into());
    }

    let (name, args) = application(&expr)?;
    let signature = cfg.sigs.get(name)?;
    let contract = compile_contract(&cfg.shapes, signature);
    let parameter = contract.params.get(args.len())?;
    // A record supplied explicitly fails at an earlier argument. Requiring the
    // next argument to match confines this hint to the implicit repo argument.
    if !crash.msg.starts_with(&format!("contract: {} expected ", name))
        || !crash.msg.ends_with(&format!(" as argument {}, got record", args.len() + 1))
        || matches!(cfg.shapes.unfold(parameter), TypeExpr::Record(_))
    {
        return None;
    }
    let expected = type_name(parameter);
    Some(format!(
        "`{}` needs an argument of type {} before it can receive the repository. See `{}`.",
        name, expected, help_command(name)
    ))
}

/// `global_names` also reserves every implemented builtin for shadowing checks.
/// Only names actually supplied by this config are useful typo suggestions.
fn vocabulary(cfg: &Config) -> BTreeSet<&str> {
    cfg.defs
        .iter()
        .map(|(name, _)| name.as_str())
        .chain(cfg.builtin_names.iter().map(String::as_str))
        .collect()
}

fn unparen(mut expr: &Expr) -> &Expr {
    while let Expr::Paren(inner) = expr {
        expr = inner;
    }
    expr
}

fn application(expr: &Expr) -> Option<(&str, Vec<&Expr>)> {
    let mut head = unparen(expr);
    let mut args = Vec::new();
    while let Expr::App(function, argument) = head {
        args.push(unparen(argument));
        head = unparen(function);
    }
    args.reverse();
    match head {
        Expr::Var(name) => Some((name, args)),
        _ => None,
    }
}

fn text_argument_hint(
    expr: &Expr,
    cfg: &Config,
    names: &BTreeSet<&str>,
    missing: &str,
    invocation: Invocation,
) -> Option<String> {
    let (name, args) = application(expr)?;
    let contract = compile_contract(&cfg.shapes, cfg.sigs.get(name)?);
    let index = args.iter().position(|arg| matches!(arg, Expr::Var(n) if n == missing))?;
    let parameter = contract.params.get(index)?;
    if !matches!(cfg.shapes.unfold(parameter), TypeExpr::Con(n) if n == "Text") {
        return None;
    }

    // For a single Text argument, shell-stripped words can be shown in a
    // complete, copyable example. Do not guess how to group multiple Text
    // parameters or how to rewrite a larger expression.
    let only_text_argument = index == 0
        && contract.params.iter().skip(1).all(|ty| matches!(ty.as_ref(), TypeExpr::Con(n) if n == "Repo"));
    let words: Option<Vec<_>> = args
        .iter()
        .map(|arg| match arg {
            Expr::Var(n) if !names.contains(n.as_str()) => Some(n.as_str()),
            _ => None,
        })
        .collect();
    let prefix = match invocation {
        Invocation::Expression => "",
        Invocation::Push => "push ",
    };
    match words.filter(|_| only_text_argument) {
        Some(words) => Some(format!(
            "If you meant text, preserve its double quotes by quoting the whole expression: j {}. See `{}`.",
            shell_quote(&format!("{}{} \"{}\"", prefix, name, words.join(" "))), help_command(name)
        )),
        None => Some(format!(
            "If you meant text, use double quotes for the text inside shell single quotes around the expression. See `{}`.",
            help_command(name)
        )),
    }
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn help_command(name: &str) -> String {
    let argument = if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        name.to_string()
    } else {
        shell_quote(name)
    };
    format!("j help {}", argument)
}

/// Bounded Damerau-Levenshtein distance catches adjacent transpositions as well
/// as inserted, removed, and replaced letters. Long identifiers are skipped so
/// error reporting cannot become expensive for generated input.
fn edit_distance(a: &str, b: &str, limit: usize) -> Option<usize> {
    let a: Vec<_> = a.chars().take(129).collect();
    let b: Vec<_> = b.chars().take(129).collect();
    if a.len() > 128 || b.len() > 128 || a.len().abs_diff(b.len()) > limit {
        return None;
    }
    let mut older = vec![0; b.len() + 1];
    let mut previous: Vec<_> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];
    for (i, letter) in a.iter().enumerate() {
        current[0] = i + 1;
        for (k, other) in b.iter().enumerate() {
            current[k + 1] = (previous[k + 1] + 1)
                .min(current[k] + 1)
                .min(previous[k] + usize::from(letter != other));
            if i > 0 && k > 0 && *letter == b[k - 1] && a[i - 1] == *other {
                current[k + 1] = current[k + 1].min(older[k - 1] + 1);
            }
        }
        std::mem::swap(&mut older, &mut previous);
        std::mem::swap(&mut previous, &mut current);
    }
    (previous[b.len()] <= limit).then_some(previous[b.len()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{eval_config, load_config};
    use crate::eval::Interp;
    use crate::value::Value;

    fn reference() -> Config {
        load_config(include_str!("../config.j")).unwrap()
    }

    fn failure(cfg: &Config, text: &str) -> Crash {
        let mut interp = Interp::dummy();
        interp.shapes = cfg.shapes.clone();
        eval_config(&mut interp, cfg).unwrap();
        let expr = Rc::new(parse_expr(text, Rc::new(cfg.global_names.clone())).unwrap());
        let result = interp.eval(&expr, &interp.global_env()).and_then(|value| match value {
            Value::Fun(_) => interp.apply(value, Value::record(&[])),
            value => Ok(value),
        });
        result.expect_err("test expression must fail")
    }

    #[test]
    fn suggests_real_commands_and_transposed_names() {
        let cfg = reference();
        for (expression, expected) in [("neww", "j help new"), ("desrcibe", "j help describe")] {
            let hint = expression_hint(expression, &cfg, &failure(&cfg, expression)).unwrap();
            assert!(hint.contains(expected), "{}", hint);
        }
    }

    #[test]
    fn suggests_custom_definitions_but_not_undeclared_builtins() {
        let cfg = load_config("user = { name = \"A\", email = \"a@b\" }\ntree = \\r -> \"\"\nimmutable = \\r -> []\nlabelled = \\n r -> []\nship = \\r -> r\n").unwrap();
        let hint = expression_hint("shipp", &cfg, &failure(&cfg, "shipp")).unwrap();
        assert!(hint.contains("j help ship"));
        let hint = expression_hint("repla", &cfg, &failure(&cfg, "repla")).unwrap();
        assert!(!hint.contains("replay"));
        assert!(hint.contains("j commands"));
    }

    #[test]
    fn explains_shell_quoting_only_for_text_parameters() {
        let cfg = reference();
        for expression in ["describe hello", "describe first change"] {
            let hint = expression_hint(expression, &cfg, &failure(&cfg, expression)).unwrap();
            assert!(hint.contains("j 'describe \""), "{}", hint);
            assert!(hint.contains("j help describe"));
        }
        let hint = expression_hint("rebase helloo", &cfg, &failure(&cfg, "rebase helloo")).unwrap();
        assert!(!hint.contains("quotes"));
    }

    #[test]
    fn push_quoting_examples_preserve_the_reserved_command() {
        let cfg = reference();
        for (expression, corrected) in [
            ("unlabel feature", "unlabel \"feature\""),
            ("describe hello", "describe \"hello\""),
        ] {
            let crash = failure(&cfg, expression);
            let hint = push_expression_hint(expression, &cfg, &crash).unwrap();
            assert!(hint.contains(&format!("j 'push {}'", corrected)), "{}", hint);
            assert!(!hint.contains(&format!("j '{}'", corrected)), "{}", hint);

            let ordinary = expression_hint(expression, &cfg, &crash).unwrap();
            assert!(ordinary.contains(&format!("j '{}'", corrected)), "{}", ordinary);
            assert!(!ordinary.contains("j 'push "), "{}", ordinary);
        }
    }

    #[test]
    fn explains_implicit_repo_application_but_not_explicit_type_errors() {
        let cfg = reference();
        for expression in ["describe", "rebase", "split"] {
            let hint = expression_hint(expression, &cfg, &failure(&cfg, expression)).unwrap();
            assert!(hint.contains("before it can receive the repository"), "{}", hint);
            assert!(hint.contains(&format!("j help {}", expression)));
        }
        assert!(expression_hint("describe {}", &cfg, &failure(&cfg, "describe {}")).is_none());
        assert!(expression_hint("describe 42", &cfg, &failure(&cfg, "describe 42")).is_none());
    }

    #[test]
    fn custom_signatures_drive_hints_after_partial_application() {
        let src = format!("{}\ncustom : Text -> Int -> Edit\ncustom = \\message count repo -> repo\n", include_str!("../config.j"));
        let cfg = load_config(&src).unwrap();
        let expression = "custom \"message\"";
        let hint = expression_hint(expression, &cfg, &failure(&cfg, expression)).unwrap();
        assert!(hint.contains("needs an argument of type Int"), "{}", hint);
        assert!(hint.contains("j help custom"));
        let expression = "custom \"message\" {}";
        assert!(expression_hint(expression, &cfg, &failure(&cfg, expression)).is_none());
    }

    #[test]
    fn quote_examples_preserve_apostrophes_in_valid_identifiers() {
        let cfg = reference();
        let expression = "describe don't";
        let hint = expression_hint(expression, &cfg, &failure(&cfg, expression)).unwrap();
        assert!(hint.contains("j 'describe \"don'\\''t\"'"), "{}", hint);
    }

    #[test]
    fn does_not_blame_the_cli_for_unbound_names_inside_definitions() {
        let cfg = reference();
        assert!(expression_hint("new", &cfg, &Crash::new("unbound name `neww`")).is_none());
        assert!(expression_hint("\\x -> x", &cfg, &Crash::new("unbound name `x`")).is_none());
    }

    #[test]
    fn source_errors_name_the_file_and_preserve_line_numbers() {
        let output = format_source_error("/tmp/config.j", "first\nbroken =\n", 2, "unexpected newline");
        assert_eq!(output, "/tmp/config.j: line 2: unexpected newline\n  2 | broken =");
        assert_eq!(format_source_error("expression", "bad\n", 2, "unexpected end"), "expression: line 2: unexpected end\n  2 | ");
        assert_eq!(format_source_error("expression", "bad", 8, "unexpected end"), "expression: line 8: unexpected end");
    }

    #[test]
    fn huge_unicode_source_lines_have_bounded_excerpts() {
        use unicode_width::UnicodeWidthStr;

        for source in ["(".repeat(200_000), "界é🙂\u{301}".repeat(50_000), "\u{301}".repeat(200_000)] {
            let output = format_source_error("expression", &source, 1, "nested too deeply");
            let excerpt = output.split_once("  1 | ").unwrap().1;
            assert!(excerpt.ends_with('…'));
            assert!(excerpt.width() <= 160, "excerpt exceeds display width budget");
            assert!(excerpt.chars().count() <= 160, "zero-width characters must also be bounded");
            assert!(output.len() < 700, "diagnostic must not repeat the entire huge line");
        }
    }

    #[test]
    fn source_excerpts_escape_terminal_controls() {
        let output = format_source_error("expression", "\tfirst\u{1b}[31m\rsecond\u{7}", 1, "invalid input");
        let excerpt = output.split_once("  1 | ").unwrap().1;
        assert_eq!(excerpt, "\\tfirst\\u{1b}[31m\\rsecond\\u{7}");
        assert!(!excerpt.chars().any(char::is_control));
    }
}
