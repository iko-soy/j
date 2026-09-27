//! `show` (§5.2), unified diff (§4.9 `diff`), and the difftastic bridge (§7.10).

use crate::eval::Interp;
use crate::value::{BlobContent, BlobVal, Crash, FunVal, ThunkVal, Value};
use num_bigint::BigInt;
use unicode_width::UnicodeWidthChar;

/// Crashes where `v` holds a lazy part the store cannot read (§7.2), as any
/// other use of it does: a stand-in for the part would be the literal of
/// some other value, or would not parse.
///
/// Every level of `v` appends to the one string: a level that built its own
/// and copied its child's rendering into it cost the rendering's size times
/// its depth, and a Repo whose focus has a long line of descendants nests
/// them hundreds of levels deep, in megabytes of indentation. The renderers
/// recurse natively, once per level, so they also keep their frames small:
/// plain loops, a call's result returned as theirs without a `?` where it can
/// be, and the arms that end the recursion or seldom carry it on (a leaf, a
/// lazy part, a function) out of line.
pub fn show(interp: &Interp, v: &Value) -> Result<String, Crash> {
    let mut s = String::new();
    render(interp, v, &mut s)?;
    // line breaking: one line if it fits in 80 columns, counted in display
    // cells as everywhere else (§5.1), not code points. Not by
    // `render::width`, which skips colour codes: `show` writes none, and an
    // ESC from the data would hide everything up to the next `m`
    let w: usize = s.chars().map(|c| UnicodeWidthChar::width(c).unwrap_or(0)).sum();
    if w <= 80 {
        return Ok(s);
    }
    s.clear();
    render_wide(interp, v, 0, &mut s)?;
    Ok(s)
}

fn render(interp: &Interp, v: &Value, out: &mut String) -> Result<(), Crash> {
    match v {
        Value::Thunk(t) => return render_forced(interp, t, None, out),
        Value::Int(n) => render_int(n, out),
        Value::Text(t) => push_text_literal(t, out),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Id(id) => {
            out.push('@');
            out.push_str(id);
        }
        Value::List(xs) => {
            // list elements must be atoms: parenthesise applications and
            // operator expressions (§3.4)
            out.push('[');
            for (i, x) in xs.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                render_atom(interp, x, out)?;
            }
            out.push(']');
        }
        Value::Record(m) => {
            out.push_str("{ ");
            for (i, (k, x)) in m.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(k);
                out.push_str(" = ");
                render(interp, x, out)?;
            }
            out.push_str(" }");
        }
        Value::Blob(b) => return render_blob(b, out),
        Value::Shape(s) => out.push_str(&s.name),
        Value::Fun(f) => return render_fun(interp, f, out),
    }
    Ok(())
}

#[inline(never)]
fn render_int(n: &BigInt, out: &mut String) {
    if n.sign() == num_bigint::Sign::Minus {
        out.push_str(&format!("(0 - {})", n.magnitude()));
    } else {
        out.push_str(&n.to_string());
    }
}

#[inline(never)]
fn render_blob(b: &BlobVal, out: &mut String) -> Result<(), Crash> {
    match &b.content {
        BlobContent::Resolved(bytes) => match std::str::from_utf8(bytes) {
            Ok(s) => {
                out.push_str("blob ");
                push_text_literal(s, out);
            }
            Err(_) => out.push_str("blob \"<binary>\""),
        },
        BlobContent::Lazy(_) | BlobContent::Conflict(_) => {
            let bytes = b.bytes()?;
            if b.is_unresolved() {
                out.push_str("{- unresolved -} ");
            }
            out.push_str("blob ");
            push_text_literal(&String::from_utf8_lossy(&bytes), out);
        }
    }
    Ok(())
}

/// A lazy part (a commit's `files`) renders as the value it forces to: on
/// one line, or broken as that value would be at the indentation `wide`
/// gives.
#[inline(never)]
fn render_forced(interp: &Interp, t: &ThunkVal, wide: Option<usize>, out: &mut String) -> Result<(), Crash> {
    let v = t.force()?;
    match wide {
        None => render(interp, &v, out),
        Some(indent) => render_wide(interp, &v, indent, out),
    }
}

#[inline(never)]
fn render_fun(interp: &Interp, f: &FunVal, out: &mut String) -> Result<(), Crash> {
    match f {
        // a signed definition whose value is a builtin's partial application
        // or a composition (`tree = treeWith { … }`) renders as its name,
        // followed only by the arguments it is given (§5.2), which its
        // signature counts, not those baked into its value
        FunVal::Builtin {
            name,
            args,
            pending: Some((cname, c)),
            ..
        } if interp.builtin_def(name, cname).is_some() => {
            let own = c.position().unwrap_or(0).min(args.len());
            out.push_str(cname);
            render_args(interp, &args[args.len() - own..], out)
        }
        FunVal::Builtin { name, args, .. } => {
            // a selector `.name` carries its field as a baked-in first
            // argument (builtins::make_selector) and renders as the selector
            // atom itself (§3.4), not as an application to that field
            if name.starts_with('.') && name.len() > 1 {
                out.push_str(name);
                return Ok(());
            }
            // operator builtins render parenthesised; `(.)` is its own name
            if is_operator_name(name) {
                out.push('(');
                out.push_str(name);
                out.push(')');
            } else {
                out.push_str(name);
            }
            // a partial application: the name followed by the applied args,
            // which may contain a baked-in value (compose)
            if name == "(.)" && args.len() == 2 {
                for a in args {
                    out.push_str(" (");
                    render_atom(interp, a, out)?;
                    out.push(')');
                }
                Ok(())
            } else {
                render_args(interp, args, out)
            }
        }
        FunVal::Closure {
            name: Some(n),
            applied_args,
            ..
        } => {
            // a partial application of a named definition: the name followed
            // by the arguments supplied so far (§5.2)
            out.push_str(n);
            render_args(interp, applied_args, out)
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
            // (§5.2); only an AST built without source is re-rendered. A
            // partial application parenthesises it, as it is not atomic, and
            // follows it with the arguments supplied so far (§5.2)
            let applied = !applied_args.is_empty();
            if applied {
                out.push('(');
            }
            if src.as_str().is_empty() {
                out.push_str(&crate::parse::render_lambda(params, body));
            } else {
                out.push_str(src.as_str());
            }
            if applied {
                out.push(')');
            }
            render_args(interp, applied_args, out)
        }
        FunVal::OrFun(a, b) => render_infix(interp, a, " or ", b, out),
        FunVal::Labelled(n, _) => {
            out.push('%');
            out.push_str(n);
            Ok(())
        }
        FunVal::ComposeLazy(f, g) => render_infix(interp, f, " . ", g, out),
    }
}

/// `(a op b)`, for the functions an `or` or a composition builds
fn render_infix(interp: &Interp, a: &Value, op: &str, b: &Value, out: &mut String) -> Result<(), Crash> {
    out.push('(');
    render_atom(interp, a, out)?;
    out.push_str(op);
    render_atom(interp, b, out)?;
    out.push(')');
    Ok(())
}

/// The arguments of a partial application, each after a space.
fn render_args(interp: &Interp, args: &[Value], out: &mut String) -> Result<(), Crash> {
    for a in args {
        out.push(' ');
        render_atom(interp, a, out)?;
    }
    Ok(())
}

fn render_atom(interp: &Interp, v: &Value, out: &mut String) -> Result<(), Crash> {
    // parenthesise if not atomic; a nested list is self-delimiting (a `[`
    // cannot begin a postfix), but a record is not: `{ … }` following another
    // element would parse as a record *update* on it (§3.4), so records keep
    // their parens
    let atomic = matches!(
        v,
        Value::Int(_) | Value::Text(_) | Value::Bool(_) | Value::Id(_) | Value::Shape(_) | Value::List(_)
    ) || is_section(v);
    if !atomic {
        out.push('(');
    }
    render(interp, v, out)?;
    if !atomic {
        out.push(')');
    }
    Ok(())
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
    let mut out = String::new();
    push_text_literal(s, &mut out);
    out
}

fn push_text_literal(s: &str, out: &mut String) {
    out.push('"');
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
}

fn render_wide(interp: &Interp, v: &Value, indent: usize, out: &mut String) -> Result<(), Crash> {
    match v {
        // see through a lazy `files` list as `render` does, so that it breaks
        // like the equal forced list instead of staying on one line
        Value::Thunk(t) => return render_forced(interp, t, Some(indent), out),
        Value::List(xs) if !xs.is_empty() => {
            out.push('[');
            for (i, x) in xs.iter().enumerate() {
                if i > 0 {
                    out.push('\n');
                    out.push_str(&" ".repeat(indent + 2));
                }
                // list elements must be atoms (§3.4)
                let atomic = matches!(
                    x,
                    Value::Int(_)
                        | Value::Text(_)
                        | Value::Bool(_)
                        | Value::Id(_)
                        | Value::Shape(_)
                        | Value::List(_)
                        | Value::Record(_)
                ) || is_section(x);
                if !atomic {
                    out.push('(');
                }
                render_wide(interp, x, indent + 2, out)?;
                if !atomic {
                    out.push(')');
                }
            }
            out.push(']');
        }
        Value::Record(m) if !m.is_empty() => {
            out.push_str("{ ");
            for (i, (k, x)) in m.iter().enumerate() {
                if i > 0 {
                    out.push('\n');
                    out.push_str(&" ".repeat(indent));
                    out.push_str(", ");
                }
                out.push_str(k);
                out.push_str(" = ");
                render_wide(interp, x, indent + 2, out)?;
            }
            out.push_str(" }");
        }
        _ => return render(interp, v, out),
    }
    Ok(())
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
        // each side's range is the lines of it the hunk shows. `hunk.header()`
        // took a side's start from the hunk's first op and its end from the
        // last, but `similar`'s compaction moves ops past one another without
        // updating the index each keeps on the side it does not touch, so the
        // counts could disagree with the body ("@@ -1 +2,2 @@" over two old
        // lines and three new), which `patch` and `git apply` refuse. A
        // change's index on its own side is right, and each side's lines
        // come in order.
        let (mut old_first, mut old_len, mut new_first, mut new_len) = (None, 0, None, 0);
        for change in hunk.iter_changes() {
            if let Some(i) = change.old_index() {
                old_first.get_or_insert(i);
                old_len += 1;
            }
            if let Some(i) = change.new_index() {
                new_first.get_or_insert(i);
                new_len += 1;
            }
        }
        out.push_str(&format!(
            "@@ -{} +{} @@\n",
            hunk_range(old_first, old_len),
            hunk_range(new_first, new_len)
        ));
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

/// One side's range in a hunk header as `diff -u` writes it, from the index
/// of the side's first line in the hunk and how many it has: `s` for one
/// line, `s,n` otherwise. With context around every change a hunk shows no
/// line of a side only when that side has none, the empty range `0,0`.
fn hunk_range(first: Option<usize>, len: usize) -> String {
    match first {
        Some(i) if len == 1 => format!("{}", i + 1),
        Some(i) => format!("{},{}", i + 1, len),
        None => "0,0".to_string(),
    }
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
