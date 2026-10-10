//! Interactive presentation uses real terminal descriptors; pipes and opt-outs
//! must keep the CLI suitable for scripts (§1.4).

#![cfg(unix)]

use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

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
fn pty() -> (File, File) {
    let (mut master, mut slave): (RawFd, RawFd) = (-1, -1);
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
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

fn capture(mut command: Command, stdout_tty: bool, stderr_tty: bool) -> Out {
    let stdout = stdout_tty.then(pty).map(|(master, slave)| {
        command.stdout(slave);
        master
    });
    let stderr = stderr_tty.then(pty).map(|(master, slave)| {
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
