//! j — a functional-programming-centered CLI for jj repositories (§1).

use j::config::{self, Config};
use j::jj::JjBackend;
use j::parse::parse_expr;
use j::value::{Crash, Value};
use std::collections::BTreeSet;
use std::io::{IsTerminal, Read};
use std::process::ExitCode;
use std::rc::Rc;

const USAGE: &str = "usage: j EXPRESSION…   (or echo EXPRESSION | j)";

fn err(status: u8, msg: impl std::fmt::Display) -> ExitCode {
    j::report!("j: {}", msg);
    ExitCode::from(status)
}

/// Report a failure that ends the process, keeping the status as a number so
/// callers do not have to recover it from an `ExitCode`.
fn fail(status: u8, msg: impl std::fmt::Display) -> u8 {
    j::report!("j: {}", msg);
    status
}

/// Write a run's output to stdout (§1.2.9, `ops`). A reader that stops early
/// (`j tree | head`) closes the pipe; that is not a failure (§1.4), so the
/// rest is dropped and the run still succeeds. Any other write failure is an
/// error line. `print!` would panic on either.
fn print_output(s: &str) -> ExitCode {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    match out.write_all(s.as_bytes()).and_then(|()| out.flush()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => err(1, format!("cannot write output: {}", e)),
    }
}

fn crash_err(c: &Crash, expr_text: Option<&str>) -> ExitCode {
    j::report!("j: crash: {}", c.msg);
    match (&c.def, expr_text) {
        (Some(d), Some(e)) => j::report!("   in {}, from {}", d, e),
        (None, Some(e)) => j::report!("   from {}", e),
        (Some(d), None) => j::report!("   in {}", d),
        (None, None) => {}
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
        Err(_) => {
            // No user config yet: materialise the default there, editable by
            // the current user, so they have a starting point to customise.
            if let Err(e) = write_default_config(&path) {
                return Err(fail(3, format!("cannot create config at {}: {}", path, e)));
            }
            j::report!("j: created an editable default config at {}", path);
            match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(e) => return Err(fail(3, format!("cannot read config at {}: {}", path, e))),
            }
        }
    };
    match config::load_config(&src) {
        Ok(c) => Ok((c, src)),
        Err(e) => Err(fail(3, e.message())),
    }
}

/// Write the embedded default config to `path`, creating parent directories
/// and making the file readable and writable by the current user.
fn write_default_config(path: &str) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, DEFAULT_CONFIG)?;
    // user-read+write (0o600 on unix): the file is meant to be edited by them
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

const RESERVED: &[&str] = &["init", "clone", "remote", "fetch", "push", "undo", "redo", "ops"];

fn main() -> ExitCode {
    // evaluation builds deep continuation chains (trampolined, but the final
    // continuation still tears down by recursive Drop), so run the real work
    // on a thread with a large stack to keep very deep histories safe
    std::thread::Builder::new()
        .stack_size(512 * 1024 * 1024)
        .spawn(run)
        .expect("failed to spawn worker thread")
        .join()
        .expect("worker thread panicked")
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
    let args: Vec<String> = std::env::args().skip(1).collect();
    // §1: stdin vs arguments. With no arguments the expression comes from
    // stdin, so reading to end-of-input is the whole point. With arguments,
    // stdin is only read to report the "both" error — and reading it blindly
    // hangs whenever the process inherited a pipe nobody is writing to (a
    // shell loop, a CI step), so wait only briefly for input to appear.
    let mut stdin_content = String::new();
    let stdin_has_data = {
        if std::io::stdin().is_terminal() {
            false
        } else if args.is_empty() || stdin_is_ready(50) {
            match std::io::stdin().read_to_string(&mut stdin_content) {
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
    let text = if stdin_has_data {
        stdin_content
    } else if !args.is_empty() {
        args.join(" ")
    } else {
        j::report!("{}", USAGE);
        return ExitCode::from(2);
    };

    // §1.1: reserved commands
    let trimmed = text.trim();
    let first = trimmed.split_whitespace().next().unwrap_or("");
    if RESERVED.contains(&first) {
        return run_reserved(first, trimmed);
    }

    run_expression(trimmed, true)
}

fn run_reserved(cmd: &str, text: &str) -> ExitCode {
    let words: Vec<&str> = text.split_whitespace().collect();
    match cmd {
        "init" => {
            if words.len() != 1 {
                return err(2, "usage: j init");
            }
            let (cfg, _) = or_exit(load_config_file());
            match j::jj::cmd_init(&cfg) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => err(e.0, e.1),
            }
        }
        "clone" => {
            if words.len() != 2 && words.len() != 3 {
                return err(2, "usage: j clone URL [DIR]");
            }
            let (cfg, _) = or_exit(load_config_file());
            let dir = if words.len() == 3 {
                words[2].to_string()
            } else {
                default_clone_dir(words[1])
            };
            match j::jj::cmd_clone(&cfg, words[1], &dir) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => err(e.0, e.1),
            }
        }
        "remote" => {
            if words.len() != 2 {
                return err(2, "usage: j remote URL");
            }
            let backend = or_exit(open_repo());
            match backend.cmd_remote(words[1]) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => err(e.0, e.1),
            }
        }
        "fetch" => {
            if words.len() != 1 {
                return err(2, "usage: j fetch");
            }
            let (cfg, _) = or_exit(load_config_file());
            let backend = or_exit(open_repo_locked(&cfg));
            match backend.cmd_fetch() {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => err(e.0, e.1),
            }
        }
        "push" => {
            let rest = text.trim_start_matches("push").trim();
            if rest.is_empty() {
                return err(2, "usage: j push EXPR");
            }
            let (mut cfg, _) = or_exit(load_config_file());
            let backend = or_exit(open_repo_locked(&cfg));
            // evaluate EXPR against the recorded repository (no snapshot)
            match backend.cmd_push(&mut cfg, rest) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => err(e.0, e.1),
            }
        }
        "undo" => {
            if words.len() != 1 {
                return err(2, "usage: j undo");
            }
            let backend = or_exit(open_repo_noconfig());
            match backend.cmd_undo(false) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => err(e.0, e.1),
            }
        }
        "redo" => {
            if words.len() != 1 {
                return err(2, "usage: j redo");
            }
            let backend = or_exit(open_repo_noconfig());
            match backend.cmd_undo(true) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => err(e.0, e.1),
            }
        }
        "ops" => {
            if words.len() != 1 {
                return err(2, "usage: j ops");
            }
            let backend = or_exit(open_repo_noconfig());
            match backend.cmd_ops() {
                Ok(log) => print_output(&log),
                Err(e) => err(e.0, e.1),
            }
        }
        _ => unreachable!(),
    }
}

fn default_clone_dir(url: &str) -> String {
    let last = url.trim_end_matches('/').rsplit('/').next().unwrap_or("repo");
    last.strip_suffix(".git").unwrap_or(last).to_string()
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
        Err(p) => return err(3, format!("line {}: {}", p.line, p.msg)),
    };

    // §1.2.4: build the current repository value (with snapshot)
    let (mut interp, loaded_repo, current_repo) =
        match backend.build_interp(&mut cfg, text, snapshot) {
            Ok(t) => t,
            Err((2, m)) => return err(2, m),
            Err((3, m)) => return err(3, m),
            Err((_, m)) => return err(1, m),
        };

    // §1.2.5: the config's Id literals were resolved by build_interp, as soon
    // as the visible ids were known; the expression's are resolved here
    expr = match config::resolve_ids(&expr, &interp) {
        Ok(e) => e,
        Err(failures) => {
            let (prefix, candidates) = &failures[0];
            if candidates.is_empty() {
                j::report!("j: crash: `@{}` matches no commit", prefix);
            } else {
                j::report!("j: crash: `@{}` is ambiguous", prefix);
                for c in candidates {
                    j::report!("   {}", c);
                }
            }
            return ExitCode::from(1);
        }
    };

    // §1.2.6: evaluate config definitions, then the expression
    if let Err(c) = config::eval_config(&mut interp, &cfg) {
        return err(3, format!("config.j: {}", c.msg));
    }
    *interp.old_repo.borrow_mut() = Some(loaded_repo.clone());
    let env = interp.global_env();
    let expr_rc = Rc::new(expr);
    let mut v = match interp.eval(&expr_rc, &env) {
        Ok(v) => v,
        Err(c) => return crash_err(&c, Some(text)),
    };

    // §1.2.7: apply a function result to the repo, once
    if matches!(v, Value::Fun(_)) {
        v = match interp.apply(v, current_repo.clone()) {
            Ok(v) => v,
            Err(c) => return crash_err(&c, Some(text)),
        };
    }

    // §1.2.8: a Repo is persisted (unless unchanged)
    if is_repo_value(&v) {
        match backend.persist(&mut interp, &cfg, &loaded_repo, &current_repo, &v, text) {
            Ok(()) => ExitCode::SUCCESS,
            Err(c) => crash_err(&c, Some(text)),
        }
    } else {
        // §1.2.9: display
        let color = j::render::color_enabled("auto");
        match j::render::display(&mut interp, &v, color) {
            Ok(s) => print_output(&s),
            Err(c) => crash_err(&c, Some(text)),
        }
    }
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
