//! j — a functional-programming-centered CLI for jj repositories (§1).

mod help;
mod setup;

use j::config::{self, Config};
use j::diagnostics;
use j::jj::JjBackend;
use j::parse::parse_expr;
use j::value::{Crash, Value};
use std::collections::BTreeSet;
use std::io::{IsTerminal, Read};
use std::process::ExitCode;
use std::rc::Rc;

const USAGE: &str = "usage: j EXPRESSION…   (or echo EXPRESSION | j)

Configure identity: j setup
Start a repository:  j init   or   j clone URL
Inspect your work:   j status   or   j tree
Discover commands:  j help   or   j commands
Command details:    j help NAME";

fn err(status: u8, msg: impl std::fmt::Display) -> ExitCode {
    j::terminal::report_error(format_args!("j: {}", msg));
    ExitCode::from(status)
}

/// Report a failure that ends the process, keeping the status as a number so
/// callers do not have to recover it from an `ExitCode`.
fn fail(status: u8, msg: impl std::fmt::Display) -> u8 {
    j::terminal::report_error(format_args!("j: {}", msg));
    status
}

/// Write a run's output to stdout (§1.2.9, `ops`). A reader that stops early
/// (`j tree | head`) closes the pipe; that is not a failure (§1.4), so the
/// rest is dropped and the run still succeeds. Any other write failure is an
/// error line. `print!` would panic on either.
fn print_output(s: &str) -> ExitCode {
    let mut out = std::io::stdout().lock();
    match j::terminal::write_output(&mut out, s) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => err(1, format!("cannot write output: {}", e)),
    }
}

fn crash_err(c: &Crash, expr_text: &str, cfg: &Config) -> ExitCode {
    j::terminal::report_error(format_args!("j: crash: {}", c.msg));
    match &c.def {
        Some(d) => j::report!("   in {}, from {}", d, expr_text),
        None => j::report!("   from {}", expr_text),
    }
    if let Some(hint) = diagnostics::expression_hint(expr_text, cfg, c) {
        j::report!("   {}", hint);
    }
    ExitCode::from(1)
}

fn config_path() -> String {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return format!("{}/j/config.j", xdg);
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    format!("{}/.config/j/config.j", home)
}

/// The default config, embedded so `j` always has it regardless of how it was
/// installed (the share/j/config.j file may not exist next to the binary).
const DEFAULT_CONFIG: &str = include_str!("../config.j");

fn load_config_file() -> Result<(Config, String), u8> {
    let path = config_path();
    let src = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // No user config yet: materialise the default there, editable by
            // the current user, so they have a starting point to customise.
            match write_default_config(&path) {
                Ok(()) => j::report!("j: created an editable default config at {}", path),
                // Another invocation may have created it after our read.
                // Read that file below, without overwriting it.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(fail(3, format!("cannot create config at {}: {}", path, e))),
            }
            match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(e) => return Err(fail(3, format!("cannot read config at {}: {}", path, e))),
            }
        }
        Err(e) => return Err(fail(3, format!("cannot read config at {}: {}", path, e))),
    };
    match config::load_config(&src) {
        Ok(c) => Ok((c, src)),
        Err(e) => {
            let message = match e {
                config::ConfigError::Parse(p) => {
                    diagnostics::format_source_error(&path, &src, p.line, &p.msg)
                }
                config::ConfigError::Validation(message) => format!("{}: {}", path, message),
            };
            Err(fail(3, format!("{}\n   Edit this file to repair your config; see `j help config`.", message)))
        }
    }
}

/// Write the embedded default config to `path`, creating parent directories
/// and making the file readable and writable by the current user.
fn write_default_config(path: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    // user-read+write (0o600 on unix): the file is meant to be edited by them
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(DEFAULT_CONFIG.as_bytes())
}

const RESERVED: &[&str] = &[
    "init", "clone", "remote", "fetch", "push", "undo", "redo", "ops", "help", "commands", "setup",
];

/// Discovery reads the user's vocabulary without creating or evaluating it.
/// A missing file uses the embedded reference; other read errors stay visible.
fn read_help_config() -> Result<(String, String), u8> {
    let path = config_path();
    match std::fs::read_to_string(&path) {
        Ok(src) => Ok((src, path)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((
            DEFAULT_CONFIG.to_string(),
            format!("embedded default config.j (no user config at {})", path),
        )),
        Err(e) => Err(fail(3, format!("cannot read config at {}: {}", path, e))),
    }
}

fn run_help(words: &[&str]) -> ExitCode {
    match words {
        [_] => print_output(help::OVERVIEW),
        [_, name] => {
            if *name == "config" {
                return print_output(&format!("{}\nConfig: {}\n", help::CONFIG_HELP, config_path()));
            }
            if let Some(output) = help::reserved_help(name) {
                return print_output(&output);
            }
            let (src, path) = or_exit(read_help_config());
            match help::command_help(&src, name) {
                Ok(Some(output)) => print_output(&format!("{}\nConfig: {}\n", output, path)),
                Ok(None) => err(
                    2,
                    format!("unknown help topic `{}`; run `j commands` to browse or `j commands {}` to search", name, name),
                ),
                Err(message) => err(3, format!("{}: {}", path, message)),
            }
        }
        _ => err(
            2,
            "usage: j help [NAME]; run `j commands` to search available names",
        ),
    }
}

fn run_commands(text: &str) -> ExitCode {
    let query = text.strip_prefix("commands").unwrap_or("").trim();
    let (src, path) = or_exit(read_help_config());
    match help::command_list(&src, query) {
        Ok(output) => print_output(&format!("{}\nConfig: {}\n", output, path)),
        Err(message) => err(3, format!("{}: {}", path, message)),
    }
}

fn command_usage(usage: &str, command: &str) -> ExitCode {
    err(
        2,
        format!("usage: j {}; see `j help {}` for examples", usage, command),
    )
}

fn command_result(result: Result<(), (u8, String)>, message: &str) -> ExitCode {
    match result {
        Ok(()) => {
            j::terminal::success(message);
            ExitCode::SUCCESS
        }
        Err(e) => err(e.0, e.1),
    }
}

fn setup_identity(cfg: &Config) -> Result<String, u8> {
    j::jj::configured_identity(cfg).map_err(|(_, message)| {
        fail(3, format!("{}: {}\n   Edit `user` in this file; see `j help config`.", config_path(), message.trim_start_matches("config.j: ")))
    })
}

fn operation_result(result: Result<String, (u8, String)>) -> ExitCode {
    match result {
        Ok(message) => {
            j::terminal::success(&message);
            ExitCode::SUCCESS
        }
        Err((code, message)) => err(code, message),
    }
}

fn main() -> ExitCode {
    // evaluation builds deep continuation chains (trampolined, but the final
    // continuation still tears down by recursive Drop), so run the real work
    // on a thread with a large stack to keep very deep histories safe
    let code = std::thread::Builder::new()
        .stack_size(512 * 1024 * 1024)
        .spawn(run)
        .expect("failed to spawn worker thread")
        .join()
        .expect("worker thread panicked");
    // §1.3: a signal to stop that came while the working copy was checked
    // out takes effect now, once the run has said what it had to
    j::jj::raise_deferred();
    code
}

/// True when stdin has input (or end-of-input) waiting within `ms`
/// milliseconds. A closed or exhausted stdin reports ready at once, so the
/// wait is only paid for a pipe that is open but idle.
fn stdin_is_ready(ms: i32) -> bool {
    let mut fds = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN,
        revents: 0,
    };
    unsafe { libc::poll(&mut fds, 1, ms) > 0 }
}

fn run() -> ExitCode {
    // §1: an expression is text, so an argument that is not UTF-8 is a
    // usage error (`std::env::args` would panic on it)
    let args: Vec<String> = match std::env::args_os()
        .skip(1)
        .map(|a| a.into_string())
        .collect()
    {
        Ok(args) => args,
        Err(a) => return err(2, format!("argument is not valid UTF-8: {:?}", a)),
    };
    // §1: stdin vs arguments. With no arguments the expression comes from
    // stdin, so reading to end-of-input is the whole point. With arguments,
    // stdin is only read to report the "both" error — and reading it blindly
    // hangs whenever the process inherited a pipe nobody is writing to (a
    // shell loop, a CI step), so wait only briefly for input to appear.
    // One byte already proves the "both" error: do not wait for the pipe's
    // writer to close it. Bytes that are not UTF-8 are input too, and an
    // error only once they are the expression.
    let mut stdin_content = Vec::new();
    let stdin_has_data = {
        if std::io::stdin().is_terminal() {
            false
        } else if args.is_empty() || stdin_is_ready(50) {
            let limit = if args.is_empty() { u64::MAX } else { 1 };
            match std::io::stdin().take(limit).read_to_end(&mut stdin_content) {
                Ok(n) => n > 0,
                Err(_) => false,
            }
        } else {
            false
        }
    };
    if stdin_has_data && !args.is_empty() {
        return err(2, "expression given both as arguments and on stdin");
    }
    // An argv help flag is a request for help, never an expression comment.
    // Comments embedded in a single expression argument or stdin remain part
    // of the language, including `j 'new --help'`.
    if args.iter().skip(1).any(|arg| matches!(arg.as_str(), "--help" | "-h")) {
        let topic = args.first().and_then(|arg| arg.split_whitespace().next());
        return match topic {
            Some("--help" | "-h" | "help") | None => run_help(&["help"]),
            Some(name) => run_help(&["help", name]),
        };
    }
    let text = if stdin_has_data {
        match String::from_utf8(stdin_content) {
            Ok(text) => text,
            Err(_) => return err(2, "the expression on stdin is not valid UTF-8"),
        }
    } else if !args.is_empty() {
        args.join(" ")
    } else {
        j::report!("{}", USAGE);
        return ExitCode::from(2);
    };

    // §1.1: reserved commands
    let trimmed = text.trim();
    if trimmed.is_empty() {
        j::report!("{}", USAGE);
        return ExitCode::from(2);
    }
    let first = trimmed.split_whitespace().next().unwrap_or("");
    // Preserve shell argument boundaries for literal reserved-command
    // arguments. Single quoted commands and stdin still use text words.
    let words: Vec<&str> = if args.first().is_some_and(|arg| arg == first) {
        args.iter().map(String::as_str).collect()
    } else {
        trimmed.split_whitespace().collect()
    };
    if matches!(first, "--help" | "-h") {
        return run_help(&words);
    }
    if RESERVED.contains(&first) {
        return run_reserved(first, trimmed, &words);
    }

    run_expression(trimmed, true)
}

fn run_reserved(cmd: &str, text: &str, words: &[&str]) -> ExitCode {
    match cmd {
        "help" => run_help(words),
        "commands" => run_commands(text),
        "setup" => match setup::run(std::path::Path::new(&config_path()), DEFAULT_CONFIG, &words[1..]) {
            Ok(output) => print_output(&output),
            Err((code, message)) => err(code, message),
        },
        "init" => {
            if words.len() != 1 {
                return command_usage("init", "init");
            }
            let (cfg, _) = or_exit(load_config_file());
            let identity = or_exit(setup_identity(&cfg));
            command_result(
                j::terminal::with_progress("Creating repository", || j::jj::cmd_init(&cfg)),
                &format!("Repository created; identity {}", identity),
            )
        }
        "clone" => {
            if words.len() != 2 && words.len() != 3 {
                return command_usage("clone URL [DIR]", "clone");
            }
            let dir = if words.len() == 3 {
                words[2].to_string()
            } else {
                match default_clone_dir(words[1]) {
                    Some(dir) => dir,
                    None => {
                        let msg = format!(
                            "`{}` names no directory to clone into; give one: j clone URL DIR",
                            words[1]
                        );
                        return err(2, msg);
                    }
                }
            };
            let (cfg, _) = or_exit(load_config_file());
            let identity = or_exit(setup_identity(&cfg));
            command_result(
                j::terminal::with_progress("Cloning repository", || j::jj::cmd_clone(&cfg, words[1], &dir)),
                &format!("Repository cloned into {}; identity {}\n   Next: cd {} && j status", dir, identity, shell_word(&dir)),
            )
        }
        "remote" => {
            if words.len() != 2 {
                return command_usage("remote URL", "remote");
            }
            let backend = or_exit(open_repo());
            command_result(
                j::terminal::with_progress("Updating remote", || backend.cmd_remote(words[1])),
                "Remote updated",
            )
        }
        "fetch" => {
            if words.len() != 1 {
                return command_usage("fetch", "fetch");
            }
            let backend = or_exit(open_repo_noconfig());
            command_result(
                j::terminal::with_progress("Fetching changes", || backend.cmd_fetch()),
                "Fetch complete",
            )
        }
        "push" => {
            let rest = text.trim_start_matches("push").trim();
            if rest.is_empty() {
                return command_usage("push EXPR", "push");
            }
            let (mut cfg, _) = or_exit(load_config_file());
            let backend = or_exit(open_repo_locked(&cfg));
            // evaluate EXPR against the recorded repository (no snapshot)
            match j::terminal::with_progress("Pushing changes", || backend.cmd_push(&mut cfg, rest)) {
                Err((3, message)) if message.starts_with("config.j:") => {
                    err(3, format!("{}: {}\n   See `j help config` for configuration help.", config_path(), message.trim_start_matches("config.j: ")))
                }
                Ok(outcome) => {
                    if outcome.published + outcome.deleted + outcome.unchanged == 0 {
                        j::report!("j: No bookmarks selected; nothing published.\n   First publication: j 'push (label \"feature\" here)'\n   Inspect available commits with `j tree` and check your selector.");
                    } else {
                        j::terminal::success(&format!(
                            "Push complete: {} published, {} deleted, {} unchanged bookmark(s)",
                            outcome.published, outcome.deleted, outcome.unchanged,
                        ));
                    }
                    ExitCode::SUCCESS
                }
                Err((code, message)) => err(code, message),
            }
        }
        "undo" => {
            if words.len() != 1 {
                return command_usage("undo", "undo");
            }
            let backend = or_exit(open_repo_noconfig());
            operation_result(
                j::terminal::with_progress("Undoing operation", || backend.cmd_undo(false)),
            )
        }
        "redo" => {
            if words.len() != 1 {
                return command_usage("redo", "redo");
            }
            let backend = or_exit(open_repo_noconfig());
            operation_result(
                j::terminal::with_progress("Redoing operation", || backend.cmd_undo(true)),
            )
        }
        "ops" => {
            if words.len() != 1 {
                return command_usage("ops", "ops");
            }
            let backend = or_exit(open_repo_noconfig());
            match j::terminal::with_progress("Loading operation log", || backend.cmd_ops()) {
                Ok(log) => print_output(&log),
                Err(e) => err(e.0, e.1),
            }
        }
        _ => unreachable!(),
    }
}

/// `clone`'s default DIR (§1.1): the last component of `url`'s path, a
/// final `.git` component skipped, minus `.git`, as `git clone` names it;
/// in an scp-style `host:repo.git`, what follows the `:`. None when that
/// leaves no name.
fn default_clone_dir(url: &str) -> Option<String> {
    let mut path = url.trim_end_matches('/');
    if let Some(repo) = path.strip_suffix("/.git") {
        path = repo.trim_end_matches('/');
    }
    let last = match path.rfind('/') {
        Some(i) => &path[i + 1..],
        None => path.rsplit(':').next().unwrap_or(path),
    };
    let name = last.strip_suffix(".git").unwrap_or(last);
    match name {
        "" | "." | ".." => None,
        name => Some(name.to_string()),
    }
}

/// A runnable shell word for literal clone destinations, including spaces and
/// quotes. Prefix a leading dash so `cd` does not interpret it as an option.
fn shell_word(path: &str) -> String {
    let path = if path.starts_with('-') { format!("./{}", path) } else { path.to_string() };
    format!("'{}'", path.replace('\'', "'\\''"))
}

fn or_exit<T>(r: Result<T, u8>) -> T {
    match r {
        Ok(v) => v,
        Err(code) => std::process::exit(code as i32),
    }
}

fn open_repo() -> Result<Rc<JjBackend>, u8> {
    match JjBackend::open(None) {
        Ok(b) => Ok(Rc::new(b)),
        Err(e) => Err(fail(e.0, e.1)),
    }
}

fn open_repo_noconfig() -> Result<Rc<JjBackend>, u8> {
    // undo/redo/ops do not read the config (§6.1), but take the lock
    match JjBackend::open(None) {
        Ok(b) => {
            b.take_lock();
            Ok(Rc::new(b))
        }
        Err(e) => Err(fail(e.0, e.1)),
    }
}

fn open_repo_locked(cfg: &Config) -> Result<Rc<JjBackend>, u8> {
    match JjBackend::open(Some(cfg)) {
        Ok(b) => {
            b.take_lock();
            Ok(Rc::new(b))
        }
        Err(e) => Err(fail(e.0, e.1)),
    }
}

fn run_expression(text: &str, snapshot: bool) -> ExitCode {
    // §1.2.1: locate the repository
    let (mut cfg, _cfg_src) = or_exit(load_config_file());
    let backend = or_exit(open_repo_locked(&cfg));

    // §1.2.2: parse the expression
    let outer = Rc::new(cfg.global_names.clone());
    let mut expr = match parse_expr(text, outer) {
        Ok(e) => e,
        Err(p) => return err(3, diagnostics::format_source_error("expression", text, p.line, &p.msg)),
    };

    // §1.2.4: build the current repository value (with snapshot)
    let (mut interp, loaded_repo, current_repo) =
        match j::terminal::with_progress("Reading repository", || backend.build_interp(&mut cfg, text, snapshot)) {
            Ok(t) => t,
            Err((2, m)) => return err(2, m),
            Err((3, m)) => return err(3, format!("{}: {}\n   See `j help config` for configuration help.", config_path(), m.trim_start_matches("config.j: "))),
            Err((_, m)) => return err(1, m),
        };

    // §1.2.5: the config's Id literals were resolved by build_interp, as soon
    // as the visible ids were known; the expression's are resolved here
    expr = match config::resolve_ids(&expr, &interp) {
        Ok(e) => e,
        Err(failures) => {
            let (prefix, candidates) = &failures[0];
            if candidates.is_empty() {
                j::terminal::report_error(format_args!("j: crash: `@{}` matches no commit", prefix));
            } else {
                j::terminal::report_error(format_args!("j: crash: `@{}` is ambiguous", prefix));
                for c in candidates {
                    j::report!("   {}", c);
                }
            }
            return ExitCode::from(1);
        }
    };

    // §1.2.6: evaluate config definitions, then the expression
    if let Err(c) = j::terminal::with_progress("Evaluating configuration", || {
        config::eval_config(&mut interp, &cfg)
    }) {
        return err(3, format!("{}: {}\n   See `j help config` for configuration help.", config_path(), c.msg));
    }
    *interp.old_repo.borrow_mut() = Some(loaded_repo.clone());
    *interp.given_repo.borrow_mut() = Some(current_repo.clone());
    let env = interp.global_env();
    let expr_rc = Rc::new(expr);
    let v = match j::terminal::with_progress("Evaluating expression", || {
        let v = interp.eval(&expr_rc, &env)?;
        // §1.2.7: apply a function result to the repo, once.
        if matches!(v, Value::Fun(_)) {
            interp.apply(v, current_repo.clone())
        } else {
            Ok(v)
        }
    }) {
        Ok(v) => v,
        Err(c) => return crash_err(&c, text, &cfg),
    };

    // §1.2.8: a Repo is persisted (unless unchanged)
    if is_repo_value(&v) {
        match j::terminal::with_progress("Saving changes", || {
            backend.persist(&mut interp, &cfg, &loaded_repo, &current_repo, &v, text)
        }) {
            Ok(changed) => {
                if std::io::stdout().is_terminal() && std::io::stderr().is_terminal() {
                    let outcome = if changed { "Saved changes" } else { "No changes" };
                    j::terminal::success(&format!("{}; {}", outcome, focus_summary(&v)));
                }
                if changed {
                    report_conflicts(&interp, &loaded_repo, &v);
                }
                ExitCode::SUCCESS
            }
            Err(c) if c.msg.starts_with("config.j: user identity") => {
                err(3, format!("{}: {}\n   Edit `user` in this file; see `j help config`.", config_path(), c.msg.trim_start_matches("config.j: ")))
            }
            Err(c) => crash_err(&c, text, &cfg),
        }
    } else {
        // §1.2.9: display
        let color = j::render::color_enabled("auto");
        match j::terminal::with_progress("Rendering output", || j::render::display(&mut interp, &v, color)) {
            Ok(s) => print_output(&s),
            Err(c) => crash_err(&c, text, &cfg),
        }
    }
}

/// Report newly introduced conflicts and conflicts in the checked-out focus.
/// Stored, resolved trees are skipped without loading their entries or blobs.
fn conflict_paths(interp: &j::eval::Interp, repo: &Value) -> Result<BTreeSet<(String, String)>, Crash> {
    let mut paths = BTreeSet::new();
    for commit in j::repo::all_commits(repo)? {
        let Value::Id(id) = commit.field("id")? else { continue };
        let Value::Record(fields) = &commit else { continue };
        if let Some(Value::Thunk(files)) = fields.get("files") {
            // `new` can inherit an unchanged snapshot from another commit.
            // Its stored origin still answers for those files even when the
            // new commit has a different id.
            if files.origin().and_then(|origin| interp.backend.has_conflict(origin)) == Some(false) {
                continue;
            }
        }
        for entry in commit.field("files")?.as_list()? {
            if matches!(entry.field("content")?, Value::Blob(blob) if blob.is_unresolved()) {
                let path = entry.field("path")?.as_list()?.iter()
                    .map(|part| part.as_text().map(str::to_owned))
                    .collect::<Result<Vec<_>, _>>()?.join("/");
                paths.insert((id.to_string(), path));
            }
        }
    }
    Ok(paths)
}

fn report_conflicts(interp: &j::eval::Interp, before: &Value, after: &Value) {
    let summary = || -> Result<(), Crash> {
        let old = conflict_paths(interp, before)?;
        let new = conflict_paths(interp, after)?;
        let Value::Id(focus) = after.field("root")?.field("id")? else { return Ok(()) };
        let relevant: Vec<_> = new.iter()
            .filter(|(id, path)| id == focus.as_str() || !old.contains(&(id.clone(), path.clone())))
            .collect();
        if relevant.is_empty() {
            return Ok(());
        }
        j::report!("j: warning: {} unresolved file(s) remain:", relevant.len());
        for (id, path) in relevant.iter().take(10) {
            j::report!("   @{}  {}", interp.unique_prefix(id), path.escape_debug());
        }
        if relevant.len() > 10 {
            j::report!("   … and {} more; inspect with `j conflicts`.", relevant.len() - 10);
        }
        j::report!("   Run `j conflicts` to inspect affected commits. In the focused commit, edit the conflict markers, run `j id` to record the resolution, then `j conflicts` to verify.\n   For another commit, first use `j by @ID`. See `j help conflicts`.");
        Ok(())
    };
    // Saving has succeeded: an optional diagnostic must not turn that into a
    // failed operation that the user might retry.
    if let Err(c) = summary() {
        j::report!("j: changes saved, but conflict details could not be read: {}", c.msg);
    }
}

/// Summaries inspect only commit metadata, never force file blobs. Use the
/// resulting tree's ids, including any commits created by this expression.
fn focus_summary(repo: &Value) -> String {
    let summary = || -> Result<String, Crash> {
        let focus = repo.field("root")?;
        let Value::Id(id) = focus.field("id")? else {
            return Err(Crash::new("focus has no id"));
        };
        let mut ids: Vec<String> = j::repo::all_commits(repo)?
            .into_iter()
            .filter_map(|commit| match commit.field("id").ok()? {
                Value::Id(id) => Some(id.to_string()),
                _ => None,
            })
            .collect();
        ids.sort();
        let prefix = j::eval::unique_prefix_in(&ids, &id);
        let message = focus.field("message")?.as_text()?.to_string();
        let title: String = message.lines().next().unwrap_or("").chars()
            .map(|ch| if ch.is_control() { ' ' } else { ch }).take(72).collect();
        let title = if title.trim().is_empty() { "(no description)" } else { title.trim() };
        Ok(format!("focus @{} {}", prefix, title))
    };
    summary().unwrap_or_else(|_| "repository saved".to_string())
}

fn is_repo_value(v: &Value) -> bool {
    // §4.4: shape Repo and root's shape Commit
    let fields = match v {
        Value::Record(m) => m.keys().cloned().collect::<BTreeSet<String>>(),
        _ => return false,
    };
    let want: BTreeSet<String> = ["children", "context", "root"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if fields != want {
        return false;
    }
    let root = match v.field("root") {
        Ok(r) => r,
        Err(_) => return false,
    };
    let root_fields = match &root {
        Value::Record(m) => m.keys().cloned().collect::<BTreeSet<String>>(),
        _ => return false,
    };
    let want_root: BTreeSet<String> = ["files", "id", "labels", "message"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    root_fields == want_root
}

#[cfg(test)]
mod tests {
    #[test]
    fn default_config_does_not_replace_a_file_created_since_the_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.j");
        std::fs::write(&path, "user-owned configuration").unwrap();
        let error = super::write_default_config(path.to_str().unwrap()).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "user-owned configuration");
    }
}
