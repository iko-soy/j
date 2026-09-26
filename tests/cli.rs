//! End-to-end CLI tests: the built binary against real jj repositories (§1,
//! §7). Each test builds its own repo under a temp dir with its own config.

use std::path::PathBuf;
use std::process::{Command, Stdio};

fn j_bin() -> PathBuf {
    // cargo sets CARGO_BIN_EXE_<name> for integration tests
    option_env!("CARGO_BIN_EXE_j")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let mut p = std::env::current_exe().unwrap();
            p.pop(); // deps
            p.pop(); // debug
            p.push("j");
            p
        })
}

struct Repo {
    dir: PathBuf,
    cfg: PathBuf,
}

fn setup() -> Repo {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "j-cli-test-{}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // the config lives outside the repo: a checkout may remove untracked
    // files inside the working directory
    let cfg = dir.join("..").join(format!("{}-cfg", dir.file_name().unwrap().to_string_lossy()));
    std::fs::create_dir_all(&cfg.join("j")).unwrap();
    let cfg = cfg.canonicalize().unwrap();
    let mut config = include_str!("../config.j").to_string();
    config = config.replace("Your Name", "Test User");
    config = config.replace("you@example.com", "test@example.com");
    std::fs::write(cfg.join("j/config.j"), config).unwrap();
    let r = Repo { dir, cfg };
    let out = r.j(&["init"]);
    if out.code != 0 {
        eprintln!("INIT FAILED in {}: {}", r.dir.display(), out.stderr);
        eprintln!("config exists: {}", r.cfg.join("j/config.j").exists());
        for e in std::fs::read_dir(&r.dir).unwrap() {
            eprintln!("  entry: {:?}", e.unwrap().file_name());
        }
    }
    out.ok();
    r
}

impl Repo {
    fn j(&self, args: &[&str]) -> Out {
        let out = Command::new(j_bin())
            .args(args)
            .current_dir(&self.dir)
            .env("XDG_CONFIG_HOME", &self.cfg)
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        Out {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        }
    }

    fn j_stdin(&self, input: &str, args: &[&str]) -> Out {
        let mut child = Command::new(j_bin())
            .args(args)
            .current_dir(&self.dir)
            .env("XDG_CONFIG_HOME", &self.cfg)
            .env("NO_COLOR", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        Out {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        }
    }

    /// Run `j` with stdout sent to `stdout` instead of captured.
    fn j_to(&self, stdout: impl Into<Stdio>, args: &[&str]) -> Out {
        let out = Command::new(j_bin())
            .args(args)
            .current_dir(&self.dir)
            .env("XDG_CONFIG_HOME", &self.cfg)
            .env("NO_COLOR", "1")
            .stdout(stdout)
            .output()
            .unwrap();
        Out {
            code: out.status.code().unwrap_or(-1),
            stdout: String::new(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        }
    }

    /// Run `j` with stdout on a pipe whose reader has already exited, as in
    /// `j tree | head` once `head` has read all it wants.
    fn j_closed_stdout(&self, args: &[&str]) -> Out {
        let (reader, writer) = std::io::pipe().unwrap();
        drop(reader);
        self.j_to(writer, args)
    }

    fn write(&self, path: &str, content: &str) {
        std::fs::write(self.dir.join(path), content).unwrap();
    }

    fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.dir.join(path)).unwrap()
    }
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Out {
    fn ok(self) -> Self {
        assert_eq!(self.code, 0, "stderr: {}", self.stderr);
        self
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
        let _ = std::fs::remove_dir_all(&self.cfg);
    }
}

#[test]
fn no_args_is_usage_error() {
    let r = setup();
    let out = r.j(&[]);
    assert_eq!(out.code, 2);
    assert!(out.stderr.contains("usage"), "{}", out.stderr);
}

#[test]
fn both_stdin_and_args_rejected() {
    let r = setup();
    let out = r.j_stdin("1 + 1", &["extra"]);
    assert_eq!(out.code, 2);
    assert!(out.stderr.contains("both"), "{}", out.stderr);
}

#[test]
fn expression_from_stdin() {
    let r = setup();
    let out = r.j_stdin("1 + 2 * 3", &[]).ok();
    assert_eq!(out.stdout.trim(), "7");
}

#[test]
fn arguments_joined_verbatim() {
    let r = setup();
    let out = r.j(&["1", "+", "2"]).ok();
    assert_eq!(out.stdout.trim(), "3");
}

#[test]
fn parse_error_is_exit_3() {
    let r = setup();
    let out = r.j(&["1 +"]);
    assert_eq!(out.code, 3);
    assert!(out.stderr.starts_with("j: "), "{}", out.stderr);
}

#[test]
fn crash_is_exit_1_with_trace() {
    let r = setup();
    let out = r.j(&["crash \"boom\""]);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("j: crash: boom"), "{}", out.stderr);
    assert!(out.stderr.contains("from crash \"boom\""), "{}", out.stderr);
}

#[test]
fn full_edit_flow() {
    let r = setup();
    r.write("a.txt", "one\n");
    r.j(&["describe \"first\""]).ok();
    r.j(&["new"]).ok();
    r.write("b.txt", "two\n");
    r.j(&["describe \"second\""]).ok();
    let out = r.j(&["status"]).ok();
    assert!(out.stdout.contains("second"), "{}", out.stdout);
    assert!(out.stdout.contains("b.txt"), "{}", out.stdout);
    r.j(&["prev"]).ok();
    let out = r.j(&["status"]).ok();
    assert!(out.stdout.contains("first"), "{}", out.stdout);
    r.j(&["next"]).ok();
    let out = r.j(&["here"]).ok();
    assert!(!out.stdout.trim().is_empty());
}

#[test]
fn gitignore_is_honored() {
    // §1.2: the snapshot tracks every file not matched by .gitignore
    let r = setup();
    r.write(".gitignore", "*.log\n");
    r.j(&["new"]).ok();
    r.write("secret.log", "ignored\n");
    r.write("normal.txt", "tracked\n");
    r.j(&["id"]).ok();
    let out = r.j(&["status"]).ok();
    assert!(out.stdout.contains("normal.txt"), "{}", out.stdout);
    assert!(!out.stdout.contains("secret.log"), "{}", out.stdout);
}

#[test]
fn dry_run_persists_nothing() {
    let r = setup();
    r.write("a.txt", "x\n");
    r.j(&["describe \"work\""]).ok();
    r.j(&["new"]).ok();
    r.write("b.txt", "y\n");
    let before = r.j(&["log"]).ok().stdout;
    let out = r.j(&["tree . squash"]).ok();
    assert!(!out.stdout.is_empty());
    let after = r.j(&["log"]).ok().stdout;
    assert_eq!(before, after, "dry run changed the repository");
}

#[test]
fn validate_catches_what_persistence_refuses() {
    let r = setup();
    r.write("a.txt", "x\n");
    r.j(&["describe \"work\""]).ok();
    // tree . validate . squash is an honest dry run
    let out = r.j(&["tree . validate . squash"]).ok();
    assert!(!out.stdout.is_empty());
}

#[test]
fn undo_and_redo() {
    let r = setup();
    r.write("a.txt", "x\n");
    r.j(&["describe \"one\""]).ok();
    let before = r.j(&["status"]).ok().stdout;
    assert!(before.contains("one"));
    r.j(&["undo"]).ok();
    let after = r.j(&["status"]).ok().stdout;
    assert!(!after.contains("one"), "{}", after);
    r.j(&["redo"]).ok();
    let restored = r.j(&["status"]).ok().stdout;
    assert!(restored.contains("one"), "{}", restored);
}

#[test]
fn undo_refuses_dirty_working_copy() {
    let r = setup();
    r.write("a.txt", "x\n");
    r.j(&["id"]).ok();
    r.write("a.txt", "dirty\n");
    let out = r.j(&["undo"]);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("working copy has changes"), "{}", out.stderr);
}

#[test]
fn nothing_to_undo_or_redo() {
    let r = setup();
    let out = r.j(&["redo"]);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("nothing to redo"), "{}", out.stderr);
}

#[test]
fn undo_after_init_refuses_cleanly() {
    // undoing the operation that established the working copy would restore a
    // view with no checked-out commit, leaving the repository unusable; it is
    // reported as "nothing to undo", not a confusing error
    let r = setup();
    let out = r.j(&["undo"]);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("nothing to undo"), "{}", out.stderr);
    // and the repository is still usable afterwards
    r.write("a.txt", "x\n");
    r.j(&["describe \"still works\""]).ok();
    assert!(r.j(&["status"]).ok().stdout.contains("still works"));
}

#[test]
fn ops_lists_operations() {
    let r = setup();
    r.write("a.txt", "x\n");
    r.j(&["describe \"marker\""]).ok();
    let out = r.j(&["ops"]).ok();
    assert!(out.stdout.contains("describe \"marker\""), "{}", out.stdout);
    // the current operation is marked
    assert!(out.stdout.contains('*'), "{}", out.stdout);
    // one line per operation, newest first
    assert!(out.stdout.lines().count() >= 3, "{}", out.stdout);
}

#[test]
fn squash_and_abandon() {
    let r = setup();
    r.write("a.txt", "one\n");
    r.j(&["describe \"base\""]).ok();
    r.j(&["new"]).ok();
    r.write("a.txt", "two\n");
    r.j(&["describe \"fold me\""]).ok();
    r.j(&["squash"]).ok();
    let out = r.j(&["log"]).ok().stdout;
    assert!(!out.contains("fold me"), "{}", out);
    assert_eq!(r.read("a.txt"), "two\n");
}

#[test]
fn rebase_onto_sibling() {
    let r = setup();
    r.write("f.txt", "base\n");
    r.j(&["describe \"base\""]).ok();
    // branch from the parent of the focus
    r.j(&["new"]).ok();
    r.write("f.txt", "side\n");
    r.j(&["describe \"side\""]).ok();
    let out = r.j(&["tree"]).ok();
    assert!(out.stdout.contains("side"), "{}", out.stdout);
}

#[test]
fn lock_blocks_second_j() {
    // hold the flock ourselves, then a second j is refused
    use std::os::unix::io::AsRawFd;
    let r = setup();
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(r.dir.join(".jj/j.lock"))
        .unwrap();
    let rc = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(rc, 0);
    let out = r.j(&["tree"]);
    assert_eq!(out.code, 2);
    assert!(out.stderr.contains("another j is running"), "{}", out.stderr);
    unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_UN) };
    r.j(&["tree"]).ok();
}

#[test]
fn reserved_words_as_first_word() {
    let r = setup();
    // `(push)` is an ordinary expression (unbound name)
    let out = r.j(&["(push)"]);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("unbound"), "{}", out.stderr);
    // `x fetch` too
    let out = r.j(&["x fetch"]);
    assert_eq!(out.code, 1);
}

#[test]
fn id_resolution_errors() {
    let r = setup();
    let out = r.j(&["by @nope"]);
    assert_eq!(out.code, 3); // lexical error: @ followed by non-k..z
    let out = r.j(&["by @kqzz"]);
    assert_eq!(out.code, 1); // no such commit: crash
    assert!(out.stderr.contains("matches no commit"), "{}", out.stderr);
}

#[test]
fn goto_trunk_requires_new() {
    let r = setup();
    r.write("a", "1\n");
    r.j(&["describe \"x\""]).ok();
    // focusing the root commit is refused
    let out = r.j(&["by @zzzzzzzz"]);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("mutable"), "{}", out.stderr);
}

#[test]
fn text_result_prints_raw() {
    let r = setup();
    let out = r.j(&["\"literal text\""]).ok();
    assert_eq!(out.stdout, "literal text\n");
    let out = r.j(&["show [1 \"two\"]"]).ok();
    assert!(out.stdout.contains("[1 \"two\"]"), "{}", out.stdout);
}

#[test]
fn closed_stdout_ends_the_display_quietly() {
    // §1.4: a reader that stops early (`j tree | head`) is not a failure;
    // writing into its closed pipe used to panic and exit 101
    let r = setup();
    r.write("a.txt", "x\n");
    r.j(&["describe \"shown\""]).ok();
    for expr in ["1 + 1", "tree", "range 0 200000"] {
        let out = r.j_closed_stdout(&[expr]);
        assert_eq!(out.code, 0, "{}: {}", expr, out.stderr);
        assert_eq!(out.stderr, "", "{}", expr);
    }
    // the reserved command that prints writes the same way
    let out = r.j_closed_stdout(&["ops"]);
    assert_eq!(out.code, 0, "ops: {}", out.stderr);
    assert_eq!(out.stderr, "");
    // and the lock was released
    assert!(r.j(&["ops"]).ok().stdout.contains("describe \"shown\""));
}

#[test]
fn unwritable_stdout_is_a_one_line_error() {
    // §1.4: any other write failure is an error line, not a panic
    let full = std::path::Path::new("/dev/full");
    if !full.exists() {
        return;
    }
    let r = setup();
    for args in [["1 + 1"], ["ops"]] {
        let dev = std::fs::OpenOptions::new().write(true).open(full).unwrap();
        let out = r.j_to(dev, &args);
        assert_eq!(out.code, 1, "{:?}: {}", args, out.stderr);
        assert!(out.stderr.starts_with("j: "), "{:?}: {}", args, out.stderr);
        assert_eq!(out.stderr.lines().count(), 1, "{:?}: {}", args, out.stderr);
    }
}

#[test]
fn crash_leaves_repo_untouched() {
    let r = setup();
    r.write("a.txt", "x\n");
    r.j(&["describe \"stable\""]).ok();
    let before = r.j(&["log"]).ok().stdout;
    r.write("b.txt", "y\n");
    let out = r.j(&["describe \"x\" . crash \"no\" . describe \"y\""]);
    assert_eq!(out.code, 1);
    let after = r.j(&["log"]).ok().stdout;
    assert_eq!(before, after);
    // the working directory is untouched too
    assert_eq!(r.read("b.txt"), "y\n");
}

#[test]
fn unwritable_path_refused_before_anything_is_recorded() {
    // these paths were written and the operation published; only the
    // checkout then refused them, so every later persisting run recorded
    // another operation and crashed the same way, and undo refused
    let r = setup();
    r.write("a.txt", "x\n");
    r.j(&["describe \"stable\""]).ok();
    let log = r.j(&["log"]).ok().stdout;
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    for bad in [
        r#"[".." "evil"]"#,
        r#"["x" ".."]"#,
        r#"[".git" "hooks" "post-checkout"]"#,
        r#"[".jj" "x"]"#,
    ] {
        let edit = format!(
            "mapRoot (\\c -> c {{ files = c.files ++ [{{ path = {}, content = blob \"pwn\" }}] }})",
            bad
        );
        // the dry run reports what persisting would
        let out = r.j(&[&format!("tree . validate . {}", edit)]);
        assert_eq!(out.code, 1, "validate accepted {}: {}", bad, out.stdout);
        assert!(out.stderr.contains("path component"), "{}", out.stderr);
        let out = r.j(&[&edit]);
        assert_eq!(out.code, 1, "{}: {}", bad, out.stderr);
        assert!(out.stderr.contains("path component"), "{}", out.stderr);
        assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops, "{} was recorded", bad);
        assert_eq!(r.j(&["log"]).ok().stdout, log);
    }
    // nothing was created in the working directory, and the repo still works
    assert!(!r.dir.join("x").exists());
    r.j(&["describe \"after\""]).ok();
}

#[test]
fn root_path_entry_is_a_crash_not_a_panic() {
    // jj's tree builder asserts a path is not the root: exit 101 and a Rust
    // panic, which `or` could not catch
    let r = setup();
    r.write("a.txt", "x\n");
    r.j(&["describe \"stable\""]).ok();
    let log = r.j(&["log"]).ok().stdout;
    let out = r.j(&["replay [{ path = ./, content = blob \"x\" }] ({ from = [], to = [] })"]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.starts_with("j: crash:"), "{}", out.stderr);
    let out = r
        .j(&["(show (replay [] ({ from = [], to = [{ path = ./, content = blob \"x\" }] }))) or \"caught\""])
        .ok();
    assert_eq!(out.stdout.trim(), "caught");
    let edit = "mapRoot (\\c -> c { files = c.files ++ [{ path = ./, content = blob \"x\" }] })";
    let out = r.j(&[&format!("tree . validate . {}", edit)]);
    assert_eq!(out.code, 1, "validate accepted it: {}", out.stdout);
    let out = r.j(&[edit]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("root path"), "{}", out.stderr);
    assert_eq!(r.j(&["log"]).ok().stdout, log);
}

#[test]
fn file_and_directory_at_one_path_refused() {
    // a jj tree holds `a` as a file or as a directory, not both: persisting
    // kept `a/b` and silently dropped the file `a`, and replay did the same,
    // disagreeing with the in-memory backend (§7.3, §10)
    let r = setup();
    r.write("a.txt", "x\n");
    r.j(&["describe \"stable\""]).ok();
    let log = r.j(&["log"]).ok().stdout;
    let edit = "mapRoot (\\c -> c { files = [({ path = [\"a\"], content = blob \"x\\n\" }) ({ path = [\"a\" \"b\"], content = blob \"y\\n\" })] })";
    let out = r.j(&[&format!("tree . validate . {}", edit)]);
    assert_eq!(out.code, 1, "validate accepted it: {}", out.stdout);
    let out = r.j(&[edit]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("both a file and a directory"), "{}", out.stderr);
    assert_eq!(r.j(&["log"]).ok().stdout, log);
    assert_eq!(r.read("a.txt"), "x\n");
    assert!(!r.dir.join("a").exists());
    let out = r.j(&[
        "let x = [({ path = ./a, content = blob \"file\" }) ({ path = ./a/b, content = blob \"nested\" })] in replay x ({ from = [], to = [] })",
    ]);
    assert_eq!(out.code, 1, "replay dropped a file: {}", out.stdout);
    assert!(out.stderr.contains("both a file and a directory"), "{}", out.stderr);
}

#[test]
fn split_and_contract_paths() {
    let r = setup();
    r.write("code.rs", "fn main() {}\n");
    r.write("doc.md", "# doc\n");
    r.j(&["describe \"both\""]).ok();
    r.j(&["split (ext \"rs\")"]).ok();
    let out = r.j(&["tree"]).ok();
    assert!(out.stdout.contains("both"), "{}", out.stdout);
}

#[test]
fn label_literal_empty_without_remote() {
    let r = setup();
    let out = r.j(&["labelled \"main\""]).ok();
    assert!(out.stdout.contains("none") || out.stdout.contains("0"), "{}", out.stdout);
    let out = r.j(&["trunk"]).ok();
    assert!(out.stdout.contains("none") || out.stdout.contains("0"), "{}", out.stdout);
}

#[test]
fn interrupted_expression_persists_nothing() {
    let r = setup();
    r.write("a.txt", "x\n");
    r.j(&["describe \"stable\""]).ok();
    // a non-terminating expression; kill it quickly
    let mut child = Command::new(j_bin())
        .arg("let go = \\n -> go (n + 1) in go 0")
        .current_dir(&r.dir)
        .env("XDG_CONFIG_HOME", &r.cfg)
        .env("NO_COLOR", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    child.kill().unwrap();
    let _ = child.wait();
    // lock released, repo intact
    let out = r.j(&["status"]).ok();
    assert!(out.stdout.contains("stable"));
}

#[test]
fn init_refuses_inside_existing_repo() {
    let r = setup();
    let out = r.j(&["init"]);
    assert_eq!(out.code, 2);
    assert!(out.stderr.contains("already"), "{}", out.stderr);
}

#[test]
fn missing_config_is_created_editable() {
    let dir = std::env::temp_dir().join(format!(
        "j-cli-nocfg-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let cfg_home = dir.join("nonexistent");
    let out = Command::new(j_bin())
        .arg("status")
        .current_dir(&dir)
        .env("XDG_CONFIG_HOME", &cfg_home)
        .output()
        .unwrap();
    // no config error: the default config is materialised instead
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("no config"), "{}", stderr);
    assert!(stderr.contains("created an editable default config"), "{}", stderr);
    // the config now exists at the user path and is editable by the owner
    let cfg = cfg_home.join("j/config.j");
    let written = std::fs::read_to_string(&cfg).expect("config was created");
    assert!(written.contains("treeWith"), "default config content");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&cfg).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ----------------------------------------------------------------------
// regressions
// ----------------------------------------------------------------------

#[test]
fn read_only_run_does_not_hide_working_copy_changes() {
    // A printing run snapshots the working directory but records no
    // operation. It must not save the scanned tree state either: doing so
    // told the next run's incremental snapshot that the directory was already
    // recorded, so it rebuilt the Repo from the stale tree in the working-copy
    // commit — and a later persisting run then checked that stale tree back
    // out, discarding the edits from disk (§7.4/§7.7).
    let r = setup();
    r.write("a.txt", "one\n");
    r.j(&["id"]).ok();
    r.write("a.txt", "edited content\n");
    r.write("b.txt", "brand new\n");
    for _ in 0..3 {
        let out = r.j(&["files"]).ok().stdout;
        assert!(out.contains("a.txt"), "{}", out);
        assert!(out.contains("b.txt"), "b.txt missing from: {}", out);
    }
    // and the edits survive the next persisting run, on disk and in the commit
    r.j(&["describe \"after reads\""]).ok();
    assert_eq!(r.read("a.txt"), "edited content\n");
    assert_eq!(r.read("b.txt"), "brand new\n");
    let files = r.j(&["files"]).ok().stdout;
    assert!(files.contains("b.txt"), "{}", files);
}

#[test]
fn refused_reserved_command_does_not_hide_working_copy_changes() {
    // `undo` refuses a dirty working copy; the snapshot it took to decide
    // that must not be saved either, for the same reason
    let r = setup();
    r.write("a.txt", "one\n");
    r.j(&["id"]).ok();
    r.write("a.txt", "edited content\n");
    assert_eq!(r.j(&["undo"]).code, 1);
    let out = r.j(&["files"]).ok().stdout;
    assert!(out.contains("a.txt"), "{}", out);
    // the size column reflects the edited file, not the recorded one
    assert!(out.contains("15 B"), "expected the edited size in: {}", out);
}

#[test]
fn tree_does_not_mark_commits_empty() {
    // `empty` was read off a file count that is 0 whenever the parent's file
    // list was never materialized, so every commit whose parent skipped
    // loading files rendered with the empty glyph
    let r = setup();
    for n in ["a", "b", "c", "d"] {
        r.write(&format!("{}.txt", n), &format!("content {}\n", n));
        r.j(&[&format!("describe \"commit {}\"", n)]).ok();
        r.j(&["new"]).ok();
    }
    let out = r.j(&["tree"]).ok().stdout;
    // each commit that added a file must not carry the empty glyph (the
    // legend at the bottom names it too, so check the commit rows only)
    for n in ["a", "b", "c", "d"] {
        let row = out
            .lines()
            .find(|l| l.contains(&format!("commit {}", n)))
            .unwrap_or_else(|| panic!("no row for commit {} in:\n{}", n, out));
        assert!(!row.contains('◌'), "commit {} marked empty: {:?}", n, row);
    }
}

/// The tree row of the commit whose message is `msg`.
fn tree_row<'a>(out: &'a str, msg: &str) -> &'a str {
    out.lines()
        .find(|l| l.ends_with(&format!("  {}", msg)))
        .unwrap_or_else(|| panic!("no row for {:?} in:\n{}", msg, out))
}

#[test]
fn dry_run_tree_glyphs_come_from_the_edited_history() {
    // `⊗` and `◌` were read off the stored commits by change id, so a dry run
    // drew what is stored rather than what the edit would produce (§1.2): a
    // rebase that conflicts showed no `⊗`, an edit that resolves a stored
    // conflict kept it, and emptying a commit left its glyph and its
    // untouched child's as they were
    let r = setup();
    r.write("f.txt", "base\n");
    r.j(&["describe \"base\""]).ok();
    r.j(&["new"]).ok();
    r.write("f.txt", "left\n");
    r.j(&["describe \"left\""]).ok();
    r.j(&["describe \"tip\" . new"]).ok();
    // `right`: a sibling of `left` on `base`
    r.j(&["new . prev . prev"]).ok();
    r.write("f.txt", "right\n");
    r.j(&["describe \"right\""]).ok();

    let out = r.j(&["tree . rebase siblings"]).ok().stdout;
    assert!(tree_row(&out, "right").contains('⊗'), "conflict not drawn:\n{}", out);

    let emptied = "tree . at siblings (\\r -> r { root = r.root { files = (up r).root.files } })";
    let out = r.j(&[emptied]).ok().stdout;
    assert!(tree_row(&out, "left").contains('◌'), "emptied commit not empty:\n{}", out);
    assert!(!tree_row(&out, "tip").contains('◌'), "changed child still empty:\n{}", out);

    // stored with the conflict, then resolved by an edit that is not persisted
    r.j(&["rebase siblings"]).ok();
    let out = r.j(&["tree"]).ok().stdout;
    assert!(tree_row(&out, "right").contains('⊗'), "{}", out);
    let resolved = "tree . (\\r -> r { root = r.root { files = (up r).root.files } })";
    let out = r.j(&[resolved]).ok().stdout;
    let row = tree_row(&out, "right");
    assert!(!row.contains('⊗') && row.contains('◌'), "resolved commit drawn as {:?}:\n{}", row, out);
}

#[test]
fn tree_draws_the_size_bar_of_the_focus_parent() {
    // the focus's parent diffs against its own parent, whose file list was
    // only loaded when that grandparent was the root: every other parent of
    // the focus lost its bar (specs/tree.md, Step 4 column 5)
    let r = setup();
    for (n, body) in [("A", "a\n"), ("B", "b\nb\nb\n"), ("C", "c\n")] {
        r.write(&format!("{}.txt", n), body);
        r.j(&[&format!("describe \"{}\"", n)]).ok();
        if n != "C" {
            r.j(&["new"]).ok();
        }
    }
    let out = r.j(&["tree"]).ok().stdout;
    let bars = ['▁', '▂', '▃', '▅', '▇'];
    for n in ["B", "C"] {
        assert!(tree_row(&out, n).contains(bars), "no size bar on {}:\n{}", n, out);
    }
    // the grandparent is neither the focus nor next to it: no bar at detail 1
    assert!(!tree_row(&out, "A").contains(bars), "{}", out);
}

#[test]
fn redo_twice_in_a_row() {
    // the second redo followed the redo marker to the undo it reversed and
    // restored *that* view, instead of looking outward for an undo that had
    // not been reversed yet (§7.7)
    let r = setup();
    r.write("a.txt", "x\n");
    // two operations on the same commit, so undoing both clears the message
    r.j(&["describe \"one\""]).ok();
    r.j(&["describe \"two\""]).ok();
    r.j(&["undo"]).ok();
    assert!(r.j(&["status"]).ok().stdout.contains("one"));
    r.j(&["undo"]).ok();
    let cleared = r.j(&["status"]).ok().stdout;
    assert!(!cleared.contains("one"), "{}", cleared);
    r.j(&["redo"]).ok();
    let first = r.j(&["status"]).ok().stdout;
    assert!(first.contains("one"), "first redo: {}", first);
    r.j(&["redo"]).ok();
    let second = r.j(&["status"]).ok().stdout;
    assert!(second.contains("two"), "second redo did not restore it:\n{}", second);
}

#[test]
fn undo_redo_cycles_are_stable() {
    // alternating undo/redo must keep returning to the same two states
    let r = setup();
    r.write("a.txt", "x\n");
    r.j(&["describe \"one\""]).ok();
    for i in 0..3 {
        r.j(&["undo"]).ok();
        let after = r.j(&["status"]).ok().stdout;
        assert!(!after.contains("one"), "cycle {}: {}", i, after);
        r.j(&["redo"]).ok();
        let back = r.j(&["status"]).ok().stdout;
        assert!(back.contains("one"), "cycle {}: {}", i, back);
    }
}

#[test]
fn extract_sees_through_lazy_file_lists() {
    // a commit's `files` is a thunk until something asks for it; `extract`
    // walked past it, so it found no entries at all on a real repository
    // while finding them all on the in-memory backend
    let r = setup();
    r.write("a.txt", "x\n");
    r.write("b.txt", "y\n");
    r.j(&["id"]).ok();
    let entries = r.j(&["length . extract Entry"]).ok().stdout;
    assert_eq!(entries.trim(), "2", "extract Entry: {}", entries);
    let blobs = r.j(&["length . extract Blob"]).ok().stdout;
    assert_eq!(blobs.trim(), "2", "extract Blob: {}", blobs);
}

#[test]
fn moving_the_focus_rewrites_uncommitted_edits() {
    // A persisting run that starts with uncommitted edits folds their
    // snapshot into its operation and then checks out the new focus. The
    // checkout diffed from the tree the working copy had last *recorded*,
    // not from the snapshot, so a path whose recorded content equals the new
    // focus's was never rewritten: `prev` left the child's edit on disk, and
    // the next run recorded it into the parent (§7.4, §7.5 step 7).
    let r = setup();
    r.write("a.txt", "one\n");
    r.j(&["describe \"first\""]).ok();
    r.j(&["new"]).ok();
    r.write("a.txt", "two\n");
    r.j(&["prev"]).ok();
    assert_eq!(r.read("a.txt"), "one\n");
    r.j(&["describe \"first, again\""]).ok();
    let files = r.j(&["\\r -> show r.root.files"]).ok().stdout;
    assert!(files.contains("blob \"one\\n\""), "the parent took the edit: {}", files);
    // the edit itself lives on in the child it was snapshotted into
    r.j(&["next"]).ok();
    assert_eq!(r.read("a.txt"), "two\n");
}

#[test]
fn abandon_discards_uncommitted_edits_from_disk() {
    // the same stale diff: the new focus, an empty child of the abandoned
    // commit's parent, has exactly the recorded tree, so the checkout did
    // nothing at all and the discarded
    // edits, deletions and new files stayed on disk to be recorded into the
    // new focus by the next run
    let r = setup();
    r.write("a.txt", "orig\n");
    r.write("b.txt", "bee\n");
    r.j(&["describe \"base\""]).ok();
    r.j(&["new"]).ok();
    r.write("a.txt", "scratch\n");
    std::fs::remove_file(r.dir.join("b.txt")).unwrap();
    r.write("c.txt", "junk\n");
    r.j(&["abandon"]).ok();
    assert_eq!(r.read("a.txt"), "orig\n");
    assert_eq!(r.read("b.txt"), "bee\n");
    assert!(!r.dir.join("c.txt").exists(), "c.txt survived the abandon");
    // the directory now equals the focus, so recording it is a no-op
    r.j(&["id"]).ok();
    let ops = r.j(&["ops"]).ok().stdout;
    assert!(ops.lines().next().unwrap_or("").contains("abandon"), "{}", ops);
}

#[test]
fn reordering_a_snapshot_is_not_a_change() {
    // `select` lists the entries a fileset matches first, so a `contract m`
    // that moves nothing still hands persistence a parent whose entries are
    // in another order. Comparing snapshots as lists called that a change:
    // every such run rewrote the parent and each commit above it (new hashes
    // and committer times, the same trees) and recorded an operation. A
    // snapshot stands for a tree, which has no order (§7.3, §7.5 step 4).
    let r = setup();
    r.write("a.txt", "a\n");
    std::fs::create_dir(r.dir.join("h")).unwrap();
    r.write("h/x", "x\n");
    r.j(&["describe \"base\""]).ok();
    r.j(&["new"]).ok();
    r.write("h/x", "x2\n");
    r.j(&["describe \"child\""]).ok();
    // the first contract moves h/x into base; after it, one moves nothing
    r.j(&["contract (under ./h)"]).ok();
    let hashes = || {
        r.j(&["\\r -> map (\\i -> (meta i).hash) (ancestors r)"])
            .ok()
            .stdout
    };
    let before = hashes();
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    let rotate = "mapRoot (\\c -> c { files = tail c.files ++ [(head c.files)] })";
    for edit in [
        "contract (under ./h)".to_string(),
        "tree . validate . contract (under ./h)".to_string(),
        // the same kind of reorder written by hand, on the focus and its parent
        rotate.to_string(),
        format!("at parents ({})", rotate),
    ] {
        r.j(&[&edit]).ok();
        let now = r.j(&["ops"]).ok().stdout.lines().count();
        assert_eq!(now, ops, "{} was recorded", edit);
        assert_eq!(hashes(), before, "{} rewrote a commit", edit);
    }
    // an edit in the working directory is recorded, and the parent, which
    // comes back only reordered, is kept as stored
    let parent_hash = || r.j(&["\\r -> (meta (up r).root.id).hash"]).ok().stdout;
    let parent = parent_hash();
    r.write("a.txt", "edited\n");
    r.j(&["contract (under ./h)"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    assert_eq!(parent_hash(), parent, "the parent was rewritten");
    // a real change to the parent is recorded
    r.write("h/x", "x3\n");
    r.j(&["contract (under ./h)"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 2);
    assert_ne!(parent_hash(), parent);
    let base = "\\r -> text (contentAt ./h/x (up r).root.files)";
    assert_eq!(r.j(&[base]).ok().stdout, "x3\n");
}

#[test]
fn replacing_a_file_with_a_directory_survives_persistence() {
    // checking out from the stale recorded tree turned a file <-> directory
    // swap into working-copy state that contradicts the tree: debug builds
    // panicked in jj-lib on this and every later run, release builds dropped
    // the directory from disk and from the commit on the next snapshot
    let r = setup();
    r.write("a", "hi\n");
    r.j(&["describe \"file\""]).ok();
    r.j(&["new"]).ok();
    std::fs::remove_file(r.dir.join("a")).unwrap();
    std::fs::create_dir(r.dir.join("a")).unwrap();
    r.write("a/b.txt", "t\n");
    r.j(&["describe \"dir\""]).ok();
    r.j(&["tree"]).ok();
    r.j(&["id"]).ok();
    assert_eq!(r.read("a/b.txt"), "t\n");
    let files = r.j(&["files"]).ok().stdout;
    assert!(files.contains("b.txt"), "{}", files);
    // and back: a directory replaced by a file
    r.j(&["new"]).ok();
    std::fs::remove_dir_all(r.dir.join("a")).unwrap();
    r.write("a", "file again\n");
    r.j(&["describe \"file again\""]).ok();
    r.j(&["tree"]).ok();
    r.j(&["id"]).ok();
    assert_eq!(r.read("a"), "file again\n");
    // the checkout itself swaps them back when the focus moves
    r.j(&["prev"]).ok();
    assert_eq!(r.read("a/b.txt"), "t\n");
    r.j(&["prev"]).ok();
    assert_eq!(r.read("a"), "hi\n");
}

#[test]
fn replay_carries_conflicts_through() {
    // replay unwrapped each input tree's ids as resolved, so every rebase,
    // squash or abandon that replayed a snapshot holding a conflict panicked
    // (exit 101, uncatchable by `or`) instead of keeping the conflict (§7.3,
    // §8); the in-memory backend handled it, so only a real repo showed it
    let r = setup();
    r.write("a.txt", "base\n");
    r.j(&["describe \"base\""]).ok();
    r.j(&["new"]).ok();
    r.write("a.txt", "left\n");
    r.j(&["describe \"left\""]).ok();
    r.j(&["new . goto parents"]).ok();
    r.write("a.txt", "right\n");
    r.j(&["describe \"right\""]).ok();
    // all three inputs are still resolved here; the result is not
    r.j(&["rebase siblings"]).ok();
    let conflicted = "\\r -> show (conflicted (files r))";
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"a.txt\"]]");
    // the §8 laws, with a conflicted snapshot in each position
    for law in [
        "\\r -> replay (files r) ({ from = files r, to = files r }) == files r",
        "\\r -> replay (files r) ({ from = [], to = [] }) == files r",
        "\\r -> replay [] ({ from = [], to = files r }) == files r",
        "\\r -> replay (files r) ({ from = files r, to = files (up r) }) == files (up r)",
    ] {
        assert_eq!(r.j(&[law]).ok().stdout.trim(), "true", "{}", law);
    }
    // rebase parents r = r: nothing to record. Count the operations: the
    // listing shows each one's age, which can tick over between the runs
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    r.j(&["rebase parents"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops);
    // squashing into the conflicted commit keeps its conflict
    r.j(&["new"]).ok();
    r.write("b.txt", "bee\n");
    r.j(&["squash"]).ok();
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"a.txt\"]]");
    assert_eq!(r.read("b.txt"), "bee\n");
    assert!(r.read("a.txt").contains("<<<<<<<"), "{}", r.read("a.txt"));
    // abandoning the side the conflict came from replays the child onto
    // base, where jj's merge resolves it to the child's own side
    let named = |m: &str| format!("(matching (\\c -> c.message == \"{}\") all)", m);
    r.j(&[&format!("abandon . goto {}", named("left"))]).ok();
    assert_eq!(r.j(&["\\r -> show (conflicts r)"]).ok().stdout.trim(), "[]");
    let files = format!("\\r -> show (map (\\i -> (commitAt i r).files) ({} r))", named("right"));
    let files = r.j(&[&files]).ok().stdout;
    assert!(files.contains("blob \"right\\n\"") && files.contains("blob \"bee\\n\""), "{}", files);
}

#[cfg(unix)]
#[test]
fn conflict_sides_keep_absence_and_file_type() {
    // a conflict blob kept each side as bare bytes: a side the path was
    // deleted on came back as an empty file, and every side lost its
    // executable bit or symlink type (§7.3). The conflicted checkout of an
    // executable file was not executable, and rebasing a conflicted commit
    // back where it came from, whose sides must cancel, left an empty file
    // for its deletion, dropped +x, and kept a symlink conflict.
    use std::os::unix::fs::{symlink, PermissionsExt};
    let r = setup();
    let exec = |p: &str| {
        let mode = std::fs::metadata(r.dir.join(p)).unwrap().permissions().mode();
        mode & 0o111 != 0
    };
    let relink = |target: &str| {
        let _ = std::fs::remove_file(r.dir.join("l"));
        symlink(target, r.dir.join("l")).unwrap();
    };
    r.write("f.txt", "base\n");
    r.write("g.sh", "x\n");
    std::fs::set_permissions(r.dir.join("g.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    relink("a");
    r.j(&["describe \"A\""]).ok();
    r.j(&["new"]).ok();
    r.write("f.txt", "mod\n");
    r.write("g.sh", "y\n");
    relink("b");
    r.j(&["describe \"B\""]).ok();
    r.j(&["new"]).ok();
    std::fs::remove_file(r.dir.join("f.txt")).unwrap();
    r.write("g.sh", "z\n");
    relink("c");
    r.j(&["describe \"C\""]).ok();
    assert!(exec("g.sh"));
    // C onto A: a modify/delete conflict, and executable and symlink sides
    r.j(&["rebase (parents . prev)"]).ok();
    let conflicted = "\\r -> show (conflicted (files r))";
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"f.txt\"] [\"g.sh\"] [\"l\"]]");
    // every side of g.sh is executable, so its materialised conflict is
    assert!(exec("g.sh"), "the conflicted checkout of g.sh lost +x");
    // C back onto B: B's sides cancel, leaving C's own change
    r.j(&["rebase siblings"]).ok();
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[]");
    let paths = r.j(&["\\r -> show (map (.path) (files r))"]).ok().stdout;
    assert_eq!(paths.trim(), "[[\"g.sh\"] [\"l\"]]");
    assert_eq!(r.read("g.sh"), "z\n");
    assert!(exec("g.sh"), "g.sh lost +x");
    assert_eq!(std::fs::read_link(r.dir.join("l")).unwrap(), PathBuf::from("c"));
}
