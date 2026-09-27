//! `show` (§5.2), unified diff (§4.9 `diff`), and the difftastic bridge (§7.10).

use crate::eval::Interp;
use crate::value::{BlobContent, Crash, FunVal, Value};
use unicode_width::UnicodeWidthChar;

pub fn show(interp: &Interp, v: &Value) -> String {
    let s = render(interp, v);
    // line breaking: one line if it fits in 80 columns, counted in display
    // cells as everywhere else (§5.1), not code points. Not by
    // `render::width`, which skips colour codes: `show` writes none, and an
    // ESC from the data would hide everything up to the next `m`
    let w: usize = s.chars().map(|c| UnicodeWidthChar::width(c).unwrap_or(0)).sum();
    if w <= 80 {
        return s;
    }
    render_wide(interp, v, 0)
}

fn render(interp: &Interp, v: &Value) -> String {
    match v {
        Value::Thunk(t) => match t.force() {
            Ok(v) => render(interp, &v),
            Err(_) => "<lazy>".to_string(),
        },
        Value::Int(n) => {
            if n.sign() == num_bigint::Sign::Minus {
                format!("(0 - {})", n.magnitude())
            } else {
                n.to_string()
            }
        }
        Value::Text(t) => text_literal(t),
        Value::Bool(b) => b.to_string(),
        Value::Id(id) => format!("@{}", id),
        Value::List(xs) => {
            // list elements must be atoms: parenthesise applications and
            // operator expressions (§3.4)
            let parts: Vec<String> = xs.iter().map(|x| render_atom(interp, x)).collect();
            format!("[{}]", parts.join(" "))
        }
        Value::Record(m) => {
            let parts: Vec<String> = m
                .iter()
                .map(|(k, x)| format!("{} = {}", k, render(interp, x)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
        Value::Blob(b) => match &b.content {
            BlobContent::Resolved(bytes) => match String::from_utf8(bytes.as_ref().clone()) {
                Ok(s) => format!("blob {}", text_literal(&s)),
                Err(_) => "blob \"<binary>\"".into(),
            },
            BlobContent::Lazy(_) | BlobContent::Conflict(_) => {
                let s = b
                    .bytes()
                    .map(|v| String::from_utf8_lossy(&v).to_string())
                    .unwrap_or_else(|_| "<unreadable>".into());
                let marker = if b.is_unresolved() { "{- unresolved -} " } else { "" };
                format!("{}blob {}", marker, text_literal(&s))
            }
        },
        Value::Shape(s) => s.name.clone(),
        Value::Fun(f) => render_fun(interp, f),
    }
}

fn render_fun(interp: &Interp, f: &FunVal) -> String {
    match f {
        FunVal::Builtin { name, args, .. } => {
            // a selector `.name` carries its field as a baked-in first
            // argument (builtins::make_selector) and renders as the selector
            // atom itself (§3.4), not as an application to that field
            if name.starts_with('.') && name.len() > 1 {
                return name.clone();
            }
            // operator builtins render parenthesised when bare
            if args.is_empty() {
                if is_operator_name(name) {
                    format!("({})", name)
                } else if name == "(.)" {
                    "(.)".into()
                } else {
                    name.clone()
                }
            } else {
                // partial application: name followed by applied args
                let base = if is_operator_name(name) {
                    format!("({})", name)
                } else if name == "(.)" {
                    "(.)".into()
                } else {
                    name.clone()
                };
                // args may contain a baked-in value (compose)
                match name.as_str() {
                    "(.)" if args.len() == 2 => {
                        format!(
                            "(.) ({}) ({})",
                            render_atom(interp, &args[0]),
                            render_atom(interp, &args[1])
                        )
                    }
                    _ => {
                        let shown: Vec<String> =
                            args.iter().map(|a| render_atom(interp, a)).collect();
                        format!("{} {}", base, shown.join(" "))
                    }
                }
            }
        }
        FunVal::Closure {
            name: Some(n),
            applied_args,
            ..
        } => {
            if applied_args.is_empty() {
                n.clone()
            } else {
                // partial application of a named definition: the name followed
                // by the arguments supplied so far (§5.2)
                let args: Vec<String> =
                    applied_args.iter().map(|a| render_atom(interp, a)).collect();
                format!("{} {}", n, args.join(" "))
            }
        }
        FunVal::Closure {
            name: None,
            applied_args,
            params,
            body,
            src,
            ..
        } => {
            // a lambda or section renders as its source, exactly as written
            // (§5.2); only an AST built without source is re-rendered
            let base = if src.as_str().is_empty() {
                crate::parse::render_lambda(params, body)
            } else {
                src.as_str().to_string()
            };
            if applied_args.is_empty() {
                base
            } else {
                // partial application: the lambda, parenthesised as it is not
                // atomic, followed by the arguments supplied so far (§5.2)
                let args: Vec<String> =
                    applied_args.iter().map(|a| render_atom(interp, a)).collect();
                format!("({}) {}", base, args.join(" "))
            }
        }
        FunVal::OrFun(a, b) => format!("({} or {})", render_atom(interp, a), render_atom(interp, b)),
        FunVal::Labelled(n, _) => format!("%{}", n),
        FunVal::ComposeLazy(f, g) => format!("({} . {})", render_atom(interp, f), render_atom(interp, g)),
    }
}

fn render_atom(interp: &Interp, v: &Value) -> String {
    // parenthesise if not atomic; a nested list is self-delimiting (a `[`
    // cannot begin a postfix), but a record is not: `{ … }` following another
    // element would parse as a record *update* on it (§3.4), so records keep
    // their parens
    let atomic = matches!(
        v,
        Value::Int(_) | Value::Text(_) | Value::Bool(_) | Value::Id(_) | Value::Shape(_) | Value::List(_)
    ) || is_section(v);
    let s = render(interp, v);
    if atomic {
        s
    } else {
        format!("({})", s)
    }
}

/// A section renders as its source, `(op e)` or `(e op)` (§5.2), which is
/// already an atom (§3.4) and takes no parentheses of its own. A lambda's
/// source begins with its `\` instead, and a section, taking one argument,
/// is never partially applied.
fn is_section(v: &Value) -> bool {
    match v {
        Value::Fun(f) => matches!(
            f.as_ref(),
            FunVal::Closure { name: None, applied_args, src, .. }
                if applied_args.is_empty() && src.as_str().starts_with('(')
        ),
        _ => false,
    }
}

fn is_operator_name(n: &str) -> bool {
    matches!(
        n,
        "." | "*"
            | "+"
            | "-"
            | "++"
            | "::"
            | "=="
            | "/="
            | "<"
            | "<="
            | ">"
            | ">="
            | "&&"
            | "||"
    )
}

pub fn text_literal(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn render_wide(interp: &Interp, v: &Value, indent: usize) -> String {
    match v {
        // see through a lazy `files` list as `render` does, so that it breaks
        // like the equal forced list instead of staying on one line
        Value::Thunk(t) => match t.force() {
            Ok(v) => render_wide(interp, &v, indent),
            Err(_) => "<lazy>".to_string(),
        },
        Value::List(xs) if !xs.is_empty() => {
            let mut out = String::from("[");
            for (i, x) in xs.iter().enumerate() {
                if i > 0 {
                    out.push('\n');
                    out.push_str(&" ".repeat(indent + 2));
                }
                // list elements must be atoms (§3.4)
                let s = render_wide(interp, x, indent + 2);
                if matches!(
                    x,
                    Value::Int(_)
                        | Value::Text(_)
                        | Value::Bool(_)
                        | Value::Id(_)
                        | Value::Shape(_)
                        | Value::List(_)
                        | Value::Record(_)
                ) || is_section(x)
                {
                    out.push_str(&s);
                } else {
                    out.push_str(&format!("({})", s));
                }
            }
            out.push(']');
            out
        }
        Value::Record(m) if !m.is_empty() => {
            let mut out = String::from("{ ");
            for (i, (k, x)) in m.iter().enumerate() {
                if i > 0 {
                    out.push('\n');
                    out.push_str(&" ".repeat(indent));
                    out.push_str(", ");
                }
                out.push_str(&format!("{} = {}", k, render_wide(interp, x, indent + 2)));
            }
            out.push_str(" }");
            out
        }
        _ => render(interp, v),
    }
}

// ----------------------------------------------------------------------
// unified diff, 3 lines of context, no headers (§4.9 `diff`)
// ----------------------------------------------------------------------

pub fn unified_diff(a: &str, b: &str) -> String {
    // lines end after each `\n` and nowhere else, as for GNU diff and git:
    // `similar`'s own line splitting also ends one at a lone `\r`, so a last
    // line `b\r` counted as terminated and got no marker below
    let old: Vec<&str> = a.split_inclusive('\n').collect();
    let new: Vec<&str> = b.split_inclusive('\n').collect();
    let diff = similar::TextDiff::configure().diff_slices(&old, &new);
    let mut out = String::new();
    for hunk in diff.unified_diff().context_radius(3).iter_hunks() {
        out.push_str(&format!("{}\n", hunk.header()));
        for change in hunk.iter_changes() {
            let sign = match change.tag() {
                similar::ChangeTag::Delete => "-",
                similar::ChangeTag::Insert => "+",
                similar::ChangeTag::Equal => " ",
            };
            out.push_str(sign);
            let text: &str = change.value();
            out.push_str(text);
            // a line with no terminator, only ever the last, is ended here
            // and marked, as GNU diff and git do: otherwise dropping the
            // final newline shows as the same line removed and re-added,
            // alike in both directions
            if !text.ends_with('\n') {
                out.push('\n');
                out.push_str("\\ No newline at end of file\n");
            }
        }
    }
    out
}

// ----------------------------------------------------------------------
// difftastic (§7.10)
// ----------------------------------------------------------------------

pub fn difft(path: &Value, a: &Value, b: &Value) -> Result<Value, Crash> {
    let comps = path.as_list()?;
    let last = comps
        .last()
        .map(|c| c.as_text().map(|s| s.to_string()))
        .transpose()?
        .unwrap_or_else(|| "file".to_string());
    let a_bytes = match a {
        Value::Blob(bl) => bl.bytes()?,
        v => return Err(Crash::new(format!("difft: expected a Blob, got a {}", v.kind_name()))),
    };
    let b_bytes = match b {
        Value::Blob(bl) => bl.bytes()?,
        v => return Err(Crash::new(format!("difft: expected a Blob, got a {}", v.kind_name()))),
    };
    let dir = tempfile::tempdir()
        .map_err(|e| Crash::new(format!("difft: cannot create temp dir: {}", e)))?;
    let old = dir.path().join(format!("old-{}", last));
    let new = dir.path().join(format!("new-{}", last));
    std::fs::write(&old, &a_bytes).map_err(|e| Crash::new(format!("difft: {}", e)))?;
    std::fs::write(&new, &b_bytes).map_err(|e| Crash::new(format!("difft: {}", e)))?;
    let mut cmd = std::process::Command::new("difft");
    cmd.arg(&old).arg(&new);
    cmd.stdin(std::process::Stdio::null());
    if std::env::var_os("DFT_COLOR").is_none() && stdout_is_tty() {
        cmd.env("DFT_COLOR", "always");
    }
    if std::env::var_os("DFT_WIDTH").is_none() && stdout_is_tty() {
        if let Some(w) = terminal_width() {
            cmd.env("DFT_WIDTH", w.to_string());
        }
    }
    let out = cmd
        .output()
        .map_err(|_| Crash::new("difft: cannot execute `difft` (not on PATH)"))?;
    Ok(Value::text(String::from_utf8_lossy(&out.stdout).to_string()))
}

pub fn stdout_is_tty() -> bool {
    unsafe { libc::isatty(libc::STDOUT_FILENO) == 1 }
}

pub fn stdin_is_tty() -> bool {
    unsafe { libc::isatty(libc::STDIN_FILENO) == 1 }
}

pub fn terminal_width() -> Option<usize> {
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 {
            Some(ws.ws_col as usize)
        } else {
            None
        }
    }
}
