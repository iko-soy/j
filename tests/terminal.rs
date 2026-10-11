//! Interactive presentation uses real terminal descriptors; pipes stay quiet,
//! and color opt-outs preserve useful plain confirmations (§1.4–1.5).

#![cfg(unix)]

use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use unicode_width::UnicodeWidthStr;

const CLEAR_LINE: &str = "\r\x1b[2K";
// A bounded builtin fold keeps evaluation active for multiple progress frames
// without constructing a deep recursive value or touching the network.
const SLOW_SUM: &str = "foldl (+) 0 (range 0 500000)";

struct Fixture {
    root: tempfile::TempDir,
    dir: PathBuf,
    cfg: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("work");
        let cfg = root.path().join("config");
        std::fs::create_dir(&dir).unwrap();
        std::fs::create_dir_all(cfg.join("j")).unwrap();
        let config = include_str!("../config.j")
            .replace("Your Name", "Terminal Test")
            .replace("you@example.com", "terminal@example.com");
        std::fs::write(cfg.join("j/config.j"), config).unwrap();
        Self { root, dir, cfg }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_j"));
        command
            .args(args)
            .current_dir(&self.dir)
            .env("XDG_CONFIG_HOME", &self.cfg)
            .env("TERM", "xterm-256color")
            .env_remove("NO_COLOR")
            .env_remove("CI")
            .env_remove("J_ANIMATIONS")
            .env_remove("J_GIT")
            .stdin(Stdio::null());
        command
    }

    fn init(&self) {
        let out = self.command(&["init"]).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }

    fn slow_clone(&self, fail: bool) -> Command {
        let remote = self.root.path().join("remote.git");
        let out = Command::new("git")
            .args(["init", "--bare", "-q"])
            .arg(&remote)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        // Clone installs the remote during this run, so it uses the fetch
        // subprocess controlled by J_GIT. The fixture never contacts a network.
        let stub = self.root.path().join("slow-git");
        let failure = if fail {
            "echo 'terminal fixture failure' >&2; exit 1;"
        } else {
            ""
        };
        std::fs::write(&stub, format!(
            "#!/bin/sh\ncase \" $* \" in *\" fetch \"*) sleep 0.6; {failure} ;; esac\nexec git \"$@\"\n",
        )).unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut command = self.command(&["clone", remote.to_str().unwrap(), "clone"]);
        command.env("J_GIT", stub);
        command
    }
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

/// Return the master to capture output and the slave to install as stdout or
/// stderr. Disable newline translation so assertions see the bytes j wrote.
fn pty(columns: u16) -> (File, File) {
    let (mut master, mut slave): (RawFd, RawFd) = (-1, -1);
    let size = libc::winsize {
        ws_row: 24,
        ws_col: columns,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                &size,
            )
        },
        0,
        "openpty: {}",
        io::Error::last_os_error()
    );
    let files = unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) };
    for fd in [master, slave] {
        assert_ne!(unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) }, -1);
    }
    let mut settings = std::mem::MaybeUninit::<libc::termios>::uninit();
    assert_eq!(unsafe { libc::tcgetattr(slave, settings.as_mut_ptr()) }, 0);
    let mut settings = unsafe { settings.assume_init() };
    settings.c_oflag &= !libc::OPOST;
    assert_eq!(unsafe { libc::tcsetattr(slave, libc::TCSANOW, &settings) }, 0);
    files
}

fn read_stream(mut reader: Box<dyn Read + Send>) -> String {
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => bytes.extend_from_slice(&buffer[..n]),
            // Linux PTYs report EIO after their last slave has closed.
            Err(e) if e.raw_os_error() == Some(libc::EIO) => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => panic!("reading subprocess output: {e}"),
        }
    }
    String::from_utf8(bytes).unwrap()
}

fn capture(command: Command, stdout_tty: bool, stderr_tty: bool) -> Out {
    capture_with_stderr_width(command, stdout_tty, stderr_tty, 80)
}

fn capture_with_stderr_width(
    mut command: Command,
    stdout_tty: bool,
    stderr_tty: bool,
    columns: u16,
) -> Out {
    let stdout = stdout_tty.then(|| pty(80)).map(|(master, slave)| {
        command.stdout(slave);
        master
    });
    let stderr = stderr_tty.then(|| pty(columns)).map(|(master, slave)| {
        command.stderr(slave);
        master
    });
    if !stdout_tty {
        command.stdout(Stdio::piped());
    }
    if !stderr_tty {
        command.stderr(Stdio::piped());
    }
    let mut child = command.spawn().unwrap();
    drop(command); // Close the parent's slave descriptors so readers reach EOF.
    let stdout: Box<dyn Read + Send> = match stdout {
        Some(master) => Box::new(master),
        None => Box::new(child.stdout.take().unwrap()),
    };
    let stderr: Box<dyn Read + Send> = match stderr {
        Some(master) => Box::new(master),
        None => Box::new(child.stderr.take().unwrap()),
    };
    let stdout = std::thread::spawn(move || read_stream(stdout));
    let stderr = std::thread::spawn(move || read_stream(stderr));
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > std::time::Duration::from_secs(15) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("terminal subprocess did not exit within 15 seconds");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    Out {
        code: status.code().unwrap_or(-1),
        stdout: stdout.join().unwrap(),
        stderr: stderr.join().unwrap(),
    }
}

fn assert_no_motion(out: &Out) {
    for text in [&out.stdout, &out.stderr] {
        assert!(!text.contains('\r'), "unexpected terminal animation: {text:?}");
        assert!(!text.contains("\x1b[2K"), "unexpected line clearing: {text:?}");
        assert!(!text.contains("\x1b[?25"), "unexpected cursor control: {text:?}");
    }
}

fn without_sgr(text: &str) -> String {
    let mut plain = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            assert_eq!(chars.next(), Some('['), "unexpected escape: {text:?}");
            for parameter in chars.by_ref() {
                if parameter == 'm' {
                    break;
                }
                assert!(parameter.is_ascii_digit() || parameter == ';');
            }
        } else {
            plain.push(c);
        }
    }
    plain
}

fn assert_progress(out: &Out, label: &str) {
    let frames: Vec<_> = out.stderr.split(CLEAR_LINE)
        .filter(|frame| frame.contains(label))
        .map(without_sgr)
        .collect();
    assert!(frames.len() >= 2, "no repeated {label} frames: {:?}", out.stderr);
    let mut bars = std::collections::BTreeSet::new();
    for frame in &frames {
        let (_, details) = frame.split_once(" [")
            .unwrap_or_else(|| panic!("missing activity bar: {frame:?}"));
        let (bar, elapsed) = details.split_once("] ")
            .unwrap_or_else(|| panic!("missing elapsed time: {frame:?}"));
        assert!(bar.contains('━') && bar.contains('·'), "invalid activity bar: {bar:?}");
        assert!(elapsed.strip_suffix('s').and_then(|s| s.parse::<f64>().ok()).is_some(),
            "invalid elapsed time: {elapsed:?}");
        bars.insert(bar);
    }
    assert!(bars.len() >= 2, "activity bar did not animate: {frames:?}");
}

#[test]
fn terminal_tree_is_colored_and_raw_text_is_unchanged() {
    let fixture = Fixture::new();
    fixture.init();
    let tree = capture(fixture.command(&["tree"]), true, true);
    assert_eq!(tree.code, 0, "{}", tree.stderr);
    assert!(tree.stdout.contains("\x1b["), "tree has no color: {:?}", tree.stdout);

    let expr = "\"a rainbow\\nkeeps its bytes\\n\"";
    let plain = capture(fixture.command(&[expr]), false, false);
    let terminal = capture(fixture.command(&[expr]), true, true);
    assert_eq!(plain.code, 0, "{}", plain.stderr);
    assert_eq!(terminal.code, 0, "{}", terminal.stderr);
    assert_eq!(terminal.stdout, plain.stdout);
    assert_eq!(terminal.stdout, "a rainbow\nkeeps its bytes\n");
}

#[test]
fn slow_expression_animates_and_clears_before_reporting_failure() {
    let fixture = Fixture::new();
    fixture.init();
    let expr = format!(
        "if {SLOW_SUM} == 124999750000 then crash \"expression fixture failure\" else \"\""
    );
    let out = capture(fixture.command(&[&expr]), true, true);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stdout.is_empty(), "progress leaked into stdout: {:?}", out.stdout);
    assert_progress(&out, "Evaluating expression");
    let last_clear = out.stderr.rfind(CLEAR_LINE).expect("progress was never cleared");
    let diagnostic = without_sgr(&out.stderr[last_clear + CLEAR_LINE.len()..]);
    assert!(diagnostic.starts_with("j:"), "progress label was not cleared: {diagnostic:?}");
    assert!(diagnostic.contains("expression fixture failure"), "{diagnostic:?}");
    assert!(diagnostic.ends_with('\n'), "diagnostic did not end a line: {diagnostic:?}");
}

#[test]
fn slow_implicit_application_preserves_output_and_respects_opt_outs() {
    let fixture = Fixture::new();
    fixture.init();
    // Evaluating this lambda is fast; applying it to the implicit repository
    // must remain inside the same progress phase.
    let expr = format!(
        "\\repo -> if {SLOW_SUM} == 124999750000 then \"computed\\nunchanged\\n\" else crash \"unexpected sum\""
    );
    let animated = capture(fixture.command(&[&expr]), true, true);
    assert_eq!(animated.code, 0, "{}", animated.stderr);
    assert_eq!(animated.stdout, "computed\nunchanged\n");
    assert_progress(&animated, "Evaluating expression");
    assert!(animated.stderr.ends_with(CLEAR_LINE), "progress was not cleared: {:?}", animated.stderr);

    let mut command = fixture.command(&[&expr]);
    command.env("J_ANIMATIONS", "off");
    let opted_out = capture(command, true, true);
    let piped = capture(fixture.command(&[&expr]), false, false);
    for out in [opted_out, piped] {
        assert_eq!(out.code, 0, "{}", out.stderr);
        assert_eq!(out.stdout, animated.stdout);
        assert_no_motion(&out);
    }
}

#[test]
fn slow_configuration_animates_and_clears_before_output() {
    let fixture = Fixture::new();
    fixture.init();
    let path = fixture.cfg.join("j/config.j");
    let mut config = std::fs::read_to_string(&path).unwrap();
    config.push_str(&format!("\nterminalWork = {SLOW_SUM}\n"));
    std::fs::write(path, config).unwrap();
    let out = capture(fixture.command(&["show terminalWork"]), true, true);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stdout, "124999750000\n");
    assert_progress(&out, "Evaluating configuration");
    assert!(out.stderr.ends_with(CLEAR_LINE), "progress was not cleared: {:?}", out.stderr);
}

#[test]
fn slow_work_animates_and_clears_before_reporting_failure() {
    let fixture = Fixture::new();
    let mut command = fixture.slow_clone(true);
    // Empty opt-out variables are not opt-outs.
    command.env("NO_COLOR", "").env("CI", "");
    let out = capture(command, true, true);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stdout.is_empty(), "spinner leaked into stdout: {:?}", out.stdout);
    let clear = "\r\x1b[2K";
    assert!(out.stderr.matches(clear).count() >= 3, "no repeated frames: {:?}", out.stderr);
    let diagnostic = out.stderr.find("j:").expect("missing diagnostic prefix");
    let last_clear = out.stderr.rfind(clear).expect("spinner was never cleared");
    assert!(last_clear < diagnostic, "animation continued over diagnostic: {:?}", out.stderr);
    let final_line = without_sgr(&out.stderr[last_clear + clear.len()..]);
    assert!(final_line.starts_with("j:"), "spinner label was not cleared: {final_line:?}");
    assert!(final_line.contains("terminal fixture failure"));
    assert!(out.stderr.ends_with('\n'), "diagnostic did not end a line: {:?}", out.stderr);
}

#[test]
fn successful_slow_work_clears_before_its_confirmation() {
    let fixture = Fixture::new();
    let out = capture(fixture.slow_clone(false), true, true);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.is_empty(), "spinner leaked into stdout: {:?}", out.stdout);
    let clear = "\r\x1b[2K";
    assert!(out.stderr.matches(clear).count() >= 3, "no repeated frames: {:?}", out.stderr);
    let last_clear = out.stderr.rfind(clear).expect("spinner was never cleared");
    let final_line = without_sgr(&out.stderr[last_clear + clear.len()..]);
    assert!(final_line.starts_with("✓"), "spinner label was not cleared: {final_line:?}");
    assert!(final_line.contains("j:"), "missing success confirmation: {final_line:?}");
    assert!(fixture.dir.join("clone/.jj").is_dir());
}

#[test]
fn progress_fits_a_narrow_stderr_terminal() {
    let fixture = Fixture::new();
    // stdout stays 80 columns wide: progress must measure its own stderr.
    let out = capture_with_stderr_width(fixture.slow_clone(false), true, true, 18);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let segments: Vec<_> = out.stderr.split(CLEAR_LINE).skip(1).collect();
    let (confirmation, frames) = segments.split_last().expect("progress was never cleared");
    assert!(frames.len() >= 2, "no repeated frames: {:?}", out.stderr);
    for frame in frames {
        let plain = without_sgr(frame);
        assert!(!plain.contains('\n'), "progress used multiple rows: {plain:?}");
        assert!(plain.width() <= 17, "progress wrapped the 18-column terminal: {plain:?}");
    }
    assert!(without_sgr(confirmation).starts_with('✓'), "confirmation was not preserved: {confirmation:?}");
    assert!(out.stdout.is_empty(), "progress leaked into stdout: {:?}", out.stdout);
}

#[test]
fn inherited_difft_diagnostic_is_not_erased_by_progress() {
    let fixture = Fixture::new();
    fixture.init();
    let bin = fixture.root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let stub = bin.join("difft");
    std::fs::write(
        &stub,
        "#!/bin/sh\nsleep 0.3\nprintf 'external difft diagnostic' >&2\nsleep 0.4\nexit 2\n",
    ).unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let inherited_path = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&inherited_path)),
    ).unwrap();
    // Push evaluates its expression inside an active progress phase. A difft
    // that exits unsuccessfully still returns its stdout (§7.10), so deliberately
    // fail the expression afterwards, before any remote is consulted.
    let mut command = fixture.command(&[
        "push (if difft ./f (blob \"a\") (blob \"b\") == \"\" then crash \"push fixture failure\" else [])",
    ]);
    command.env("PATH", path);
    let out = capture(command, true, true);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stdout.is_empty(), "unexpected stdout: {:?}", out.stdout);
    let diagnostic = out.stderr.find("external difft diagnostic")
        .expect("inherited difft diagnostic was lost");
    let after = &out.stderr[diagnostic..];
    assert!(!after.contains('\r'), "spinner overwrote external diagnostic: {after:?}");
    assert!(!after.contains("\x1b[2K"), "spinner cleared external diagnostic: {after:?}");
    assert!(after.contains("push fixture failure"), "push failure was lost: {after:?}");
}

#[test]
fn terminal_motion_respects_environment_opt_outs() {
    let fixture = Fixture::new();
    for (key, value) in [
        ("NO_COLOR", "1"),
        ("TERM", "dumb"),
        ("CI", "1"),
        ("J_ANIMATIONS", "never"),
        ("J_ANIMATIONS", "0"),
        ("J_ANIMATIONS", "false"),
        ("J_ANIMATIONS", "off"),
    ] {
        let mut command = fixture.slow_clone(true);
        command.env(key, value);
        let out = capture(command, true, true);
        assert_eq!(out.code, 1, "{key}={value}: {}", out.stderr);
        assert!(out.stderr.contains("terminal fixture failure"), "{}", out.stderr);
        assert_no_motion(&out);
        if key == "NO_COLOR" || key == "TERM" {
            assert!(!out.stderr.contains('\x1b'), "{key}={value}: {:?}", out.stderr);
        } else if key == "J_ANIMATIONS" {
            assert!(out.stderr.contains("\x1b["), "motion opt-out also disabled color");
        }
    }
}

#[test]
fn color_opt_outs_keep_plain_success_confirmations() {
    for (key, value) in [("NO_COLOR", "1"), ("TERM", "dumb")] {
        let fixture = Fixture::new();
        let mut command = fixture.slow_clone(false);
        command.env(key, value);
        let out = capture(command, true, true);
        assert_eq!(out.code, 0, "{key}={value}: {}", out.stderr);
        assert!(out.stdout.is_empty(), "unexpected stdout: {:?}", out.stdout);
        assert!(out.stderr.contains("j: Repository cloned"), "missing success confirmation: {:?}", out.stderr);
        assert!(!out.stderr.contains('\x1b'), "{key}={value}: {:?}", out.stderr);
        assert_no_motion(&out);
        assert!(fixture.dir.join("clone/.jj").is_dir());
    }
}

#[test]
fn noninteractive_runs_keep_success_confirmations_quiet() {
    let fixture = Fixture::new();
    fixture.init();
    for (stdout_tty, stderr_tty) in [(false, true), (true, false), (false, false)] {
        let out = capture(fixture.command(&["id"]), stdout_tty, stderr_tty);
        assert_eq!(out.code, 0, "{}", out.stderr);
        assert!(out.stdout.is_empty(), "unexpected stdout: {:?}", out.stdout);
        assert!(out.stderr.is_empty(), "unexpected confirmation: {:?}", out.stderr);
    }
    let mut command = fixture.command(&["id"]);
    command.env("CI", "1");
    let out = capture(command, true, true);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.is_empty(), "unexpected stdout: {:?}", out.stdout);
    assert!(out.stderr.is_empty(), "unexpected CI confirmation: {:?}", out.stderr);
}

#[test]
fn edit_confirmations_identify_changes_and_the_resulting_focus() {
    let fixture = Fixture::new();
    fixture.init();
    let confirmation = |expression: &str, expected: &str, description: &str| {
        let mut command = fixture.command(&[expression]);
        command.env("NO_COLOR", "1");
        let out = capture(command, true, true);
        assert_eq!(out.code, 0, "{}", out.stderr);
        assert!(out.stdout.is_empty(), "unexpected stdout: {:?}", out.stdout);
        assert_no_motion(&out);
        assert!(out.stderr.starts_with(&format!("j: {expected}; focus @")), "{}", out.stderr);
        assert!(out.stderr.ends_with(&format!(" {description}\n")), "{}", out.stderr);
        let focus = out.stderr.split("; focus ").nth(1).unwrap()
            .split_whitespace().next().unwrap().to_owned();
        assert!(focus.len() >= 5, "focus prefix is too short: {focus}");
        let full = fixture.command(&["\\r -> show r.root.id"]).output().unwrap();
        assert!(full.status.success(), "{}", String::from_utf8_lossy(&full.stderr));
        assert!(String::from_utf8(full.stdout).unwrap().starts_with(&focus));
        focus
    };
    let initial = confirmation("describe \"Fix login\"", "Saved changes", "Fix login");
    assert_eq!(confirmation("describe \"Fix login\"", "No changes", "Fix login"), initial);
    let next = confirmation("new", "Saved changes", "(no description)");
    assert_ne!(next, initial, "new did not report the resulting focus");
    std::fs::write(fixture.dir.join("file.txt"), "a working-copy change\n").unwrap();
    assert_eq!(confirmation("id", "Saved changes", "(no description)"), next);
    assert_eq!(confirmation("id", "No changes", "(no description)"), next);
}

#[test]
fn conflicting_edits_report_paths_and_a_working_recovery_sequence() {
    let fixture = Fixture::new();
    fixture.init();
    let run = |expr: &str| {
        let out = fixture.command(&[expr]).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        out
    };
    std::fs::write(fixture.dir.join("settings.txt"), "base\n").unwrap();
    run("describe \"base\"");
    run("new");
    std::fs::write(fixture.dir.join("settings.txt"), "left\n").unwrap();
    run("describe \"left\"");
    run("new . prev");
    std::fs::write(fixture.dir.join("settings.txt"), "right\n").unwrap();
    run("describe \"right\"");

    let mut command = fixture.command(&["rebase siblings"]);
    command.env("NO_COLOR", "1");
    let out = capture(command, true, true);
    assert_eq!(out.code, 0, "{}", out.stderr);
    for expected in ["Saved changes", "1 unresolved file(s)", "settings.txt", "j conflicts", "j id"] {
        assert!(out.stderr.contains(expected), "missing {expected}: {}", out.stderr);
    }
    assert!(std::fs::read_to_string(fixture.dir.join("settings.txt")).unwrap().contains("<<<<<<<"));
    assert_no_motion(&out);

    std::fs::write(fixture.dir.join("settings.txt"), "resolved\n").unwrap();
    let out = run("save");
    assert!(out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8(run("show . conflicts").stdout).unwrap().trim(), "[]");
}

#[test]
fn empty_push_explains_that_nothing_was_published() {
    let fixture = Fixture::new();
    fixture.init();
    let mut command = fixture.command(&["push relabel"]);
    command.env("NO_COLOR", "1");
    let out = capture(command, true, true);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stderr.contains("No bookmarks selected; nothing published"), "{}", out.stderr);
    assert!(out.stderr.contains("push (label \"feature\" here)"), "{}", out.stderr);
    assert!(!out.stderr.contains("Push complete"), "{}", out.stderr);
    assert_no_motion(&out);
}

#[test]
fn push_confirmations_count_published_unchanged_and_deleted_bookmarks() {
    let fixture = Fixture::new();
    fixture.init();
    let remote = fixture.root.path().join("origin.git");
    let output = Command::new("git").args(["init", "--bare", "-q"])
        .arg(&remote).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    for args in [vec!["describe \"feature\""], vec!["remote", remote.to_str().unwrap()]] {
        let output = fixture.command(&args).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }
    for (expression, counts) in [
        ("push (label \"feature\" here)", "1 published, 0 deleted, 0 unchanged"),
        ("push (label \"feature\" here)", "0 published, 0 deleted, 1 unchanged"),
        ("push (unlabel \"feature\")", "0 published, 1 deleted, 0 unchanged"),
    ] {
        let mut command = fixture.command(&[expression]);
        command.env("NO_COLOR", "1");
        let out = capture(command, true, true);
        assert_eq!(out.code, 0, "{}", out.stderr);
        assert!(out.stderr.contains(counts), "{}", out.stderr);
        assert_no_motion(&out);
    }
}

#[test]
fn undo_redo_confirmations_name_the_affected_edit_and_restored_focus() {
    let fixture = Fixture::new();
    fixture.init();
    let focus_id = || {
        let out = fixture.command(&["\\r -> show r.root.id"]).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    let original_id = focus_id();
    let expression = "describe \"Next task\" . new";
    let edit = fixture.command(&[expression]).output().unwrap();
    assert!(edit.status.success(), "{}", String::from_utf8_lossy(&edit.stderr));
    let next_id = focus_id();
    assert_ne!(original_id, next_id);
    for (command, action, id, title) in [
        ("undo", "Undid", &original_id, "(no description)"),
        ("redo", "Redid", &next_id, "Next task"),
        ("undo", "Undid", &original_id, "(no description)"),
        ("redo", "Redid", &next_id, "Next task"),
    ] {
        let mut command = fixture.command(&[command]);
        command.env("NO_COLOR", "1");
        let out = capture(command, true, true);
        assert_eq!(out.code, 0, "{}", out.stderr);
        assert!(out.stdout.is_empty(), "unexpected stdout: {:?}", out.stdout);
        assert_eq!(out.stderr, format!("j: {action} `{expression}`; restored focus {id} {title}\n"));
        assert_eq!(&focus_id(), id);
        assert_no_motion(&out);
    }
}

#[test]
fn redirecting_either_output_stream_disables_motion() {
    let fixture = Fixture::new();
    for (stdout_tty, stderr_tty) in [(false, true), (true, false), (false, false)] {
        let out = capture(fixture.slow_clone(true), stdout_tty, stderr_tty);
        assert_eq!(out.code, 1, "{}", out.stderr);
        assert_no_motion(&out);
        assert!(out.stderr.contains("terminal fixture failure"), "{}", out.stderr);
        if !stderr_tty {
            assert!(!out.stderr.contains('\x1b'), "redirected diagnostic has ANSI: {:?}", out.stderr);
        }
    }
    fixture.init();
    let out = capture(fixture.command(&["tree"]), false, true);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(!out.stdout.contains('\x1b'), "redirected tree has ANSI: {:?}", out.stdout);
    assert_no_motion(&out);
}
