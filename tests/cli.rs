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

    /// Run `j` with stderr on a pipe whose reader has already exited, as in
    /// `j … 2>&1 | head` once `head` has read all it wants; only the status
    /// is left to look at.
    fn j_closed_stderr(&self, args: &[&str]) -> i32 {
        let (reader, writer) = std::io::pipe().unwrap();
        drop(reader);
        Command::new(j_bin())
            .args(args)
            .current_dir(&self.dir)
            .env("XDG_CONFIG_HOME", &self.cfg)
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(writer)
            .status()
            .unwrap()
            .code()
            .unwrap_or(-1)
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
fn an_expression_that_is_not_utf8_is_a_usage_error() {
    // §1: an argument that was not UTF-8 (a path with a Latin-1 byte)
    // panicked in `std::env::args` and exited 101, and such bytes on stdin
    // read as no input at all, so the run printed the usage line
    use std::ffi::OsStr;
    use std::io::Write;
    use std::os::unix::ffi::OsStrExt;
    let r = setup();
    let run = |args: &[&OsStr], input: &[u8]| {
        let mut child = Command::new(j_bin())
            .args(args)
            .current_dir(&r.dir)
            .env("XDG_CONFIG_HOME", &r.cfg)
            .env("NO_COLOR", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        let out = child.wait_with_output().unwrap();
        Out {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        }
    };
    let message = OsStr::from_bytes(b"describe \"\xff\"");
    for args in [vec![message], vec![OsStr::new("remote"), OsStr::from_bytes(b"../nu\xff/r.git")]] {
        let out = run(&args, b"");
        assert_eq!(out.code, 2, "{}", out.stderr);
        assert!(out.stderr.starts_with("j: argument is not valid UTF-8: "), "{}", out.stderr);
        assert_eq!(out.stderr.lines().count(), 1, "{}", out.stderr);
    }
    let out = run(&[], message.as_bytes());
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert_eq!(out.stderr, "j: the expression on stdin is not valid UTF-8\n");
    // input on stdin is input, whatever its bytes (§1 rule 3)
    let out = run(&[OsStr::new("1")], message.as_bytes());
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(out.stderr.contains("both"), "{}", out.stderr);
    // nothing was described
    assert_eq!(r.j(&["\\r -> show (focus r).message"]).ok().stdout, "\"\"\n");
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
fn deep_nesting_is_a_parse_error() {
    // §1.4: however deep the input, the process exits 3 with one `j:` line
    // rather than aborting on a stack overflow in the parser
    let r = setup();
    let n = 100_000;
    // a left-associative chain too: the parser builds it in a loop, but its
    // tree is as tall as the chain is long, and resolving its id literals
    // after the parse recursed that deep
    for src in [
        format!("{}1{}", "(".repeat(n), ")".repeat(n)),
        "[".repeat(n),
        format!("1{}", " + 1".repeat(1_000_000)),
    ] {
        let out = r.j_stdin(&src, &[]);
        assert_eq!(out.code, 3, "{}", out.stderr);
        assert!(out.stderr.starts_with("j: ") && out.stderr.lines().count() == 1, "{}", out.stderr);
    }
}

#[test]
fn lists_nested_as_deep_as_they_parse_display() {
    // §1.4, §3.4: input within the nesting bound is never a stack overflow,
    // and the displayed result is no exception. Display recursed once per
    // level of nested lists, at about 14 KB a level in this debug build, so
    // a list literal 38,000 deep parsed and then aborted with exit 134 and
    // no `j:` line, as did a list built that deep at run time
    let r = setup();
    let n = 40_000;
    let out = r.j_stdin(&format!("{}{}", "[".repeat(n), "]".repeat(n)), &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    // the innermost `[[]]` is a list of one path, the empty one (§5.1)
    assert_eq!(out.stdout, "\n");
    // `[[[[] 0] 1] …]`: any other list is one block per item (§5.1), and a
    // list built at run time is not bounded by the parser
    let n = 100_000;
    let out = r.j(&[&format!("foldl (\\acc i -> [acc i]) [] (range 0 {})", n)]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let items: String = (0..n).map(|i| format!("\n{}\n", i)).collect();
    assert!(out.stdout == format!("none\n{}", items), "{}…", &out.stdout[..40.min(out.stdout.len())]);
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
fn crash_trace_names_the_innermost_definition() {
    // §1.4, §4.13, §5.1: the second line names the innermost definition that
    // was executing, as the spec's own examples show. Nothing recorded one
    // at run time, so every crash printed only `from EXPR`. A builtin is no
    // definition, so a crash outside every definition names none.
    let r = setup();
    for (expr, trace) in [
        ("goto %nope", "   in goto, from goto %nope"),
        ("describe 3", "   in describe, from describe 3"),
        ("new . goto %nope", "   in goto, from new . goto %nope"),
        ("squash 3", "   in squash, from squash 3"),
        // `prev = goto parents` runs goto's body, which raised the crash
        ("prev . top", "   in goto, from prev . top"),
        ("crash \"boom\"", "   from crash \"boom\""),
    ] {
        let out = r.j(&[expr]);
        assert_eq!(out.code, 1, "{}: {}", expr, out.stderr);
        assert_eq!(out.stderr.lines().nth(1), Some(trace), "{}: {}", expr, out.stderr);
    }
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
fn a_directory_replaced_by_a_file_inside_an_ignored_directory_is_recorded() {
    // `build/a` holds tracked files when `.gitignore` comes to ignore
    // `build/`, and is then replaced by a file. jj's scan reads an ignored
    // directory only at the paths it tracks there, and took a path below
    // the file for one it could not look at rather than one deleted, so
    // every run that scanned exited 2, `undo` included, until the file was
    // moved away (§7.4)
    let r = setup();
    std::fs::create_dir_all(r.dir.join("build/a/c")).unwrap();
    r.write("build/a/b", "x\n");
    r.write("build/a/c/d", "y\n");
    r.write("build/keep", "k\n");
    r.j(&["id"]).ok();
    r.write(".gitignore", "build/\n");
    r.j(&["id"]).ok();
    std::fs::remove_dir_all(r.dir.join("build/a")).unwrap();
    r.write("build/a", "f\n");
    // the files below it are deleted; it is ignored, as a new file there is
    let paths = "\\r -> show (map (.path) (files r))";
    let left = "[[\".gitignore\"] [\"build\" \"keep\"]]";
    assert_eq!(r.j(&[paths]).ok().stdout.trim(), left);
    let out = r.j(&["undo"]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("working copy has changes not in @"), "{}", out.stderr);
    r.j(&["id"]).ok();
    assert_eq!(r.j(&[paths]).ok().stdout.trim(), left);
    r.j(&["new"]).ok();
    assert_eq!(r.j(&[paths]).ok().stdout.trim(), left);
    assert_eq!(r.read("build/a"), "f\n");
    assert_eq!(r.read("build/keep"), "k\n");
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
fn persisting_reads_no_tree_of_a_commit_it_keeps() {
    // validation checked the paths of every commit's snapshot, and the walk
    // compared every commit with its stored tree entry by entry, so each
    // persisting run read the files of the whole history: `describe` took
    // two minutes (a debug build) on 2,500 commits of 3,000 files. A commit
    // whose files are still its stored tree's is now compared by tree (§7.5).
    // Here A's tree is missing from the store, which a run that keeps A
    // does not notice
    let r = setup();
    r.write("a.txt", "only in A\n");
    r.j(&["describe \"A\""]).ok();
    r.j(&["new"]).ok();
    r.write("b.txt", "B\n");
    r.j(&["describe \"B\""]).ok();
    r.j(&["new"]).ok();
    let hash = |kind, bytes: &[u8]| gix::objs::compute_hash(gix::hash::Kind::Sha1, kind, bytes).unwrap();
    let mut tree = b"100644 a.txt\0".to_vec();
    tree.extend_from_slice(hash(gix::objs::Kind::Blob, b"only in A\n").as_bytes());
    let id = hash(gix::objs::Kind::Tree, &tree).to_string();
    std::fs::remove_file(r.dir.join(".git/objects").join(&id[..2]).join(&id[2..])).unwrap();
    r.j(&["describe \"C\""]).ok();
    // a snapshot loads the history afresh
    r.write("c.txt", "C\n");
    r.j(&["describe \"C2\""]).ok();
    r.j(&["new"]).ok();
    // reading A's files still fails
    let out = r.j(&[PATHS_BY_COMMIT]);
    assert_eq!(out.code, 1, "{}", out.stderr);
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
fn review_says_no_changes_only_when_nothing_changed() {
    // `review` caught every crash of `diffs` with `or`, so without difft on
    // PATH it printed "no changes" over a change it could not render
    let r = setup();
    // a PATH holding nothing but what the test puts there (the config
    // directory is removed with the repository)
    let bin = r.cfg.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let review = |path: &std::path::Path| {
        let out = Command::new(j_bin())
            .arg("review")
            .current_dir(&r.dir)
            .env("XDG_CONFIG_HOME", &r.cfg)
            .env("NO_COLOR", "1")
            .env("PATH", path)
            .output()
            .unwrap();
        Out {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        }
    };
    // nothing to render: difft is never run
    assert_eq!(review(&bin).ok().stdout, "no changes\n");
    r.write("f", "hello\n");
    let out = review(&bin);
    assert_eq!(out.code, 1, "stdout: {}", out.stdout);
    assert!(out.stderr.contains("cannot execute `difft`"), "{}", out.stderr);
    use std::os::unix::fs::PermissionsExt;
    // a difft that runs and fails renders what it printed, nothing here,
    // and its complaint reaches the terminal instead of being dropped (§7.10)
    let failing = r.cfg.join("failing");
    std::fs::create_dir_all(&failing).unwrap();
    let stub = failing.join("difft");
    std::fs::write(&stub, "#!/bin/sh\necho 'error: invalid value for --color' >&2\nexit 2\n").unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = review(&failing);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stderr, "error: invalid value for --color\n");
    assert_eq!(out.stdout.trim(), "");
    // with a difft, each changed path's rendering, in order
    let stub = bin.join("difft");
    std::fs::write(&stub, "#!/bin/sh\nprintf '%s -> %s\\n' \"${1##*/}\" \"${2##*/}\"\n").unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    r.write("g", "world\n");
    assert_eq!(review(&bin).ok().stdout, "old-f -> new-f\nold-g -> new-g\n");
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
fn closed_stderr_keeps_the_exit_status() {
    // §1.4: a message stderr cannot take is dropped, and the run keeps its
    // status; every error writer used to panic on the closed pipe instead,
    // and the run exited 101
    use std::os::unix::io::AsRawFd;
    let r = setup();
    for (args, code) in [
        (&["crash \"boom\""][..], 1), // crash and its `from` line
        (&["by @kqzz"], 1),           // an unresolvable id
        (&["1 +"], 3),                // parse error
        (&["push"], 2),               // a reserved command's usage
        (&[], 2),                     // no expression at all
    ] {
        assert_eq!(r.j_closed_stderr(args), code, "{:?}", args);
    }
    // the lock refusal is written by the backend
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(r.dir.join(".jj/j.lock"))
        .unwrap();
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }, 0);
    assert_eq!(r.j_closed_stderr(&["tree"]), 2);
    drop(lock);
    // the notice for a default config written on first use is not an error
    std::fs::remove_file(r.cfg.join("j/config.j")).unwrap();
    assert_eq!(r.j_closed_stderr(&["1 + 1"]), 0);
    assert!(r.cfg.join("j/config.j").exists());
    // and a config error is still one
    std::fs::write(r.cfg.join("j/config.j"), "x =\n").unwrap();
    assert_eq!(r.j_closed_stderr(&["1 + 1"]), 3);
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
    // longer than NAME_MAX (255 bytes): the checkout failed to stat it
    let long = format!("[\"{}\"]", "x".repeat(256));
    for bad in [
        r#"[".." "evil"]"#,
        r#"["x" ".."]"#,
        r#"[".git" "hooks" "post-checkout"]"#,
        r#"[".jj" "x"]"#,
        long.as_str(),
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
fn names_the_filesystem_holds_are_accepted_at_any_length() {
    // §7.5 step 1: 255 bytes is ext4's name limit, but exFAT, NTFS and APFS
    // count 255 UTF-16 units or characters and hold a name of 100 `日`
    // (300 bytes). With one in the working directory, every persisting run
    // was refused for it, though the checkout never creates a name the
    // directory holds, and a name the checkout would create was refused
    // though the filesystem holds it. Where it does not, the name is still
    // refused before anything is recorded.
    let r = setup();
    r.write("a.txt", "x\n");
    r.j(&["describe \"stable\""]).ok();
    let wide = "日".repeat(100);
    let edit = |name: &str| {
        format!(
            "mapRoot (\\c -> c {{ files = c.files ++ [{{ path = [\"d\" \"{}\"], content = blob \"new\\n\" }}] }})",
            name
        )
    };
    if std::fs::write(r.dir.join(&wide), "on disk\n").is_ok() {
        r.j(&["describe \"wide\""]).ok();
        r.j(&["new"]).ok();
        let wider = "日".repeat(101);
        r.j(&[&format!("tree . validate . {}", edit(&wider))]).ok();
        r.j(&[&edit(&wider)]).ok();
        assert_eq!(r.read(&format!("d/{}", wider)), "new\n");
        assert_eq!(r.read(&wide), "on disk\n");
    } else {
        let ops = r.j(&["ops"]).ok().stdout.lines().count();
        let out = r.j(&[&edit(&wide)]);
        assert_eq!(out.code, 1, "{}", out.stderr);
        assert!(out.stderr.contains("path component"), "{}", out.stderr);
        assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops);
        assert!(!r.dir.join("d").exists());
    }
    r.j(&["describe \"after\""]).ok();
}

/// Each commit's message and content of `a`, top-down
const CONTENT_OF_A: &str =
    "\\r -> show (map (\\c -> [c.message (contentAt [\"a\"] c.files)]) (commits (top r)))";

/// An expression showing, for the commit with each of `messages`, the text
/// at each of `paths` (in the language's syntax), in that order: siblings
/// made or rewritten by one operation are listed in no order of their own
fn texts_by_message(paths: &[&str], messages: &[&str]) -> String {
    let texts: Vec<String> = paths.iter().map(|p| format!("(text (contentAt {} f))", p)).collect();
    let messages: Vec<String> = messages.iter().map(|m| format!("\"{}\"", m)).collect();
    format!(
        "\\r -> show (map (\\m -> let f = (focus (goto (matching (\\c -> c.message == m) all) r)).files in [{}]) [{}])",
        texts.join(" "),
        messages.join(" ")
    )
}

#[test]
fn a_checkout_that_fails_is_recorded_and_undone() {
    // §7.5 step 7, §7.7: the operation was published only once the checkout
    // had completed, so a checkout the filesystem refused (a path longer
    // than PATH_MAX here; a name longer than a smaller NAME_MAX, a full
    // disk) was put back file by file, and a put-back can fail as well. The
    // operation is now published first, as jj does, and a checkout that
    // fails leaves it recorded: the crash says so, the next run takes what
    // the checkout wrote for the focus's own, and `j undo` goes back.
    let r = setup();
    r.write("a", "p\n");
    r.j(&["describe \"P\""]).ok();
    r.j(&["new"]).ok();
    r.write("a", "x1\n");
    r.j(&["describe \"X\""]).ok();
    r.write("a", "x2\n");
    let log = r.j(&["log"]).ok().stdout;
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    // every component is a name a checkout can create (§7.5 step 1)
    let deep = vec![format!("\"{}\"", "d".repeat(250)); 20].join(" ");
    let edit = format!(
        "\\r -> mapRoot (\\c -> c {{ files = c.files ++ [{{ path = [{}], content = blob \"deep\" }}] }}) (prev r)",
        deep
    );
    let out = r.j(&[&edit]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("cannot check out"), "{}", out.stderr);
    assert!(out.stderr.contains("the operation is recorded"), "{}", out.stderr);
    assert!(out.stderr.contains("only partly updated"), "{}", out.stderr);
    assert!(out.stderr.contains("`j undo` goes back"), "{}", out.stderr);
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    // the checkout wrote P's `a` before it failed
    assert_eq!(r.read("a"), "p\n");
    // which is not recorded into P, nor into X, which holds the edit the
    // run recorded
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    let files = r.j(&[CONTENT_OF_A]).ok().stdout;
    assert_eq!(files.trim(), r#"[["" (blob "")] ["P" (blob "p\n")] ["X" (blob "x2\n")]]"#);
    r.j(&["undo"]).ok();
    assert_eq!(r.j(&["log"]).ok().stdout, log);
}

#[test]
#[cfg(target_os = "linux")]
fn an_undo_whose_checkout_fails_is_recorded_and_redone() {
    // §7.7: `undo` and `redo` publish their operation before checking the
    // restored focus out, as every run does (§7.5 step 7); a checkout that
    // fails leaves it recorded, and `j redo` goes back
    let r = setup();
    r.write("a", "one\n");
    // a file as deep as this workspace can hold (PATH_MAX, 4096 bytes with
    // the NUL), and no deeper
    let root = r.dir.canonicalize().unwrap();
    let n = (4095 - root.as_os_str().len() - "/f".len()) / 251;
    let deep: PathBuf = (0..n).map(|_| "d".repeat(250)).collect();
    std::fs::create_dir_all(r.dir.join(&deep)).unwrap();
    std::fs::write(r.dir.join(&deep).join("f"), "deep\n").unwrap();
    r.j(&["describe \"deep\""]).ok();
    r.j(&["new"]).ok();
    std::fs::remove_dir_all(r.dir.join("d".repeat(250))).unwrap();
    r.write("a", "two\n");
    r.j(&["id"]).ok();
    // 300 bytes further down, the workspace can no longer hold it
    let aside = r.dir.with_extension("aside");
    std::fs::rename(&r.dir, &aside).unwrap();
    let moved = r.dir.join("m".repeat(150)).join("m".repeat(150));
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(&aside, &moved).unwrap();
    let r = Repo { dir: moved, cfg: r.cfg.clone() };
    // `ops` prints each operation's age, so count them rather than compare
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    let out = r.j(&["undo"]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("cannot check out"), "{}", out.stderr);
    assert!(out.stderr.contains("the operation is recorded"), "{}", out.stderr);
    assert!(out.stderr.contains("`j redo` goes back"), "{}", out.stderr);
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    // the checkout wrote `a` before it failed, which the next run takes for
    // the restored focus's own
    assert_eq!(r.read("a"), "one\n");
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    r.j(&["redo"]).ok();
    assert_eq!(r.read("a"), "two\n");
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 2);
}

/// Path components, in the language's syntax, of a path under `first` far
/// longer than PATH_MAX, each a name a checkout can create (§7.5 step 1)
fn too_deep(first: &str) -> String {
    format!("\"{}\" {}", first, vec![format!("\"{}\"", "d".repeat(250)); 20].join(" "))
}

#[test]
fn a_failed_checkout_leaves_ignored_files_alone() {
    // §7.5 step 7: a failed checkout was put back, which scanned the whole
    // directory with the `.gitignore` the checkout had just written, and so
    // could read into the store, or delete, an ignored file; a checkout
    // that fails is now left as it is, for the next run to carry on from.
    // An ignored file put where the focus adds one the checkout did not
    // reach is left alone by the runs after it, which do not track it, and
    // by `j undo`, which takes back only what was written. (One there
    // before the run refuses it, before anything is recorded:
    // `a_checkout_writes_nothing_over_what_the_directory_does_not_track`.)
    let r = setup();
    r.write(".gitignore", "*.env\n");
    r.write("keep.txt", "k\n");
    r.j(&["describe \"base\""]).ok();
    let secret = "API_KEY=hunter2\n";
    r.write("secret.env", secret);
    let log = r.j(&["log"]).ok().stdout;
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    // written in path order: `.gitignore`, `build/out`, then the deep path
    // under `c` fails, before `local.env`. Each record is parenthesised:
    // side by side, the second would update the first (§3).
    let edit = format!(
        "mapRoot (\\c -> c {{ files = filter (\\e -> e.path /= [\".gitignore\"]) c.files ++ [\
         ({{ path = [\".gitignore\"], content = blob \"*.env\\n*.log\\n\" }}) \
         ({{ path = [\"build\" \"out\"], content = blob \"built\\n\" }}) \
         ({{ path = [\"local.env\"], content = blob \"committed\\n\" }}) \
         ({{ path = [{}], content = blob \"deep\" }})] }})",
        too_deep("c")
    );
    let paths = r.j(&[&format!("\\r -> show (map (\\e -> e.path) (files (({}) r)))", edit)]).ok().stdout;
    let paths = paths.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(paths.starts_with(r#"[["keep.txt"] [".gitignore"] ["build" "out"] ["local.env"] ["c" "#), "{}", paths);
    let out = r.j(&[&edit]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("cannot check out"), "{}", out.stderr);
    assert!(out.stderr.contains("the operation is recorded"), "{}", out.stderr);
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    assert_eq!(r.read(".gitignore"), "*.env\n*.log\n");
    assert_eq!(r.read("build/out"), "built\n");
    assert_eq!(r.read("secret.env"), secret);
    assert!(!r.dir.join("local.env").exists());
    r.write("local.env", "mine\n");
    for run in ["log", "id"] {
        r.j(&[run]).ok();
        assert!(!has_blob(&r, secret));
        assert!(!has_blob(&r, "mine\n"));
    }
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    r.j(&["undo"]).ok();
    assert_eq!(r.read(".gitignore"), "*.env\n");
    assert!(!r.dir.join("build").exists());
    assert_eq!(r.read("secret.env"), secret);
    assert_eq!(r.read("local.env"), "mine\n");
    assert!(!has_blob(&r, secret));
    assert!(!has_blob(&r, "mine\n"));
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 2);
    assert_eq!(r.j(&["log"]).ok().stdout, log);
}

#[test]
fn undoing_a_failed_checkout_keeps_the_ignored_files_where_it_wrote() {
    // §7.5 step 7, §7.7: `j undo` after a failed checkout removes the files
    // it added and, as any checkout does, the directories that leaves
    // empty; one holding an ignored file is not, and stays, file and all
    let r = setup();
    r.write(".gitignore", "*.log\n");
    r.write("keep.txt", "k\n");
    r.j(&["describe \"base\""]).ok();
    std::fs::create_dir(r.dir.join("logs")).unwrap();
    r.write("logs/old.log", "old\n");
    let log = r.j(&["log"]).ok().stdout;
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    // `logs/x` is written, then the deep path fails
    let edit = format!(
        "mapRoot (\\c -> c {{ files = c.files ++ [\
         ({{ path = [\"logs\" \"x\"], content = blob \"x\\n\" }}) \
         ({{ path = [{}], content = blob \"deep\" }})] }})",
        too_deep("zz")
    );
    let out = r.j(&[&edit]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("cannot check out"), "{}", out.stderr);
    assert!(out.stderr.contains("the operation is recorded"), "{}", out.stderr);
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    assert_eq!(r.read("logs/x"), "x\n");
    r.j(&["undo"]).ok();
    assert!(!r.dir.join("logs/x").exists());
    assert_eq!(r.read("logs/old.log"), "old\n");
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 2);
    assert_eq!(r.j(&["log"]).ok().stdout, log);
}

/// The entries of `.jj` a checkout moved files aside into (`clear_the_way`)
fn aside_in_jj(r: &Repo) -> Vec<String> {
    std::fs::read_dir(r.dir.join(".jj"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("aside"))
        .collect()
}

#[test]
fn a_failed_checkout_puts_back_the_ignored_file_it_was_to_write_again() {
    // §7.4, §7.5 step 7: an untracked file holding just what the checkout
    // writes at a path the focus adds was removed before the checkout, to
    // be written again; a checkout that failed before it got there left it
    // removed, and `j undo` did not bring it back. It is moved aside now,
    // and put back where the checkout did not write.
    let r = setup();
    r.write(".gitignore", "*.env\n");
    r.write("keep.txt", "k\n");
    r.j(&["describe \"base\""]).ok();
    r.write("local.env", "A=1\n");
    let log = r.j(&["log"]).ok().stdout;
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    // the deep path under `a` fails, before `local.env`
    let edit = format!(
        "mapRoot (\\c -> c {{ files = c.files ++ [\
         ({{ path = [{}], content = blob \"deep\" }}) \
         ({{ path = [\"local.env\"], content = blob \"A=1\\n\" }})] }})",
        too_deep("a")
    );
    let out = r.j(&[&edit]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("cannot check out"), "{}", out.stderr);
    assert!(out.stderr.contains("the operation is recorded"), "{}", out.stderr);
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    assert_eq!(r.read("local.env"), "A=1\n");
    assert_eq!(aside_in_jj(&r), Vec::<String>::new());
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    r.j(&["undo"]).ok();
    assert_eq!(r.read("local.env"), "A=1\n");
    assert_eq!(aside_in_jj(&r), Vec::<String>::new());
    assert_eq!(r.j(&["log"]).ok().stdout, log);
    // a checkout that completes writes it
    let edit = "mapRoot (\\c -> c { files = c.files ++ [{ path = [\"local.env\"], content = blob \"A=1\\n\" }] })";
    r.j(&[edit]).ok();
    assert_eq!(r.read("local.env"), "A=1\n");
    assert_eq!(r.j(&["\\r -> text (contentAt [\"local.env\"] (files r))"]).ok().stdout, "A=1\n");
    assert_eq!(aside_in_jj(&r), Vec::<String>::new());
}

#[test]
#[cfg(unix)]
fn a_checkout_names_the_file_it_cannot_move_out_of_its_way() {
    // §7.4: an untracked file holding just what the checkout writes at a
    // path the focus adds, which could not be removed for it, made jj skip
    // the path, and the checkout failed saying only that files were not
    // written, naming none, on every run that persisted. It names the file
    // now, and why it could not be moved. (A directory the user cannot
    // write to holds it here; root can.)
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let r = setup();
    r.write(".gitignore", "*.env\n");
    r.write("keep.txt", "k\n");
    r.j(&["describe \"A\""]).ok();
    let adds = "mapRoot (\\c -> c { files = c.files ++ [{ path = [\"etc\" \"local.env\"], content = blob \"A=1\\n\" }] })";
    r.j(&[&format!("describe \"B\" . {} . new", adds)]).ok();
    r.j(&[&new_above("A")]).ok();
    std::fs::create_dir(r.dir.join("etc")).unwrap();
    r.write("etc/local.env", "A=1\n");
    let mode = |m| std::fs::set_permissions(r.dir.join("etc"), std::fs::Permissions::from_mode(m)).unwrap();
    mode(0o555);
    let out = r.j(&[&new_above("B")]);
    let again = r.j(&["describe \"again\""]);
    mode(0o755);
    for out in [out, again] {
        assert_eq!(out.code, 1, "{}", out.stderr);
        assert!(out.stderr.contains("`etc/local.env` was not written"), "{}", out.stderr);
        assert!(out.stderr.contains("could not be moved"), "{}", out.stderr);
        assert!(out.stderr.contains("Permission denied"), "{}", out.stderr);
    }
    assert_eq!(r.read("etc/local.env"), "A=1\n");
    assert_eq!(aside_in_jj(&r), Vec::<String>::new());
    // once the directory lets it be moved, the run completes the checkout
    r.j(&["describe \"done\""]).ok();
    assert_eq!(r.read("etc/local.env"), "A=1\n");
    assert_eq!(aside_in_jj(&r), Vec::<String>::new());
    r.j(&["id"]).ok();
    let text = "\\r -> text (contentAt [\"etc\" \"local.env\"] (files r))";
    assert_eq!(r.j(&[text]).ok().stdout, "A=1\n");
}

/// An expression that goes to a new commit above the one whose message is
/// `message`
fn new_above(message: &str) -> String {
    format!("new . goto (matching (\\c -> c.message == \"{}\") all)", message)
}

/// The focus's paths
const FOCUS_PATHS: &str = "\\r -> show (map (\\e -> e.path) (files r))";

#[test]
#[cfg(unix)]
fn a_checkout_writes_nothing_over_what_the_directory_does_not_track() {
    // §7.4, §7.5 step 7: jj's checkout writes no file where something
    // stands already, and skips the path without failing, recording it as
    // written, so the next run read what stood there into the focus: an
    // ignored file (a secret, say) in place of the focus's content. The
    // run now refuses before anything is recorded or written, naming what
    // is in the way where the focus adds a file: an ignored symlink above
    // it, an ignored file, a directory in its place holding an ignored
    // file, each of them in one refusal.
    let r = setup();
    r.write(".gitignore", "*.env\nlnk\n");
    r.write("keep.txt", "k\n");
    r.j(&["describe \"A\""]).ok();
    let adds = "mapRoot (\\c -> c { files = c.files ++ [({ path = [\"lnk\" \"f\"], content = blob \"f\\n\" }) \
                ({ path = [\"local.env\"], content = blob \"COMMITTED=1\\n\" }) \
                ({ path = [\"out\"], content = blob \"out\\n\" })] })";
    r.j(&[&format!("describe \"B\" . {} . new", adds)]).ok();
    r.j(&[&new_above("A")]).ok();
    assert!(!r.dir.join("local.env").exists());
    // outside the working directory, and removed with the repository
    let outside = r.cfg.join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, r.dir.join("lnk")).unwrap();
    let secret = "SECRET=hunter2\n";
    r.write("local.env", secret);
    std::fs::create_dir_all(r.dir.join("out/sub")).unwrap();
    r.write("out/junk.env", "junk\n");
    let log = r.j(&["log"]).ok().stdout;
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    let refused = |says: &[&str]| {
        let out = r.j(&[&new_above("B")]);
        assert_eq!(out.code, 1, "{}", out.stderr);
        for said in says {
            assert!(out.stderr.contains(said), "{}", out.stderr);
        }
        assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops);
        assert_eq!(r.j(&["log"]).ok().stdout, log);
        assert!(!has_blob(&r, secret));
    };
    let out_dir = "`out` (a directory holding untracked files, such as `out/junk.env`)";
    refused(&["in the way of files it has: `lnk`, `local.env` and ", out_dir, "; move them aside"]);
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    std::fs::remove_file(r.dir.join("lnk")).unwrap();
    refused(&["in the way of files it has: `local.env` and ", out_dir, "; move them aside"]);
    assert_eq!(r.read("local.env"), secret);
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops);
    assert!(!has_blob(&r, secret));
    let aside = r.cfg.join("local.env");
    std::fs::rename(r.dir.join("local.env"), &aside).unwrap();
    refused(&[
        "it has `out`, and `out` in the working directory is a directory holding untracked files, such as `out/junk.env`; move it aside",
    ]);
    assert_eq!(r.read("out/junk.env"), "junk\n");
    // with those moved aside, B's files are written, over the empty
    // directories left in `out`
    std::fs::remove_file(r.dir.join("out/junk.env")).unwrap();
    r.j(&[&new_above("B")]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    assert_eq!(r.read("lnk/f"), "f\n");
    assert_eq!(r.read("local.env"), "COMMITTED=1\n");
    assert_eq!(r.read("out"), "out\n");
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    let paths = r.j(&[FOCUS_PATHS]).ok().stdout;
    assert_eq!(
        paths.split_whitespace().collect::<Vec<_>>().join(" "),
        r#"[[".gitignore"] ["keep.txt"] ["lnk" "f"] ["local.env"] ["out"]]"#
    );
    assert_eq!(std::fs::read_to_string(&aside).unwrap(), secret);
    assert!(!has_blob(&r, secret));
    // `undo` checks out as persistence does (§7.7)
    r.j(&[&new_above("A")]).ok();
    std::fs::rename(&aside, r.dir.join("local.env")).unwrap();
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    let out = r.j(&["undo"]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("an untracked `local.env` in the working directory is in its way; move it aside"), "{}", out.stderr);
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops);
    assert_eq!(r.read("local.env"), secret);
    std::fs::rename(r.dir.join("local.env"), &aside).unwrap();
    r.j(&["undo"]).ok();
    assert_eq!(r.read("local.env"), "COMMITTED=1\n");
    assert!(!has_blob(&r, secret));
}

#[test]
fn a_refused_checkout_names_all_that_is_in_its_way() {
    // §7.4: a checkout refused over what the working directory does not
    // track named only the first path in its way, so a directory of build
    // outputs took a run per file to clear; and for a directory standing
    // where the focus has a file, it named a file in it, the first a read
    // of the directory found, and said to move that aside. The refusal
    // lists what is in the way now, ten paths and how many more, and a
    // directory by its own path.
    let r = setup();
    let dist = |what: &str| {
        std::fs::create_dir(r.dir.join("dist")).unwrap();
        for i in 1..=12 {
            r.write(&format!("dist/m{:02}.js", i), &format!("{} {}\n", what, i));
        }
    };
    dist("release");
    r.write("build", "#!/bin/sh\n");
    r.write("keep", "k\n");
    r.j(&["describe \"v1\""]).ok();
    r.j(&["new"]).ok();
    std::fs::remove_dir_all(r.dir.join("dist")).unwrap();
    std::fs::remove_file(r.dir.join("build")).unwrap();
    std::fs::create_dir(r.dir.join("build")).unwrap();
    r.write("build/main.c", "c\n");
    r.write(".gitignore", "dist/\n*.o\n");
    r.j(&["describe \"v2\""]).ok();
    r.j(&["new"]).ok();
    // a local build
    dist("dev");
    r.write("build/1.o", "o\n");
    r.write("build/2.o", "o\n");
    let log = r.j(&["log"]).ok().stdout;
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    let out = r.j(&[&new_above("v1")]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    let listed = "in the way of files it has: `build` (a directory holding untracked files, such as `build/";
    assert!(out.stderr.contains(listed), "{}", out.stderr);
    for i in 1..=9 {
        assert!(out.stderr.contains(&format!(", `dist/m{:02}.js`", i)), "{}", out.stderr);
    }
    assert!(!out.stderr.contains("m10"), "{}", out.stderr);
    assert!(out.stderr.contains("`dist/m09.js` and 3 more; move them aside"), "{}", out.stderr);
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops);
    assert_eq!(r.j(&["log"]).ok().stdout, log);
    assert_eq!(r.read("dist/m12.js"), "dev 12\n");
    assert_eq!(r.read("build/2.o"), "o\n");
    // with them moved aside, v1's files are written
    std::fs::remove_dir_all(r.dir.join("dist")).unwrap();
    std::fs::remove_file(r.dir.join("build/1.o")).unwrap();
    std::fs::remove_file(r.dir.join("build/2.o")).unwrap();
    r.j(&[&new_above("v1")]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    assert_eq!(r.read("build"), "#!/bin/sh\n");
    assert_eq!(r.read("dist/m12.js"), "release 12\n");
}

#[test]
fn a_checkout_clears_the_empty_directories_where_it_writes_a_file() {
    // §7.4: a checkout writes no file over a directory, and skips it
    // without failing: where a tool had left empty directories, the next
    // run took the focus's file for deleted. Only a checkout over a stale
    // working copy cleared such directories; every checkout does now,
    // those in a directory the focus replaces by a file included.
    let r = setup();
    r.write("build", "b\n");
    r.write("keep", "k\n");
    r.j(&["describe \"P\""]).ok();
    r.j(&["new"]).ok();
    std::fs::remove_file(r.dir.join("build")).unwrap();
    r.j(&["describe \"Q\""]).ok();
    std::fs::create_dir_all(r.dir.join("build/sub/deeper")).unwrap();
    r.j(&["new . goto parents"]).ok();
    assert_eq!(r.read("build"), "b\n");
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops);
    assert_eq!(r.j(&[FOCUS_PATHS]).ok().stdout.trim(), r#"[["build"] ["keep"]]"#);
    // `lib` holds the tracked `lib/x` and empty directories, and the focus
    // has the file `lib`
    r.write("lib", "lib\n");
    r.j(&["describe \"L\""]).ok();
    r.j(&["new"]).ok();
    std::fs::remove_file(r.dir.join("lib")).unwrap();
    std::fs::create_dir(r.dir.join("lib")).unwrap();
    r.write("lib/x", "x\n");
    r.j(&["describe \"D\""]).ok();
    std::fs::create_dir_all(r.dir.join("lib/e/f")).unwrap();
    r.j(&[&new_above("L")]).ok();
    assert_eq!(r.read("lib"), "lib\n");
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops);
    assert_eq!(r.j(&[FOCUS_PATHS]).ok().stdout.trim(), r#"[["build"] ["keep"] ["lib"]]"#);
}

#[test]
#[cfg(unix)]
fn a_checkout_clears_nothing_through_a_symlink() {
    // §7.4: clearing the empty directories where a checkout over a stale
    // working copy writes a file looked at each path through the symlinks
    // above it: `x`, a symlink to a directory outside the working directory
    // that the focus has as a directory, had it remove an empty tree the
    // symlink's target held
    let r = setup();
    // outside the working directory, and removed with the repository
    let outside = r.cfg.join("outside");
    std::fs::create_dir_all(outside.join("y/z/deeper")).unwrap();
    r.write("keep", "k\n");
    std::os::unix::fs::symlink(&outside, r.dir.join("x")).unwrap();
    r.j(&["describe \"A\""]).ok();
    r.j(&["new"]).ok();
    std::fs::remove_file(r.dir.join("x")).unwrap();
    std::fs::create_dir_all(r.dir.join("x/y")).unwrap();
    r.write("x/y/z", "z\n");
    r.j(&["describe \"B\""]).ok();
    r.j(&[&new_above("A")]).ok();
    assert!(r.dir.join("x").symlink_metadata().unwrap().is_symlink());
    let saved: Vec<(PathBuf, Vec<u8>)> = std::fs::read_dir(r.dir.join(".jj/working_copy"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .map(|p| (p.clone(), std::fs::read(p).unwrap()))
        .collect();
    r.j(&[&new_above("B")]).ok();
    assert!(outside.join("y/z/deeper").is_dir());
    assert_eq!(r.read("x/y/z"), "z\n");
    // the same over a stale working copy, as jj leaves it when it moves the
    // working-copy commit without updating the working directory
    for (path, bytes) in saved {
        std::fs::write(path, bytes).unwrap();
    }
    std::fs::remove_dir_all(r.dir.join("x")).unwrap();
    std::os::unix::fs::symlink(&outside, r.dir.join("x")).unwrap();
    r.j(&["describe \"B2\""]).ok();
    assert!(outside.join("y/z/deeper").is_dir());
    assert_eq!(r.read("x/y/z"), "z\n");
    assert!(r.dir.join("x").symlink_metadata().unwrap().is_dir());
}

/// Run `edit`, whose checkout writes `m/0` … `m/<n-1>` and more after them,
/// and call `meanwhile` with the process stopped while it writes `m`, once
/// its operation is published
#[cfg(target_os = "linux")]
fn stopped_mid_checkout(r: &Repo, edit: &str, n: usize, meanwhile: impl FnOnce()) -> Out {
    stopped_mid_checkout_as(r, edit, n, |_| meanwhile())
}

/// `stopped_mid_checkout`, calling `meanwhile` with the stopped process's
/// id, which may signal it. The process ignores SIGXFSZ, so that a write
/// past a file size limit set on it fails (EFBIG) rather than killing it,
/// and dumps no core. A process a signal ended has as its code minus that
/// signal's number.
#[cfg(target_os = "linux")]
fn stopped_mid_checkout_as(r: &Repo, edit: &str, n: usize, meanwhile: impl FnOnce(libc::pid_t)) -> Out {
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    let mut command = Command::new(j_bin());
    command
        .arg(edit)
        .current_dir(&r.dir)
        .env("XDG_CONFIG_HOME", &r.cfg)
        .env("NO_COLOR", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // an ignored signal stays ignored across exec
    unsafe {
        command.pre_exec(|| {
            libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
            libc::setrlimit(libc::RLIMIT_CORE, &libc::rlimit { rlim_cur: 0, rlim_max: 0 });
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
    let pid = child.id() as libc::pid_t;
    // `m` is written in path order, `0` then `1`, each file created empty
    // and removed once before it is written
    let (first, second) = (r.dir.join("m").join("0"), r.dir.join("m").join("1"));
    while !second.exists() {
        assert!(child.try_wait().unwrap().is_none(), "exited before writing m/1");
        std::thread::yield_now();
    }
    let mut status = 0;
    unsafe {
        libc::kill(pid, libc::SIGSTOP);
        assert_eq!(libc::waitpid(pid, &mut status, libc::WUNTRACED), pid);
    }
    assert!(libc::WIFSTOPPED(status), "exited before it was stopped");
    // all of `m` is written before anything after it
    let written = std::fs::read_dir(r.dir.join("m")).unwrap().count();
    let in_time = first.exists() && written < n;
    let done = if in_time {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| meanwhile(pid)))
    } else {
        Ok(())
    };
    unsafe { libc::kill(pid, if in_time { libc::SIGCONT } else { libc::SIGKILL }) };
    let out = child.wait_with_output().unwrap();
    assert!(in_time, "stopped too late: {} of {} written", written, n);
    if let Err(e) = done {
        std::panic::resume_unwind(e);
    }
    Out {
        code: out.status.code().unwrap_or_else(|| -out.status.signal().unwrap()),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    }
}

/// An edit giving the focus `files` (the language's syntax, over the focus
/// `c`), `m/0` … `m/<n-1>`, and a path the checkout fails on after those
#[cfg(target_os = "linux")]
fn slow_failing_edit(files: &str, n: usize) -> String {
    format!(
        "mapRoot (\\c -> c {{ files = {} ++ map (\\i -> {{ path = [\"m\" (show i)], content = blob \"m\\n\" }}) (range 0 {}) ++ [{{ path = [{}], content = blob \"deep\" }}] }})",
        files,
        n,
        too_deep("zz")
    )
}

/// An edit giving the focus `files` (the language's syntax, over the focus
/// `c`), the empty files `m/0` … `m/<n-1>`, and `zz`, which is not empty:
/// a file created is written in full, and only `zz` is cut short by a
/// full disk (`fill_disk`)
#[cfg(target_os = "linux")]
fn slow_edit(files: &str, n: usize) -> String {
    format!(
        "mapRoot (\\c -> c {{ files = {} ++ map (\\i -> {{ path = [\"m\" (show i)], content = blob \"\" }}) (range 0 {}) ++ [{{ path = [\"zz\"], content = blob \"z\\n\" }}] }})",
        files, n
    )
}

/// Leave the stopped process `pid` no room to write: from here on each
/// write that would make a file longer than nothing fails, as on a full
/// disk, while it can still create, empty and remove files
#[cfg(target_os = "linux")]
fn fill_disk(pid: libc::pid_t) {
    let none = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    assert_eq!(unsafe { libc::prlimit(pid, libc::RLIMIT_FSIZE, &none, std::ptr::null_mut()) }, 0);
}

/// The number of files in `m`
#[cfg(target_os = "linux")]
fn in_m(r: &Repo) -> usize {
    std::fs::read_dir(r.dir.join("m")).map_or(0, |d| d.count())
}

#[test]
#[cfg(target_os = "linux")]
fn an_interrupted_checkout_is_carried_on_by_the_next_run() {
    // §1.3, §7.5 step 7: the checkout ran before the operation was
    // published, so a run killed part way through it (SIGKILL here; a crash,
    // a power cut) left the working copy at the commit the user was on,
    // and the next run recorded into that commit the files the checkout had
    // written. The operation is now published first: the next run takes
    // those files for the focus's own, records only what changed meanwhile,
    // and a run that changes the repository finishes the checkout.
    let r = setup();
    r.write("keep.txt", "k\n");
    r.j(&["describe \"base\""]).ok();
    r.j(&["new"]).ok();
    r.write("work.txt", "my work\n");
    r.j(&["describe \"W\""]).ok();
    r.write("work.txt", "my work, edited\n");
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    let n = 2000;
    // S, a new child of base: `work.txt`, after `m`, is still there when
    // the run is killed
    let edit = format!("describe \"S\" . ({}) . new . prev", slow_edit("c.files", n));
    let out = stopped_mid_checkout_as(&r, &edit, n, |pid| {
        r.write("mine.txt", "mine\n");
        unsafe { libc::kill(pid, libc::SIGKILL) };
    });
    assert_eq!(out.code, -libc::SIGKILL, "{}", out.stderr);
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    assert!(in_m(&r) < n);
    assert_eq!(r.read("work.txt"), "my work, edited\n");
    // W holds the edit, and S only what was made meanwhile
    let paths = "\\r -> show (map (\\m -> map (\\e -> e.path) (restrict (neg (under ./m)) \
                 (focus (goto (matching (\\c -> c.message == m) all) r)).files)) [\"base\" \"W\" \"S\"])";
    let paths = r.j(&[paths]).ok().stdout;
    assert_eq!(
        paths.split_whitespace().collect::<Vec<_>>().join(" "),
        r#"[[["keep.txt"]] [["keep.txt"] ["work.txt"]] [["keep.txt"] ["mine.txt"] ["zz"]]]"#
    );
    let text = "\\r -> text (contentAt [\"work.txt\"] (files (goto (matching (\\c -> c.message == \"W\") all) r)))";
    assert_eq!(r.j(&[text]).ok().stdout, "my work, edited\n");
    r.j(&["describe \"S2\""]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 2);
    assert!(!r.dir.join("work.txt").exists());
    assert_eq!(in_m(&r), n);
    assert_eq!(r.read("zz"), "z\n");
    assert_eq!(r.read("mine.txt"), "mine\n");
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 2);
}

#[test]
#[cfg(target_os = "linux")]
fn a_checkout_asked_to_stop_completes_before_the_run_stops() {
    // §1.3: with the operation recorded before its checkout, a run stopped
    // part way through the checkout by Ctrl-C (SIGINT), SIGTERM, SIGHUP or
    // SIGQUIT left the working directory holding neither the old focus nor
    // the new one. Such a signal now waits until the checkout and the
    // working copy's state are saved, and then stops the run as it would
    // have: the directory holds the focus, and nothing is left to carry on.
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT] {
        let r = setup();
        r.write("keep.txt", "k\n");
        r.j(&["describe \"base\""]).ok();
        r.j(&["new"]).ok();
        r.write("work.txt", "my work\n");
        r.j(&["describe \"W\""]).ok();
        r.write("work.txt", "my work, edited\n");
        let ops = r.j(&["ops"]).ok().stdout.lines().count();
        let n = 2000;
        // S, a new child of base: the checkout removes `work.txt` after it
        // has written `m`
        let edit = format!("describe \"S\" . ({}) . new . prev", slow_edit("c.files", n));
        let out = stopped_mid_checkout_as(&r, &edit, n, |pid| {
            assert_eq!(unsafe { libc::kill(pid, sig) }, 0);
        });
        assert_eq!(out.code, -sig, "signal {}: {}", sig, out.stderr);
        assert_eq!(in_m(&r), n, "signal {}", sig);
        assert_eq!(r.read("zz"), "z\n");
        assert!(!r.dir.join("work.txt").exists());
        assert_eq!(r.read("keep.txt"), "k\n");
        assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
        r.j(&["id"]).ok();
        assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1, "signal {}", sig);
        let texts = r.j(&[&texts_by_message(&["[\"work.txt\"]"], &["W"])]).ok().stdout;
        assert_eq!(
            texts.split_whitespace().collect::<Vec<_>>().join(" "),
            r#"[["my work, edited\n"]]"#
        );
    }
}

#[test]
#[cfg(target_os = "linux")]
fn a_failed_checkout_keeps_what_changed_meanwhile() {
    // §7.5 step 7: the put-back rescanned the whole directory, so a file
    // edited or created while the checkout ran could be taken back with
    // what the checkout wrote. Nothing is put back now, and the next run
    // records what changed meanwhile into the focus, beside what the
    // checkout wrote and the file the full disk cut short.
    let r = setup();
    r.write("keep.txt", "k\n");
    r.j(&["describe \"base\""]).ok();
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    let n = 2000;
    let out = stopped_mid_checkout_as(&r, &slow_edit("c.files", n), n, |pid| {
        fill_disk(pid);
        r.write("keep.txt", "edited meanwhile\n");
        r.write("mine.txt", "mine\n");
        std::fs::create_dir(r.dir.join("notes")).unwrap();
        r.write("notes/todo.md", "my notes\n");
    });
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("cannot check out"), "{}", out.stderr);
    assert!(out.stderr.contains("the operation is recorded"), "{}", out.stderr);
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    assert_eq!(r.read("keep.txt"), "edited meanwhile\n");
    assert_eq!(r.read("mine.txt"), "mine\n");
    assert_eq!(r.read("notes/todo.md"), "my notes\n");
    r.j(&["id"]).ok();
    let texts = "\\r -> show (map (\\e -> [e.path (text e.content)]) (restrict (neg (under ./m)) (files r)))";
    let texts = r.j(&[texts]).ok().stdout;
    let texts = texts.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        texts.starts_with(r#"[[["keep.txt"] "edited meanwhile\n"] [["mine.txt"] "mine\n"] [["notes" "todo.md"] "my notes\n"] [["zz"] "#),
        "{}",
        texts
    );
    assert_eq!(in_m(&r), n);
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 2);
}

#[test]
#[cfg(target_os = "linux")]
fn a_failed_checkout_keeps_what_was_saved_meanwhile_where_it_wrote() {
    // §7.5 step 7: the put-back gave each path the focus changes back what
    // the directory held there, so a file saved meanwhile at such a path
    // after the checkout wrote it, or at a path the focus adds before the
    // checkout got there, was lost unless the put-back recognised what the
    // checkout had written. Nothing is put back now: the next run takes the
    // directory for a change to what it held when the checkout began, and
    // each such file is a conflict with the focus's content, neither lost.
    // So is the file the full disk cut short. (A file the focus changes,
    // saved before the checkout reaches it, is written over, as in jj.)
    let r = setup();
    r.write("a.txt", "a\n");
    r.write("b.txt", "b\n");
    r.j(&["describe \"base\""]).ok();
    r.write("b.txt", "b edited\n");
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    let n = 2000;
    // `a.txt` and `b.txt` are written before `m` and saved over after;
    // `n.txt`, which the focus adds after `m`, is saved before the
    // checkout gets there; then `zz` fails on the full disk
    let files = "[({ path = [\"a.txt\"], content = blob \"focus a\\n\" }) \
                 ({ path = [\"b.txt\"], content = blob \"focus b\\n\" }) \
                 ({ path = [\"n.txt\"], content = blob \"focus n\\n\" })]";
    let out = stopped_mid_checkout_as(&r, &slow_edit(files, n), n, |pid| {
        fill_disk(pid);
        r.write("a.txt", "mine a\n");
        r.write("b.txt", "mine b\n");
        r.write("n.txt", "mine n\n");
    });
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("cannot check out"), "{}", out.stderr);
    assert!(out.stderr.contains("the operation is recorded"), "{}", out.stderr);
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    assert_eq!(r.read("a.txt"), "mine a\n");
    assert_eq!(r.read("b.txt"), "mine b\n");
    assert_eq!(r.read("n.txt"), "mine n\n");
    assert_eq!(r.read("zz"), "");
    r.j(&["id"]).ok();
    let conflicted = r.j(&["\\r -> show (conflicted (files r))"]).ok().stdout;
    assert_eq!(conflicted.trim(), r#"[["a.txt"] ["b.txt"] ["n.txt"] ["zz"]]"#);
    for (path, mine, focus) in [("a.txt", "mine a", "focus a"), ("b.txt", "mine b", "focus b"), ("n.txt", "mine n", "focus n")] {
        let text = r.read(path);
        assert!(text.contains(mine) && text.contains(focus), "{}: {}", path, text);
    }
    assert!(r.read("zz").contains("z\n"), "{}", r.read("zz"));
    assert_eq!(in_m(&r), n);
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 2);
}

/// Whether the repository's git store holds `content` as a loose object,
/// as each object j writes is
fn has_blob(r: &Repo, content: &str) -> bool {
    let id = gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::objs::Kind::Blob, content.as_bytes())
        .unwrap()
        .to_string();
    r.dir.join(".git/objects").join(&id[..2]).join(&id[2..]).exists()
}

#[test]
#[cfg(target_os = "linux")]
fn an_ignored_file_saved_meanwhile_where_the_focus_adds_one_is_left_alone() {
    // §7.5 step 7: an ignored file saved meanwhile at a path the focus adds
    // is left alone. The put-back knew the file by its inode, so one an
    // editor saved meanwhile by renaming a new file over the one the
    // checkout wrote was taken for the checkout's own, read into the store
    // and deleted. Neither the failed checkout, which writes no file over
    // one saved before it reached the path, nor the runs after it read or
    // remove such a file; those the checkout wrote stay, ignored. (One
    // there before the run refuses it, before anything is recorded:
    // `a_checkout_writes_nothing_over_what_the_directory_does_not_track`.)
    let r = setup();
    r.write(".gitignore", "*.env\n");
    r.write("keep.txt", "k\n");
    r.j(&["describe \"base\""]).ok();
    let log = r.j(&["log"]).ok().stdout;
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    let n = 2000;
    // the checkout writes `k.env` and `local.env` before `m`, and `o.env`
    // and `p.env` after it
    let files = "c.files ++ [({ path = [\"k.env\"], content = blob \"committed k\\n\" }) \
                 ({ path = [\"local.env\"], content = blob \"committed\\n\" }) \
                 ({ path = [\"o.env\"], content = blob \"committed o\\n\" }) \
                 ({ path = [\"p.env\"], content = blob \"committed p\\n\" })]";
    let saved = "SECRET=v2, saved meanwhile\n";
    let out = stopped_mid_checkout(&r, &slow_failing_edit(files, n), n, || {
        r.write("local.env.tmp", saved);
        std::fs::rename(r.dir.join("local.env.tmp"), r.dir.join("local.env")).unwrap();
        r.write("o.env.tmp", "committed");
        std::fs::rename(r.dir.join("o.env.tmp"), r.dir.join("o.env")).unwrap();
    });
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("cannot check out"), "{}", out.stderr);
    assert!(out.stderr.contains("the operation is recorded"), "{}", out.stderr);
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    let unchanged = |r: &Repo| {
        assert_eq!(r.read("k.env"), "committed k\n");
        assert_eq!(r.read("local.env"), saved);
        assert_eq!(r.read("o.env"), "committed");
        assert_eq!(r.read("p.env"), "committed p\n");
        assert!(!has_blob(r, saved));
        assert!(!has_blob(r, "committed"));
    };
    unchanged(&r);
    // the run wrote the focus's objects before its checkout
    assert!(has_blob(&r, "committed p\n"));
    r.j(&["log"]).ok();
    unchanged(&r);
    r.j(&["undo"]).ok();
    unchanged(&r);
    assert!(!r.dir.join("m").exists());
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 2);
    assert_eq!(r.j(&["log"]).ok().stdout, log);
}

#[test]
#[cfg(target_os = "linux")]
fn an_ignored_file_put_where_a_checkout_cut_short_adds_one_is_not_read() {
    // §7.4, §7.5 step 7: the next run that persisted after a checkout cut
    // short completed it, and jj skipped the path where an ignored file had
    // been put meanwhile, recording it as written: the run after that read
    // the file into the focus. That run refuses now, naming the file, and
    // no run reads it. What the checkout wrote at an ignored path before it
    // was cut short, the focus's own, is written again.
    let r = setup();
    r.write(".gitignore", "*.env\n");
    r.write("keep.txt", "k\n");
    r.j(&["describe \"base\""]).ok();
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    let n = 2000;
    // `k.env` is written before `m`, `o.env` after it
    let files = "c.files ++ [({ path = [\"k.env\"], content = blob \"committed k\\n\" }) \
                 ({ path = [\"o.env\"], content = blob \"committed\\n\" })]";
    let secret = "SECRET=hunter2\n";
    let out = stopped_mid_checkout_as(&r, &slow_edit(files, n), n, |pid| {
        r.write("o.env", secret);
        unsafe { libc::kill(pid, libc::SIGKILL) };
    });
    assert_eq!(out.code, -libc::SIGKILL, "{}", out.stderr);
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    assert!(in_m(&r) < n);
    assert_eq!(r.read("k.env"), "committed k\n");
    for run in ["log", "id"] {
        r.j(&[run]).ok();
        assert!(!has_blob(&r, secret));
    }
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    let out = r.j(&["describe \"S\""]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("untracked `o.env`"), "{}", out.stderr);
    assert!(out.stderr.contains("move it aside"), "{}", out.stderr);
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    assert_eq!(r.read("o.env"), secret);
    r.j(&["id"]).ok();
    assert!(!has_blob(&r, secret));
    // moved aside, the run completes the checkout
    let aside = r.cfg.join("o.env");
    std::fs::rename(r.dir.join("o.env"), &aside).unwrap();
    r.j(&["describe \"S\""]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 2);
    assert_eq!(r.read("k.env"), "committed k\n");
    assert_eq!(r.read("o.env"), "committed\n");
    assert_eq!(in_m(&r), n);
    assert_eq!(r.read("zz"), "z\n");
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 2);
    assert!(!has_blob(&r, secret));
}

#[test]
#[cfg(target_os = "linux")]
fn a_file_a_checkout_cut_short_moved_aside_is_put_back_by_the_next_run() {
    // §7.4, §7.5 step 7: an untracked file holding just what the checkout
    // writes at a path the focus adds is moved aside into `.jj` for the
    // checkout, and put back where it does not get to write; a run killed
    // meanwhile leaves it there, which the next run puts back, and `j undo`
    // leaves it where it was
    let r = setup();
    r.write(".gitignore", "*.env\n");
    r.write("keep.txt", "k\n");
    r.j(&["describe \"base\""]).ok();
    r.write("o.env", "committed\n");
    let log = r.j(&["log"]).ok().stdout;
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    let n = 2000;
    // `o.env` is written after `m`
    let files = "c.files ++ [{ path = [\"o.env\"], content = blob \"committed\\n\" }]";
    let out = stopped_mid_checkout_as(&r, &slow_edit(files, n), n, |pid| {
        unsafe { libc::kill(pid, libc::SIGKILL) };
    });
    assert_eq!(out.code, -libc::SIGKILL, "{}", out.stderr);
    assert!(in_m(&r) < n);
    assert!(!r.dir.join("o.env").exists());
    assert_eq!(aside_in_jj(&r).len(), 1);
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    assert_eq!(r.read("o.env"), "committed\n");
    assert_eq!(aside_in_jj(&r), Vec::<String>::new());
    r.j(&["undo"]).ok();
    assert_eq!(r.read("o.env"), "committed\n");
    assert!(!r.dir.join("m").exists());
    assert_eq!(aside_in_jj(&r), Vec::<String>::new());
    assert_eq!(r.j(&["log"]).ok().stdout, log);
}

#[test]
#[cfg(target_os = "linux")]
fn an_ignored_file_saved_meanwhile_where_a_checkout_adds_one_fails_it() {
    // §7.4, §7.5 step 7: a file saved while the checkout runs, at a path
    // it adds after that, is in its way, and jj skipped the path, recording
    // it as written: the next run read an ignored one into the focus. The
    // checkout fails now, naming the path, and leaves the working copy
    // stale, so no run reads the file.
    let r = setup();
    r.write(".gitignore", "*.env\n");
    r.write("keep.txt", "k\n");
    r.j(&["describe \"base\""]).ok();
    let log = r.j(&["log"]).ok().stdout;
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    let n = 2000;
    // `o.env` is written after `m`
    let files = "c.files ++ [{ path = [\"o.env\"], content = blob \"committed\\n\" }]";
    let secret = "SECRET=hunter2\n";
    let out = stopped_mid_checkout(&r, &slow_edit(files, n), n, || r.write("o.env", secret));
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("`o.env`"), "{}", out.stderr);
    assert!(out.stderr.contains("the operation is recorded"), "{}", out.stderr);
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    // the checkout carried on past it
    assert_eq!(in_m(&r), n);
    assert_eq!(r.read("zz"), "z\n");
    assert_eq!(r.read("o.env"), secret);
    for run in ["log", "id"] {
        r.j(&[run]).ok();
        assert!(!has_blob(&r, secret));
    }
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    let text = "\\r -> text (contentAt [\"o.env\"] (files r))";
    assert_eq!(r.j(&[text]).ok().stdout, "committed\n");
    let out = r.j(&["describe \"S\""]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("untracked `o.env`"), "{}", out.stderr);
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    r.j(&["undo"]).ok();
    assert_eq!(r.j(&["log"]).ok().stdout, log);
    assert!(!r.dir.join("m").exists());
    assert_eq!(r.read("o.env"), secret);
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 2);
    assert!(!has_blob(&r, secret));
}

/// Run `edit`, which must take a while to evaluate, over a directory with a
/// change to record, and call `meanwhile` with the process stopped once the
/// snapshot's operation is written, before it publishes one
#[cfg(target_os = "linux")]
fn stopped_before_publish(r: &Repo, edit: &str, meanwhile: impl FnOnce()) -> Out {
    let ops_dir = r.dir.join(".jj/repo/op_store/operations");
    let heads_dir = r.dir.join(".jj/repo/op_heads/heads");
    let count = || std::fs::read_dir(&ops_dir).unwrap().count();
    let heads = || {
        let mut names: Vec<_> = std::fs::read_dir(&heads_dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        names.sort();
        names
    };
    let (before, published) = (count(), heads());
    let mut child = Command::new(j_bin())
        .arg(edit)
        .current_dir(&r.dir)
        .env("XDG_CONFIG_HOME", &r.cfg)
        .env("NO_COLOR", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id() as libc::pid_t;
    while count() == before {
        assert!(child.try_wait().unwrap().is_none(), "exited before its snapshot");
        std::thread::yield_now();
    }
    let mut status = 0;
    unsafe {
        libc::kill(pid, libc::SIGSTOP);
        assert_eq!(libc::waitpid(pid, &mut status, libc::WUNTRACED), pid);
    }
    assert!(libc::WIFSTOPPED(status), "exited before it was stopped");
    let in_time = heads() == published;
    let done = if in_time {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(meanwhile))
    } else {
        Ok(())
    };
    unsafe { libc::kill(pid, if in_time { libc::SIGCONT } else { libc::SIGKILL }) };
    let out = child.wait_with_output().unwrap();
    assert!(in_time, "stopped too late: an operation was published");
    if let Err(e) = done {
        std::panic::resume_unwind(e);
    }
    Out {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    }
}

#[test]
#[cfg(target_os = "linux")]
fn a_run_that_fails_to_publish_writes_nothing() {
    // §7.5 step 7: the operation is published before the checkout, so one
    // that fails to publish records nothing and writes nothing: the
    // directory, its ignored files and its uncommitted edit are as they
    // were, and none of the ignored files is read into the store
    let r = setup();
    r.write(".gitignore", "*.env\n");
    r.write("keep.txt", "k\n");
    r.j(&["describe \"base\""]).ok();
    let secret = "SECRET=hunter2\n";
    r.write("local.env", secret);
    r.write("keep.txt", "my uncommitted edit\n");
    let log = r.j(&["log"]).ok().stdout;
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    // an ignored path the focus adds, where nothing stands: one where
    // `local.env` stands would refuse the run before it tries to publish
    let edit = "mapRoot (\\c -> c { message = show (foldl (+) 0 (range 0 300000)), \
                files = c.files ++ [({ path = [\"b\"], content = blob \"b\\n\" }) \
                ({ path = [\"other.env\"], content = blob \"committed\\n\" })] })";
    // a file where jj keeps its operation heads: publishing fails
    let heads = r.dir.join(".jj/repo/op_heads/heads");
    let aside = heads.with_extension("aside");
    let out = stopped_before_publish(&r, edit, || {
        std::fs::rename(&heads, &aside).unwrap();
        std::fs::write(&heads, "").unwrap();
    });
    std::fs::remove_file(&heads).unwrap();
    std::fs::rename(&aside, &heads).unwrap();
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("cannot publish"), "{}", out.stderr);
    assert!(!out.stderr.contains("recorded"), "{}", out.stderr);
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops);
    assert_eq!(r.j(&["log"]).ok().stdout, log);
    assert_eq!(r.read("local.env"), secret);
    assert_eq!(r.read("keep.txt"), "my uncommitted edit\n");
    assert!(!r.dir.join("b").exists());
    assert!(!r.dir.join("other.env").exists());
    assert!(!has_blob(&r, secret));
    // the run wrote the focus's objects before it tried to publish
    assert!(has_blob(&r, "committed\n"));
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    let text = r.j(&["\\r -> text (contentAt [\"keep.txt\"] (files r))"]).ok().stdout;
    assert_eq!(text, "my uncommitted edit\n");
}

#[test]
#[cfg(target_os = "linux")]
fn a_file_saved_while_the_program_runs_is_not_written_over() {
    // §7.5 step 7: the checkout's scan found a file saved after the run's
    // snapshot, while the program ran, and took the directory to hold the
    // snapshot there all the same; where the focus has that file otherwise,
    // the checkout wrote the focus's over the edit, which no snapshot or
    // operation held. Such a run now crashes before its operation is
    // recorded, naming the path, and writes nothing, and the next run
    // records the edit. One saved where the focus has what the snapshot
    // read is left, as before, for the next run to record.
    let r = setup();
    r.write("a", "base\n");
    r.write("same.txt", "same\n");
    r.j(&["describe \"P\""]).ok();
    r.j(&["new"]).ok();
    r.write("a", "child\n");
    r.j(&["describe \"C\""]).ok();
    // a change for the run's snapshot to record, whose operation
    // `stopped_before_publish` waits for
    r.write("keep.txt", "uncommitted\n");
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    // back to P, which has `a` otherwise, after a slow evaluation
    let edit = "prev . mapRoot (\\c -> c { message = show (foldl (+) 0 (range 0 300000)) })";
    let out = stopped_before_publish(&r, edit, || r.write("a", "saved meanwhile\n"));
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("`a` changed in the working directory"), "{}", out.stderr);
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops);
    assert_eq!(r.read("a"), "saved meanwhile\n");
    assert_eq!(r.read("keep.txt"), "uncommitted\n");
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    let texts = r.j(&[&texts_by_message(&["[\"a\"]", "[\"keep.txt\"]"], &["C"])]).ok().stdout;
    assert_eq!(
        texts.split_whitespace().collect::<Vec<_>>().join(" "),
        r#"[["saved meanwhile\n" "uncommitted\n"]]"#
    );
    // `same.txt`, which P has as the snapshot read it
    r.write("keep.txt", "uncommitted, again\n");
    let out = stopped_before_publish(&r, edit, || r.write("same.txt", "saved meanwhile\n"));
    out.ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 2);
    assert_eq!(r.read("same.txt"), "saved meanwhile\n");
    assert_eq!(r.read("a"), "base\n");
    assert!(!r.dir.join("keep.txt").exists());
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 3);
    let texts = r.j(&[&texts_by_message(&["[\"a\"]", "[\"same.txt\"]"], &["P"])]).ok().stdout;
    assert_eq!(
        texts.split_whitespace().collect::<Vec<_>>().join(" "),
        r#"[["base\n" "saved meanwhile\n"]]"#
    );
}

#[test]
#[cfg(target_os = "linux")]
fn a_checkout_that_fills_the_disk_is_recorded_and_carried_on_from() {
    // §7.5 step 7: a checkout that used up the disk was put back, and the
    // put-back had to write too; where it could not, the edit the checkout
    // had written over was recorded alone, or lost. The operation, which
    // holds the edit, is recorded before the checkout begins, and the next
    // run takes what the checkout wrote for the focus's own, and the file
    // it cut short for a conflict with what the focus holds there.
    let r = setup();
    r.write("keep.txt", "k\n");
    r.j(&["describe \"base\""]).ok();
    r.write("keep.txt", "my uncommitted edit\n");
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    let n = 2000;
    let out = stopped_mid_checkout_as(&r, &slow_edit("c.files", n), n, fill_disk);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("cannot check out"), "{}", out.stderr);
    assert!(out.stderr.contains("the operation is recorded"), "{}", out.stderr);
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    assert_eq!(r.read("keep.txt"), "my uncommitted edit\n");
    assert_eq!(in_m(&r), n);
    assert_eq!(r.read("zz"), "");
    r.j(&["id"]).ok();
    let conflicted = r.j(&["\\r -> show (conflicted (files r))"]).ok().stdout;
    assert_eq!(conflicted.trim(), r#"[["zz"]]"#);
    let text = r.j(&["\\r -> text (contentAt [\"keep.txt\"] (files r))"]).ok().stdout;
    assert_eq!(text, "my uncommitted edit\n");
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 2);
}

#[test]
#[cfg(target_os = "linux")]
fn a_fetch_after_a_failed_checkout_leaves_every_command_working() {
    // §7.4, §7.6: when a checkout that filled the disk could not be put
    // back, the snapshot it had written over was recorded alone, leaving
    // the working-copy state behind the commit; one `j fetch` or `j push`
    // after that, and every run, `j log` included, exited 2 as over a stale
    // working copy, and `j undo` was refused. A failed checkout now leaves
    // its operation recorded and the state stale, which refuses nothing.
    let r = setup();
    r.write("a", "a\n");
    r.j(&["describe \"B\""]).ok();
    r.j(&["new"]).ok();
    r.j(&["describe \"W\""]).ok();
    r.write("a", "my uncommitted edit\n");
    let n = 2000;
    // S, a new child of B, writes `a` before `m`
    let files = "[{ path = [\"a\"], content = blob \"s\\n\" }]";
    let edit = format!("describe \"S\" . ({}) . new . prev", slow_edit(files, n));
    let out = stopped_mid_checkout_as(&r, &edit, n, fill_disk);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("the operation is recorded"), "{}", out.stderr);
    let origin = r.cfg.join("origin.git");
    let git = Command::new("git").args(["init", "-q", "--bare"]).arg(&origin).status().unwrap();
    assert!(git.success());
    r.j(&["remote", origin.to_str().unwrap()]).ok();
    r.j(&["fetch"]).ok();
    r.j(&["log"]).ok();
    let texts = r.j(&[&texts_by_message(&["[\"a\"]"], &["B", "W", "S"])]).ok().stdout;
    assert_eq!(
        texts.split_whitespace().collect::<Vec<_>>().join(" "),
        r#"[["a\n"] ["my uncommitted edit\n"] ["s\n"]]"#
    );
    r.j(&["id"]).ok();
    r.j(&["describe \"S2\""]).ok();
    r.j(&["goto (matching (\\c -> c.message == \"W\") all)"]).ok();
    assert_eq!(r.read("a"), "my uncommitted edit\n");
    r.j(&["undo"]).ok();
    assert_eq!(r.read("a"), "s\n");
}

#[test]
#[cfg(target_os = "linux")]
fn a_failed_checkout_keeps_the_edit_of_an_immutable_focus_in_a_child() {
    // §7.2, §7.5 step 7: with an immutable working-copy commit, a checkout
    // that filled the disk could not be put back, and the snapshot it had
    // written over, recorded instead in a new child of the commit, needed
    // room on the disk as well: the uncommitted edit was lost. The
    // operation that holds the child is now recorded before the checkout.
    let r = setup();
    let cfg = r.cfg.join("j/config.j");
    let config = std::fs::read_to_string(&cfg).unwrap();
    let immutable = "immutable = ancestorsOf trunk\n";
    assert!(config.contains(immutable));
    let config = config.replace(immutable, "immutable = matching (\\c -> c.message == \"W\") all\n");
    std::fs::write(&cfg, config).unwrap();
    r.write("a", "a\n");
    r.j(&["describe \"B\""]).ok();
    r.j(&["new"]).ok();
    r.j(&["describe \"W\""]).ok();
    r.write("a", "my uncommitted edit\n");
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    let n = 2000;
    // from the child of W that holds the edit, S, a new child of B
    let files = "[{ path = [\"a\"], content = blob \"s\\n\" }]";
    let edit = format!("describe \"S\" . ({}) . new . prev . prev", slow_edit(files, n));
    let out = stopped_mid_checkout_as(&r, &edit, n, fill_disk);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("the operation is recorded"), "{}", out.stderr);
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    let files = r.j(&[CONTENT_OF_A]).ok().stdout;
    assert_eq!(
        files.split_whitespace().collect::<Vec<_>>().join(" "),
        r#"[["" (blob "")] ["B" (blob "a\n")] ["W" (blob "a\n")] ["" (blob "my uncommitted edit\n")] ["S" (blob "s\n")]]"#
    );
    r.j(&["goto (matching (\\c -> contentAt [\"a\"] c.files == blob \"my uncommitted edit\\n\") all)"]).ok();
    assert_eq!(r.read("a"), "my uncommitted edit\n");
}

#[test]
fn checkouts_that_fail_in_a_row_leave_each_edit_in_its_commit() {
    // §7.4, §7.5 step 7: a failed checkout leaves the working-copy state
    // stale, recording what the directory held when the checkout began, so
    // the runs after it take only what changed since for their changes:
    // here an uncommitted edit before each of two failed checkouts, each
    // recorded into the commit it was made on
    let r = setup();
    r.write("a", "a\n");
    r.write("d", "d\n");
    r.j(&["describe \"base\""]).ok();
    r.j(&["new"]).ok();
    r.j(&["describe \"W\""]).ok();
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    let deep = format!("{{ path = [{}], content = blob \"deep\" }}", too_deep("zz"));
    // N1, a new child of W, then N2, a new child of W beside it
    for (path, edit, name, to) in [("a", "edit 1\n", "N1", "new"), ("d", "edit 2\n", "N2", "new . prev")] {
        r.write(path, edit);
        let e = format!("describe \"{}\" . mapRoot (\\c -> c {{ files = c.files ++ [{}] }}) . {}", name, deep, to);
        let out = r.j(&[&e]);
        assert_eq!(out.code, 1, "{}", out.stderr);
        assert!(out.stderr.contains("the operation is recorded"), "{}", out.stderr);
    }
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 2);
    // N2's `d` was written before its path under `zz` failed
    assert_eq!(r.read("a"), "edit 1\n");
    assert_eq!(r.read("d"), "d\n");
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 2);
    let texts = r.j(&[&texts_by_message(&["[\"a\"]", "[\"d\"]"], &["base", "W", "N1", "N2"])]).ok().stdout;
    assert_eq!(
        texts.split_whitespace().collect::<Vec<_>>().join(" "),
        r#"[["a\n" "d\n"] ["edit 1\n" "d\n"] ["edit 1\n" "edit 2\n"] ["edit 1\n" "d\n"]]"#
    );
}

#[test]
#[cfg(target_os = "linux")]
fn a_failed_checkout_leaves_a_nested_repository_alone() {
    // §7.5 step 7: no run reads a directory holding `.git` or `.jj`, so
    // what a checkout writes there is never recorded; the put-back removed
    // what a failed one had written there and named what it left. Nothing
    // there is removed now: not what was there before, not what was saved
    // meanwhile, and, by `j undo`, not what the checkout wrote either.
    let r = setup();
    r.write("keep.txt", "k\n");
    r.j(&["describe \"base\""]).ok();
    std::fs::create_dir(r.dir.join("lib")).unwrap();
    std::fs::create_dir(r.dir.join("lib/.git")).unwrap();
    r.write("lib/own.c", "own\n");
    let log = r.j(&["log"]).ok().stdout;
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    let n = 2000;
    // `lib/x.c` and `lib/y.c` are written before `m`; `lib/y.c` is saved
    // over meanwhile
    let files = "c.files ++ [({ path = [\"lib\" \"x.c\"], content = blob \"x\\n\" }) \
                 ({ path = [\"lib\" \"y.c\"], content = blob \"y\\n\" })]";
    let out = stopped_mid_checkout(&r, &slow_failing_edit(files, n), n, || {
        r.write("lib/y.c", "mine\n");
    });
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("cannot check out"), "{}", out.stderr);
    assert!(out.stderr.contains("the operation is recorded"), "{}", out.stderr);
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    r.j(&["undo"]).ok();
    assert_eq!(r.j(&["log"]).ok().stdout, log);
    assert!(!r.dir.join("m").exists());
    assert_eq!(r.read("lib/own.c"), "own\n");
    assert_eq!(r.read("lib/y.c"), "mine\n");
    assert_eq!(r.read("lib/x.c"), "x\n");
}

#[test]
#[cfg(target_os = "linux")]
fn a_failed_checkout_removes_nothing_through_a_symlink() {
    // §7.5 step 7, §7.7: the put-back removed what the checkout had added
    // one file at a time; a directory the checkout made, replaced meanwhile
    // by a symlink to one elsewhere holding the same file, was in the way.
    // Nothing is removed after a failed checkout now, and the symlink is
    // the user's change, which `j undo` refuses to write over.
    let r = setup();
    // where the filesystem holds no symlink, none is followed
    if std::os::unix::fs::symlink("x", r.dir.join("probe")).is_err() {
        return;
    }
    std::fs::remove_file(r.dir.join("probe")).unwrap();
    r.write("keep.txt", "k\n");
    r.j(&["describe \"base\""]).ok();
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    let n = 2000;
    // `d/f` is written before `m`
    let files = "c.files ++ [{ path = [\"d\" \"f\"], content = blob \"f\\n\" }]";
    // outside the working directory, and removed with the repository
    let elsewhere = r.cfg.join("elsewhere");
    let out = stopped_mid_checkout(&r, &slow_failing_edit(files, n), n, || {
        std::fs::rename(r.dir.join("d"), &elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, r.dir.join("d")).unwrap();
    });
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("the operation is recorded"), "{}", out.stderr);
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    let out = r.j(&["undo"]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("working copy has changes not in @"), "{}", out.stderr);
    assert_eq!(std::fs::read_to_string(elsewhere.join("f")).unwrap(), "f\n");
    assert!(r.dir.join("d").symlink_metadata().unwrap().is_symlink());
}

#[test]
fn a_file_a_failed_checkout_replaced_by_directories_comes_back() {
    // §7.5 step 7: the checkout removed the file `zz` and made directories
    // for a path under it until the path grew too long. No scan sees empty
    // directories, but a checkout writes no file over one: `j undo` skipped
    // writing `zz` back without an error, and the next run recorded it as
    // deleted. A checkout removes such a tree of empty directories where it
    // writes a file.
    let r = setup();
    r.write("zz", "precious\n");
    r.write("a", "k\n");
    r.j(&["describe \"W\""]).ok();
    let log = r.j(&["log"]).ok().stdout;
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    let edit = format!(
        "mapRoot (\\c -> c {{ files = filter (\\e -> e.path /= [\"zz\"]) c.files ++ [{{ path = [{}], content = blob \"deep\" }}] }})",
        too_deep("zz")
    );
    let out = r.j(&[&edit]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("cannot check out"), "{}", out.stderr);
    assert!(out.stderr.contains("the operation is recorded"), "{}", out.stderr);
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    assert!(r.dir.join("zz").is_dir());
    r.j(&["undo"]).ok();
    assert_eq!(r.j(&["log"]).ok().stdout, log);
    assert_eq!(r.read("zz"), "precious\n");
    let paths = r.j(&["\\r -> show (map (\\e -> e.path) (files r))"]).ok().stdout;
    assert_eq!(paths.trim(), r#"[["a"] ["zz"]]"#);
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 2);
}

#[test]
fn a_conflict_a_failed_checkout_wrote_is_read_back_as_that_conflict() {
    // §7.4, §7.5 step 7: a checkout records a conflict in the working-copy
    // state as it writes its markers, so a checkout that fails records none
    // of those it wrote, and the next run read each file as the text of its
    // markers: a conflict nested in the focus's, which `j undo` refused to
    // write over. A file holding exactly what the checkout writes for the
    // focus's conflict is read back as that conflict, markers made longer
    // than any line of its sides that looks like one included.
    let r = setup();
    r.write("a.txt", "base\n");
    r.write("keep.txt", "k\n");
    r.j(&["describe \"base\""]).ok();
    r.j(&["new"]).ok();
    r.write("a.txt", "<<<<<<< left\n");
    r.j(&["describe \"left\""]).ok();
    r.j(&["new . goto parents"]).ok();
    r.write("a.txt", "right\n");
    r.j(&["describe \"right\""]).ok();
    r.j(&["rebase siblings"]).ok();
    let conflicted = "\\r -> show (conflicted (files r))";
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"a.txt\"]]");
    let log = r.j(&["log"]).ok().stdout;
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    // the conflict is written to `c.txt` and over `keep.txt` too, then the
    // deep path fails
    let conflict = "(filter (\\e -> e.path == [\"a.txt\"]) c.files)";
    let edit = format!(
        "mapRoot (\\c -> c {{ files = filter (\\e -> e.path /= [\"keep.txt\"]) c.files \
         ++ map (\\e -> e {{ path = [\"c.txt\"] }}) {0} ++ map (\\e -> e {{ path = [\"keep.txt\"] }}) {0} \
         ++ [{{ path = [{1}], content = blob \"deep\" }}] }})",
        conflict,
        too_deep("zz")
    );
    let out = r.j(&[&edit]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("cannot check out"), "{}", out.stderr);
    assert!(out.stderr.contains("the operation is recorded"), "{}", out.stderr);
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    assert!(r.read("c.txt").contains("<<<<<<<<"), "{}", r.read("c.txt"));
    assert_eq!(r.read("c.txt"), r.read("keep.txt"));
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    let all = r.j(&[conflicted]).ok().stdout;
    assert_eq!(all.split_whitespace().collect::<Vec<_>>().join(" "), r#"[["a.txt"] ["c.txt"] ["keep.txt"]]"#);
    r.j(&["undo"]).ok();
    assert_eq!(r.j(&["log"]).ok().stdout, log);
    assert!(!r.dir.join("c.txt").exists());
    assert_eq!(r.read("keep.txt"), "k\n");
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 2);
}

#[test]
#[cfg(target_os = "linux")]
fn a_checkout_whose_state_fails_to_save_is_carried_on_from() {
    // §7.4, §7.5 step 7: a checkout that completed, but whose working-copy
    // state could not then be saved, left the state behind the commit, and
    // once any file was edited every run was refused as over a stale
    // working copy. The next run takes the directory for the focus's files
    // now, and records only the edit.
    let r = setup();
    r.write("keep.txt", "k\n");
    r.j(&["describe \"base\""]).ok();
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    let n = 2000;
    // a directory where the state is saved: saving it fails
    let state = r.dir.join(".jj/working_copy/tree_state");
    let aside = state.with_extension("aside");
    let out = stopped_mid_checkout(&r, &slow_edit("c.files", n), n, || {
        std::fs::rename(&state, &aside).unwrap();
        std::fs::create_dir(&state).unwrap();
        std::fs::write(state.join("x"), "").unwrap();
    });
    std::fs::remove_dir_all(&state).unwrap();
    std::fs::rename(&aside, &state).unwrap();
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("cannot save the working copy's state"), "{}", out.stderr);
    assert!(out.stderr.contains("the operation is recorded"), "{}", out.stderr);
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    assert_eq!(in_m(&r), n);
    assert_eq!(r.read("zz"), "z\n");
    r.write("keep.txt", "edited\n");
    r.j(&["log"]).ok();
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 2);
    let texts = r.j(&["\\r -> show (map (\\e -> text e.content) (restrict (neg (under ./m)) (files r)))"]).ok().stdout;
    assert_eq!(texts.trim(), r#"["edited\n" "z\n"]"#);
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 2);
}

/// Whether `dir`'s filesystem folds case, judged as the backend judges it:
/// `.JJ` names the workspace's `.jj`, which the directory does not list
/// under that spelling
fn folds_case(dir: &std::path::Path) -> bool {
    dir.join(".JJ").symlink_metadata().is_ok()
        && !std::fs::read_dir(dir).unwrap().any(|e| e.unwrap().file_name() == ".JJ")
}

#[test]
fn a_failed_case_only_rename_keeps_the_edits() {
    // §7.5 step 7: the put-back checked the tree back out in one go, in
    // path order. Where the filesystem folds case, a focus renaming
    // `README` to `readme` had removed `README` and written `readme` when
    // its checkout failed; the put-back wrote `README` first, which
    // `readme` was in the way of, then removed `readme`, so neither
    // spelling was left and the uncommitted edit was lost, as was a file
    // in a directory renamed only in case. Nothing is put back now: the
    // edits are in the focus, and on disk under one spelling or the other.
    // Run with TMPDIR on such a mount (exFAT through FUSE) to take that
    // case; elsewhere the two names are two files.
    let rename = |from: &str, to: &str| {
        format!("map (\\e -> if e.path == {} then e {{ path = {} }} else e)", from, to)
    };
    let edit = |first: &str| {
        format!(
            "mapRoot (\\c -> c {{ files = {} ({} c.files) ++ [{{ path = [{}], content = blob \"deep\" }}] }})",
            rename("[\"README\"]", "[\"readme\"]"),
            rename("[\"Docs\" \"a\"]", "[\"docs\" \"a\"]"),
            too_deep(first)
        )
    };
    let listed = |dir: &std::path::Path| {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with('.'))
            .collect();
        names.sort();
        names
    };
    // the checkout fails after the renames (at `zz`), or before them (at
    // `A`), when `readme` and `docs` name the untouched `README` and `Docs`
    // where case folds
    for (first, readme, docs) in [("zz", "readme", "docs"), ("A", "README", "Docs")] {
        let r = setup();
        r.write("README", "hello\n");
        std::fs::create_dir(r.dir.join("Docs")).unwrap();
        r.write("Docs/a", "a\n");
        r.write("notes", "n\n");
        r.j(&["describe \"base\""]).ok();
        r.write("README", "hello, my uncommitted edit\n");
        r.write("Docs/a", "a, my uncommitted edit\n");
        let ops = r.j(&["ops"]).ok().stdout.lines().count();
        let out = r.j(&[&edit(first)]);
        assert_eq!(out.code, 1, "{}", out.stderr);
        assert!(out.stderr.contains("cannot check out"), "{}", out.stderr);
        assert!(out.stderr.contains("the operation is recorded"), "{}", out.stderr);
        assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
        let mut names = vec![docs.to_string(), readme.to_string(), "notes".to_string(), first.to_string()];
        names.sort();
        assert_eq!(listed(&r.dir), names, "{}", first);
        assert_eq!(listed(&r.dir.join(docs)), ["a"], "{}", first);
        assert_eq!(r.read(readme), "hello, my uncommitted edit\n");
        assert_eq!(r.read(&format!("{}/a", docs)), "a, my uncommitted edit\n");
        // the next run records nothing: the edits are in the focus
        r.j(&["id"]).ok();
        assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
        let texts = r
            .j(&["\\r -> show [(contentAt [\"docs\" \"a\"] (files r)) (contentAt [\"readme\"] (files r)) (contentAt [\"notes\"] (files r))]"])
            .ok()
            .stdout;
        assert_eq!(
            texts.split_whitespace().collect::<Vec<_>>().join(" "),
            r#"[(blob "a, my uncommitted edit\n") (blob "hello, my uncommitted edit\n") (blob "n\n")]"#
        );
    }
}

#[test]
fn reserved_names_in_another_case_refused_where_the_filesystem_folds_case() {
    // on a case-folding filesystem (macOS, Windows, exFAT) `.GIT` is `.git`,
    // which jj's checkout refuses after the operation is recorded or, where
    // each spelling has its own inode number (exfat-fuse), writes into;
    // elsewhere it is an ordinary name the snapshot tracks. Run with TMPDIR
    // on such a mount to take the first branch.
    let r = setup();
    r.write("a.txt", "x\n");
    r.j(&["describe \"stable\""]).ok();
    let folds = folds_case(&r.dir);
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    let edit = r#"mapRoot (\c -> c { files = c.files ++ [{ path = ["sub" ".GIT" "x"], content = blob "x" }] })"#;
    let dry = r.j(&[&format!("tree . validate . {}", edit)]);
    let out = r.j(&[edit]);
    if folds {
        assert_eq!(dry.code, 1, "validate accepted it: {}", dry.stdout);
        assert_eq!(out.code, 1, "{}", out.stderr);
        assert!(out.stderr.contains("path component"), "{}", out.stderr);
        assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops);
    } else {
        assert_eq!(dry.code, 0, "{}", dry.stderr);
        assert_eq!(out.code, 0, "{}", out.stderr);
        assert_eq!(r.read("sub/.GIT/x"), "x");
    }
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
fn replay_making_a_file_and_a_directory_of_one_path_refused() {
    // rebasing a commit that adds `a/b` onto one with a file `a`: jj's merge
    // makes one conflict at `a` with the directory as a side, so the rebase
    // and its `validate` dry run exited 0 with `a/b` gone from `files` (and,
    // while such a side was stored as an empty file, from every tree). The
    // in-memory backend left both `a` and `a/b` instead, which its next
    // replay refused (§7.3).
    let clash = "would be a file on one side and a directory on another";
    let r = setup();
    r.write("a", "a\n");
    r.j(&["describe \"B\""]).ok();
    r.j(&["new . top"]).ok();
    std::fs::create_dir(r.dir.join("a")).unwrap();
    r.write("a/b", "nested\n");
    r.j(&["describe \"S\""]).ok();
    let log = r.j(&["log"]).ok().stdout;
    let out = r.j(&["tree . validate . rebase siblings"]);
    assert_eq!(out.code, 1, "validate accepted it: {}", out.stdout);
    assert!(out.stderr.contains(&format!("`a` {}", clash)), "{}", out.stderr);
    let out = r.j(&["rebase siblings"]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains(&format!("`a` {}", clash)), "{}", out.stderr);
    assert_eq!(r.j(&["log"]).ok().stdout, log);
    let paths = r.j(&["\\r -> show (map (.path) (files r))"]).ok().stdout;
    assert_eq!(paths.trim(), "[[\"a\" \"b\"]]");
    assert_eq!(r.read("a/b"), "nested\n");
    // the builtin, either way round, and catchable
    let file = "[{ path = ./a, content = blob \"x\" }]";
    let dir = "[{ path = ./a/b, content = blob \"y\" }]";
    for (onto, to) in [(file, dir), (dir, file)] {
        let e = format!("replay {} ({{ from = [], to = {} }})", onto, to);
        let out = r.j(&[&format!("show ({})", e)]);
        assert_eq!(out.code, 1, "{}", out.stdout);
        assert!(out.stderr.contains(clash), "{}", out.stderr);
        let out = r.j(&[&format!("(show ({})) or \"caught\"", e)]).ok();
        assert_eq!(out.stdout.trim(), "caught");
    }
    // jj merges `a` as one value, so the deletion of `a/z` onto a snapshot
    // that made the directory `a` a file is refused too, where the in-memory
    // merge goes path by path and gives the file (§7.3, §10)
    let e = "replay [{ path = ./a, content = blob \"f\" }] \
             ({ from = [({ path = ./a/y, content = blob \"y\" }) ({ path = ./a/z, content = blob \"z\" })], \
             to = [{ path = ./a/y, content = blob \"y\" }] })";
    let out = r.j(&[&format!("show ({})", e)]);
    assert_eq!(out.code, 1, "{}", out.stdout);
    assert!(out.stderr.contains(&format!("`a` {}", clash)), "{}", out.stderr);
}

#[test]
fn conflict_directory_sides_kept_whole() {
    // jj keeps a conflict between a file and a directory as one path whose
    // side is the whole directory. Such a side was read as an empty file and
    // written back as one: the directory was gone from the stored tree, and
    // rebasing the commit back where its sides cancel left an empty file `a`
    // in place of the user's `a/b` (§7.3).
    let conflicted = "\\r -> show (conflicted (files r))";
    let paths = "\\r -> show (map (.path) (files r))";
    let onto = |m: &str| format!("rebase (matching (\\c -> c.message == \"{}\") all)", m);

    // R replaces the file `a` with a directory, L deletes it: R onto L is a
    // conflict with R's directory as a side, and no side a file
    let r = setup();
    r.write("a", "base\n");
    r.j(&["describe \"P\""]).ok();
    r.j(&["new"]).ok();
    std::fs::remove_file(r.dir.join("a")).unwrap();
    r.j(&["describe \"L\""]).ok();
    r.j(&["new . goto parents"]).ok();
    r.write("a", "other\n");
    r.j(&["describe \"Q\""]).ok();
    r.j(&["new . goto parents"]).ok();
    std::fs::remove_file(r.dir.join("a")).unwrap();
    std::fs::create_dir(r.dir.join("a")).unwrap();
    r.write("a/b", "deep\n");
    r.j(&["describe \"R\""]).ok();
    r.j(&[&onto("L")]).ok();
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"a\"]]");
    // a rewrite writes the conflict back as it was read
    r.j(&["describe \"R again\""]).ok();
    // onto Q's file `a`: the directory stays a side of R's conflict, now
    // beside Q's file, and is not refused, as it was a side already (§7.3)
    r.j(&[&onto("Q")]).ok();
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"a\"]]");
    // R back onto P: the sides cancel, leaving R's own directory
    r.j(&[&onto("P")]).ok();
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[]");
    assert_eq!(r.j(&[paths]).ok().stdout.trim(), "[[\"a\" \"b\"]]");
    assert_eq!(r.read("a/b"), "deep\n");

    // both sides replace the directory `a` with a file: the base is the
    // directory, and it cancels against P's when R goes back
    let r = setup();
    r.write("base.txt", "base\n");
    std::fs::create_dir(r.dir.join("a")).unwrap();
    r.write("a/b", "deep\n");
    r.j(&["describe \"P\""]).ok();
    r.j(&["new"]).ok();
    std::fs::remove_dir_all(r.dir.join("a")).unwrap();
    r.write("a", "left\n");
    r.j(&["describe \"L\""]).ok();
    r.j(&["new . goto parents"]).ok();
    std::fs::remove_dir_all(r.dir.join("a")).unwrap();
    r.write("a", "right\n");
    r.j(&["describe \"R\""]).ok();
    r.j(&["rebase siblings"]).ok();
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"a\"]]");
    let back = "\\r -> show (conflicted (files (rebase (parents . prev) r)))";
    assert_eq!(r.j(&[back]).ok().stdout.trim(), "[]");
    r.j(&["rebase (parents . prev)"]).ok();
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[]");
    assert_eq!(r.j(&[paths]).ok().stdout.trim(), "[[\"a\"] [\"base.txt\"]]");
    assert_eq!(r.read("a"), "right\n");
}

#[test]
fn conflict_between_a_file_and_a_directory_resolved_in_a_child_squashes() {
    // resolving a conflict in a child and squashing it: contract replays the
    // child with `from` the conflicted parent, whose file side the merge
    // carries beside the child's directory. That mix came in with the
    // conflict, but replay refused it as one it made, so `squash`, its
    // dry run, `abandon . goto parents`, and the law
    // (abandon . contract m . split m) r = r all crashed (§7.3, §8)
    let conflicted = "\\r -> show (conflicted (files r))";
    let paths = "\\r -> show (map (.path) (files r))";
    let resolve_into_directory = |r: &Repo| {
        r.j(&["new"]).ok();
        let a = r.dir.join("a");
        std::fs::remove_file(&a).or_else(|_| std::fs::remove_dir_all(&a)).unwrap();
        std::fs::create_dir(&a).unwrap();
        r.write("a/b", "deep2\n");
        r.j(&["describe \"C\""]).ok();
    };
    let squashes = |r: &Repo| {
        for m in ["everything", "(under ./a)"] {
            let law = format!("\\r -> remove ((abandon . contract {m} . split {m}) r) == r");
            assert_eq!(r.j(&[&law]).ok().stdout.trim(), "true", "{}", m);
        }
        r.j(&["tree . validate . squash"]).ok();
        r.j(&["tree . validate . abandon . goto parents"]).ok();
        r.j(&["squash"]).ok();
        assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[]");
        assert_eq!(r.j(&[paths]).ok().stdout.trim(), "[[\"a\" \"b\"]]");
        let parent = "\\r -> show (conflicted (files (up r)))";
        assert_eq!(r.j(&[parent]).ok().stdout.trim(), "[]");
        assert_eq!(r.read("a/b"), "deep2\n");
    };
    let onto_l = "rebase (matching (\\c -> c.message == \"L\") all)";

    // jj's conflict between R's directory and L's deletion of P's file
    let r = setup();
    r.write("a", "base\n");
    r.j(&["describe \"P\""]).ok();
    r.j(&["new"]).ok();
    std::fs::remove_file(r.dir.join("a")).unwrap();
    r.j(&["describe \"L\""]).ok();
    r.j(&["new . goto parents"]).ok();
    std::fs::remove_file(r.dir.join("a")).unwrap();
    std::fs::create_dir(r.dir.join("a")).unwrap();
    r.write("a/b", "deep\n");
    r.j(&["describe \"R\""]).ok();
    r.j(&[onto_l]).ok();
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"a\"]]");
    resolve_into_directory(&r);
    squashes(&r);

    // a conflict between two files, resolved into a directory; S, beside L,
    // makes `a` a directory
    let r = setup();
    r.write("a", "base\n");
    r.j(&["describe \"P\""]).ok();
    r.j(&["new"]).ok();
    r.write("a", "left\n");
    r.j(&["describe \"L\""]).ok();
    r.j(&["new . goto parents"]).ok();
    std::fs::remove_file(r.dir.join("a")).unwrap();
    std::fs::create_dir(r.dir.join("a")).unwrap();
    r.write("a/x", "d\n");
    r.j(&["describe \"S\""]).ok();
    r.j(&["new . goto parents"]).ok();
    r.write("a", "right\n");
    r.j(&["describe \"R\""]).ok();
    r.j(&[onto_l]).ok();
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"a\"]]");
    // `a/b` added beside the conflict at `a` is carried as one of its sides,
    // where the in-memory merge refuses it (§7.3, §10)
    let e = "\\r -> show (conflicted (replay (files r) \
             ({ from = [], to = [{ path = ./a/b, content = blob \"b\" }] })))";
    assert_eq!(r.j(&[e]).ok().stdout.trim(), "[[\"a\"]]");
    // R onto S: S's directory is a side of R's conflict beside its file
    // `right`, and only `to` holds a conflict at `a`, so it is carried too
    let onto_s = "rebase (matching (\\c -> c.message == \"S\") all)";
    let e = format!("\\r -> show (conflicted (files ({} r)))", onto_s);
    assert_eq!(r.j(&[&e]).ok().stdout.trim(), "[[\"a\"]]");
    r.j(&[&format!("tree . validate . {}", onto_s)]).ok();
    resolve_into_directory(&r);
    squashes(&r);

    // a conflict below the path is none at it: the merge still makes the
    // clash between `onto`'s file and `to`'s directory, and is refused
    let r = setup();
    std::fs::create_dir(r.dir.join("a")).unwrap();
    r.write("a/b", "base\n");
    r.j(&["describe \"P\""]).ok();
    r.j(&["new"]).ok();
    r.write("a/b", "left\n");
    r.j(&["describe \"L\""]).ok();
    r.j(&["new . goto parents"]).ok();
    r.write("a/b", "right\n");
    r.j(&["describe \"R\""]).ok();
    r.j(&[onto_l]).ok();
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"a\" \"b\"]]");
    let e = "\\r -> show (replay [{ path = ./a, content = blob \"f\" }] \
             ({ from = files r, to = [{ path = ./a/b, content = blob \"r\" }] }))";
    let out = r.j(&[e]);
    assert_eq!(out.code, 1, "{}", out.stdout);
    assert!(
        out.stderr.contains("`a` would be a file on one side and a directory on another"),
        "{}",
        out.stderr
    );
}

#[test]
fn a_file_replacing_a_directory_with_a_conflict_inside_is_recorded() {
    // the mirror of the test above: a conflict inside the directory `a`,
    // resolved in a child by replacing `a` with a file. jj's scan keeps the
    // directory's sides at `a` where a file is found there, and drops only
    // the entries below it, so the file reached no tree: a debug build
    // failed an assertion on every run that scanned (exit 101, `undo`
    // included), and a release build recorded the child without `a` (§7.4)
    let conflicted = "\\r -> show (conflicted (files r))";
    let paths = "\\r -> show (map (.path) (files r))";
    let onto_l = "rebase (matching (\\c -> c.message == \"L\") all)";
    // R, rebased onto its sibling L, conflicted at `path`
    let conflict_at = |path: &str, beside: &[&str]| {
        let r = setup();
        let write = |content: &str| {
            std::fs::create_dir_all(r.dir.join(path).parent().unwrap()).unwrap();
            r.write(path, content);
        };
        write("base\n");
        for p in beside {
            r.write(p, "beside\n");
        }
        r.j(&["describe \"P\""]).ok();
        r.j(&["new"]).ok();
        write("left\n");
        r.j(&["describe \"L\""]).ok();
        r.j(&["new . goto parents"]).ok();
        write("right\n");
        r.j(&["describe \"R\""]).ok();
        r.j(&[onto_l]).ok();
        r.j(&["new"]).ok();
        r
    };

    let r = conflict_at("a/b", &[]);
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"a\" \"b\"]]");
    // a checkout that writes the file `a` over the conflicted directory,
    // and then fails, leaves the file there: the next run scans it over
    // that directory, which the working-copy state still records
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    let edit = format!(
        "\\r -> mapRoot (\\c -> c {{ files = [({{ path = ./a, content = blob \"f\" }}) \
         ({{ path = [{}], content = blob \"deep\" }})] }}) r",
        too_deep("d")
    );
    let out = r.j(&[&edit]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("cannot check out"), "{}", out.stderr);
    assert!(out.stderr.contains("the operation is recorded"), "{}", out.stderr);
    assert_eq!(r.read("a"), "f");
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    r.j(&["undo"]).ok();
    assert!(r.read("a/b").contains("<<<<<<<"), "{}", r.read("a/b"));
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"a\" \"b\"]]");
    std::fs::remove_dir_all(r.dir.join("a")).unwrap();
    r.write("a", "file\n");
    // `undo` scans to refuse a working directory not recorded
    let out = r.j(&["undo"]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("working copy has changes not in @"), "{}", out.stderr);
    assert_eq!(r.j(&[paths]).ok().stdout.trim(), "[[\"a\"]]");
    r.j(&["describe \"C\""]).ok();
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[]");
    let content = "\\r -> show (contentAt [\"a\"] (files r))";
    assert_eq!(r.j(&[content]).ok().stdout.trim(), "blob \"file\\n\"");
    // R keeps its conflict; the child resolves it
    let parent = "\\r -> show (conflicted (files (up r)))";
    assert_eq!(r.j(&[parent]).ok().stdout.trim(), "[[\"a\" \"b\"]]");
    for m in ["everything", "(under ./a)"] {
        let law = format!("\\r -> remove ((abandon . contract {m} . split {m}) r) == r");
        assert_eq!(r.j(&[&law]).ok().stdout.trim(), "true", "{}", m);
    }
    r.j(&["tree . validate . squash"]).ok();
    r.j(&["squash"]).ok();
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[]");
    assert_eq!(r.j(&[paths]).ok().stdout.trim(), "[[\"a\"]]");
    assert_eq!(r.j(&[parent]).ok().stdout.trim(), "[]");
    assert_eq!(r.read("a"), "file\n");

    // deeper down, beside a file the directory keeps
    let r = conflict_at("a/x/b", &["a/c"]);
    std::fs::remove_dir_all(r.dir.join("a/x")).unwrap();
    r.write("a/x", "file\n");
    r.j(&["id"]).ok();
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[]");
    assert_eq!(r.j(&[paths]).ok().stdout.trim(), "[[\"a\" \"c\"] [\"a\" \"x\"]]");
    assert_eq!(r.read("a/x"), "file\n");
}

#[cfg(unix)]
#[test]
fn a_file_written_over_a_conflict_with_a_side_that_is_no_file_is_recorded() {
    // a conflict at `a` with a side that is a directory or a symlink is
    // checked out as jj's description of its sides. A file written there
    // in its place reached no tree: jj's scan found no file merge to read
    // it into and kept the conflict, so the run showed the conflict,
    // `describe` recorded it, and a checkout wrote the description back
    // over the file, which no blob held (§7.4)
    let conflicted = "\\r -> show (conflicted (files r))";
    let content = "\\r -> show (contentAt [\"a\"] (files r))";
    let goto = |m: &str| format!("goto (matching (\\c -> c.message == \"{}\") all)", m);
    // R, rebased onto its sibling L, where each changes P's file `a`
    let conflict = |left: &dyn Fn(&Repo), right: &dyn Fn(&Repo)| {
        let r = setup();
        r.write("a", "base\n");
        r.j(&["describe \"P\""]).ok();
        r.j(&["new"]).ok();
        std::fs::remove_file(r.dir.join("a")).unwrap();
        left(&r);
        r.j(&["describe \"L\""]).ok();
        r.j(&["new . goto parents"]).ok();
        std::fs::remove_file(r.dir.join("a")).unwrap();
        right(&r);
        r.j(&["describe \"R\""]).ok();
        r.j(&["rebase (matching (\\c -> c.message == \"L\") all)"]).ok();
        r.j(&["new"]).ok();
        assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"a\"]]");
        assert!(r.read("a").starts_with("Conflict:\n"), "{}", r.read("a"));
        r
    };

    // R's directory against L's deletion
    let r = conflict(&|_| {}, &|r| {
        std::fs::create_dir(r.dir.join("a")).unwrap();
        r.write("a/b", "deep\n");
    });
    // the description, left as it is or written again, is the conflict
    let description = r.read("a");
    r.j(&["id"]).ok();
    r.write("a", &description);
    r.j(&["id"]).ok();
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"a\"]]");
    r.write("a", "resolved\n");
    // `undo` scans to refuse a working directory not recorded
    let out = r.j(&["undo"]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("working copy has changes not in @"), "{}", out.stderr);
    assert_eq!(r.j(&[content]).ok().stdout.trim(), "blob \"resolved\\n\"");
    r.j(&["describe \"C\""]).ok();
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[]");
    r.j(&[&goto("P")]).ok();
    assert_eq!(r.read("a"), "base\n");
    r.j(&[&goto("C")]).ok();
    assert_eq!(r.read("a"), "resolved\n");
    assert_eq!(r.j(&[content]).ok().stdout.trim(), "blob \"resolved\\n\"");
    // R keeps its conflict; the child resolves it
    let parent = "\\r -> show (conflicted (files (up r)))";
    assert_eq!(r.j(&[parent]).ok().stdout.trim(), "[[\"a\"]]");
    r.j(&["squash"]).ok();
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[]");
    assert_eq!(r.j(&[parent]).ok().stdout.trim(), "[]");
    assert_eq!(r.read("a"), "resolved\n");

    // L's symlink against R's file; `a` stays tracked where `.gitignore`
    // has come to match it, as a tracked file does
    let r = conflict(&|r| std::os::unix::fs::symlink("elsewhere", r.dir.join("a")).unwrap(), &|r| {
        r.write("a", "right\n")
    });
    r.write(".gitignore", "a\n");
    r.write("a", "resolved\n");
    r.j(&["id"]).ok();
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[]");
    assert_eq!(r.j(&[content]).ok().stdout.trim(), "blob \"resolved\\n\"");
    assert_eq!(r.read("a"), "resolved\n");
}

/// The repository `r` as jj-lib loads it at its head operation, for what a
/// test cannot make or see through `j`: a tree as jj itself writes one
fn jj_repo(r: &Repo) -> std::sync::Arc<jj_lib::repo::ReadonlyRepo> {
    use jj_lib::config::{ConfigLayer, ConfigSource, StackedConfig};
    use jj_lib::default_backend_factories::{default_backend_factories, default_working_copy_factories};
    let mut config = StackedConfig::with_defaults();
    let user = "[user]\nname = \"Test User\"\nemail = \"test@example.com\"\n";
    config.add_layer(ConfigLayer::parse(ConfigSource::User, user).unwrap());
    let settings = jj_lib::settings::UserSettings::from_config(config).unwrap();
    let ws = jj_lib::workspace::Workspace::load(
        &settings,
        &r.dir,
        &default_backend_factories(),
        &default_working_copy_factories(),
    )
    .unwrap();
    pollster::block_on(ws.repo_loader().load_at_head()).unwrap()
}

/// The stored commit that `id`, an expression of the repo `r` giving one
/// commit's `Id`, names
fn jj_commit(r: &Repo, repo: &std::sync::Arc<jj_lib::repo::ReadonlyRepo>, id: &str) -> jj_lib::commit::Commit {
    use jj_lib::repo::Repo as _;
    let hash = r.j(&[&format!("\\r -> (meta ({})).hash", id)]).ok().stdout;
    let id = jj_lib::backend::CommitId::try_from_hex(hash.trim()).unwrap();
    repo.store().get_commit(&id).unwrap()
}

/// Rebase the commit `id` names onto the one `onto` names, as jj does: it
/// merges their trees as trees, so where the commit is conflicted, a path
/// can keep each term of its merge, a pair of sides that cancel included
fn jj_rebase(r: &Repo, id: &str, onto: &str) {
    let repo = jj_repo(r);
    let commit = jj_commit(r, &repo, id);
    let onto = jj_commit(r, &repo, onto);
    let mut tx = repo.start_transaction();
    pollster::block_on(jj_lib::rewrite::rebase_commit(tx.repo_mut(), commit, vec![onto.id().clone()])).unwrap();
    pollster::block_on(tx.repo_mut().rebase_descendants()).unwrap();
    pollster::block_on(tx.commit("rebase")).unwrap();
}

#[cfg(unix)]
#[test]
fn a_conflict_with_a_side_that_is_no_file_left_as_written_stays_when_its_sides_cancel() {
    // a tree of five sides, as jj's rebase of a conflicted commit makes one,
    // with a file conflict at `a` and a symlink/file conflict at `b`, whose
    // description names L's symlink as removed once and added twice (`j`
    // reads the conflict with the pair dropped, but keeps the tree as jj
    // stored it). Resolving `a` lets the scan write a tree of three
    // sides, where `b`'s conflict has the pair cancelled and describes
    // itself in three lines, while the file, which nobody touched, still
    // holds the five a checkout wrote. The scan took that file for one
    // written over the conflict and recorded the description as `b`'s text:
    // the run showed `b` resolved, `undo` refused a directory nobody had
    // changed, and `describe` recorded the text in C (§7.4)
    let conflicted = "\\r -> show (conflicted (files r))";
    let parent = "\\r -> show (conflicted (files (up r)))";
    let content = "\\r -> show (contentAt [\"b\"] (files r))";
    let goto = |m: &str| format!("goto (matching (\\c -> c.message == \"{}\") all)", m);
    let r = setup();
    r.write("a", "base\n");
    r.write("b", "b0\n");
    r.j(&["describe \"P\""]).ok();
    r.j(&["new"]).ok();
    std::fs::remove_file(r.dir.join("b")).unwrap();
    std::os::unix::fs::symlink("elsewhere", r.dir.join("b")).unwrap();
    r.j(&["describe \"L\""]).ok();
    r.j(&["new . goto parents"]).ok();
    r.write("a", "right\n");
    r.write("b", "b2\n");
    r.j(&["describe \"R\""]).ok();
    r.j(&["rebase (matching (\\c -> c.message == \"L\") all)"]).ok();
    r.j(&[&goto("L")]).ok();
    r.j(&["new"]).ok();
    r.write("a", "other\n");
    r.j(&["describe \"M\""]).ok();
    // R onto M as jj rebases it, keeping the pair at `b` (replay drops it)
    jj_rebase(&r, "head (matching (\\c -> c.message == \"R\") all r)", "r.root.id");
    r.j(&[&goto("R")]).ok();
    r.j(&["new"]).ok();
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"a\"] [\"b\"]]");
    let repo = jj_repo(&r);
    let tree = |id: &str| jj_commit(&r, &repo, id).tree_ids().clone();
    assert_eq!(tree("r.root.id").iter().count(), 5);
    assert_eq!(tree("r.root.id"), tree("(up r).root.id"));
    let description = r.read("b");
    assert_eq!(description.lines().count(), 6, "{}", description);
    let modified = || std::fs::symlink_metadata(r.dir.join("b")).unwrap().modified().unwrap();
    let written = modified();

    r.write("a", "resolved\n");
    r.j(&["describe \"C\""]).ok();
    r.j(&["new"]).ok();
    assert_eq!(r.j(&[parent]).ok().stdout.trim(), "[[\"b\"]]");
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"b\"]]");
    assert_eq!(r.read("b"), description);
    assert_eq!(modified(), written);
    let out = r.j(&["undo"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    r.j(&["redo"]).ok();
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"b\"]]");
    assert_eq!(modified(), written);
    // the scan reads no file the working copy's state records as it was
    // written, as jj's reads none: one describing the conflict otherwise,
    // as another version of jj may have, stays the conflict
    let other = description.replace("Conflict:", "CONFLICT:");
    r.write("b", &other);
    let file = std::fs::File::options().write(true).open(r.dir.join("b")).unwrap();
    file.set_modified(written).unwrap();
    drop(file);
    r.j(&["id"]).ok();
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"b\"]]");
    assert_eq!(r.read("b"), other);
    // the description written again, as an editor saving it unchanged
    // does, is still the conflict
    r.write("b", &description);
    r.j(&["id"]).ok();
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"b\"]]");
    // and so is it where the working-copy state is lost, and made again
    // recording no file: nothing is recorded, and `undo` does not refuse
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    std::fs::remove_file(r.dir.join(".jj/working_copy/tree_state")).unwrap();
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops);
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"b\"]]");
    let out = r.j(&["undo"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    r.j(&["redo"]).ok();
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"b\"]]");
    // anything else written over it is recorded
    r.write("b", "mine\n");
    r.j(&["describe \"D\""]).ok();
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[]");
    assert_eq!(r.j(&[content]).ok().stdout.trim(), "blob \"mine\\n\"");
    assert_eq!(r.j(&[parent]).ok().stdout.trim(), "[[\"b\"]]");
}

#[test]
fn edit_applied_to_an_edit_is_a_contract_crash() {
    // `new new` gives `new` a function where its signature wants a Repo: a
    // contract crash (§4.13), so nothing is persisted (§1.2). It used to
    // compose instead, and persisted two new commits.
    let r = setup();
    let before = r.j(&["length . commits . top"]).ok().stdout;
    let out = r.j(&["new new"]);
    assert_eq!(out.code, 1, "stdout: {}", out.stdout);
    assert!(
        out.stderr.contains("contract: new expected Repo (record) as argument 1, got function"),
        "{}",
        out.stderr
    );
    let after = r.j(&["length . commits . top"]).ok().stdout;
    assert_eq!(before, after);
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
fn tree_draws_a_minted_commit_after_its_stored_sibling() {
    // a dry run's new commit has no stored time; it sorted as the oldest, so
    // it took its parent's lane and ended it, and the stored sibling forked
    // from under it, drawn as its child. Minted commits come last
    // (specs/tree.md Step 1), and the stored sibling forks from the parent.
    let r = setup();
    r.j(&["describe \"base\""]).ok();
    r.j(&["describe \"stored\" . new"]).ok();
    let out = r.j(&["tree . new . prev"]).ok().stdout;
    let rows: Vec<&str> = out.lines().collect();
    let stored = rows.iter().position(|l| l.ends_with("  stored")).unwrap();
    let minted = rows.iter().position(|l| l.starts_with('▶')).unwrap();
    assert!(stored < minted, "minted commit drawn first:\n{}", out);
    assert!(rows[stored].contains('├'), "{}", out);
    assert!(!rows[minted].contains('├'), "{}", out);
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
fn tree_data_sizes_commits_by_their_changes() {
    // the size bar counted the whole length of both versions of each changed
    // file, so a one-line edit to a long file drew `▇`, and the files column
    // counted the whole snapshot, so an empty commit showed its parent's file
    // count (§7.11 column 5; specs/tree.md Step 4 column 9)
    let r = setup();
    let big: String = (1..=1500).map(|i| format!("{}\n", i)).collect();
    r.write("big", &big);
    r.write("small", "hello\n");
    r.j(&["describe \"big file\""]).ok();
    r.j(&["new"]).ok();
    r.write("big", &big.replace("\n750\n", "\nseven fifty\n"));
    r.j(&["describe \"one line edit\""]).ok();
    r.j(&["new"]).ok();
    r.j(&["describe \"empty commit\""]).ok();
    let row = |out: &str, msg: &str| -> (String, String) {
        let row = out
            .lines()
            .find(|l| l.contains(&format!("  {}  ", msg)))
            .unwrap_or_else(|| panic!("no row for {:?} in:\n{}", msg, out))
            .to_string();
        let bar: String = row.chars().filter(|c| "▁▂▃▅▇".contains(*c)).collect();
        let toks: Vec<&str> = row.split_whitespace().collect();
        let at = toks.iter().position(|t| *t == "files").expect("files column");
        (bar, toks[at - 1].to_string())
    };
    let out = r.j(&["treeData"]).ok().stdout;
    assert_eq!(row(&out, "big file"), ("▇".into(), "2".into()), "{}", out);
    assert_eq!(row(&out, "one line edit"), ("▁".into(), "1".into()), "{}", out);
    assert_eq!(row(&out, "empty commit"), ("".into(), "0".into()), "{}", out);
    // at detail 1 the first commit is not next to the focus and draws no
    // bar; its files column is counted all the same
    let detail1 = "treeWith ({ detail = 1, margin = false, elide = false, icons = false, color = \"never\", lanes = 4, author = false, date = false, files = true })";
    let out = r.j(&[detail1]).ok().stdout;
    assert_eq!(row(&out, "big file"), ("".into(), "2".into()), "{}", out);
    assert_eq!(row(&out, "one line edit"), ("▁".into(), "1".into()), "{}", out);
    assert_eq!(row(&out, "empty commit"), ("".into(), "0".into()), "{}", out);
}

#[test]
fn focus_lists_the_paths_the_commit_changes() {
    // `j focus` listed the whole snapshot, counted and unmarked, so a deleted
    // file never showed, and any commit shown as a block was drawn as the
    // focus (§5.1)
    let r = setup();
    r.write("a.txt", "a\n");
    r.write("b.txt", "b\n");
    r.j(&["describe \"A\""]).ok();
    r.j(&["new"]).ok();
    r.write("a.txt", "a2\n");
    std::fs::remove_file(r.dir.join("b.txt")).unwrap();
    r.write("c.txt", "c\n");
    r.j(&["describe \"B\""]).ok();
    let out = r.j(&["focus"]).ok().stdout;
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].starts_with("◉ ") && lines[0].ends_with("  B"), "{}", out);
    assert!(lines[1].ends_with(" · 3 files"), "{}", out);
    assert_eq!(lines[2..], ["", "  ~ a.txt", "  − b.txt", "  + c.txt"], "{}", out);
    // the parent is drawn as `tree` draws it
    let out = r.j(&["\\r -> (up r).root"]).ok().stdout;
    let tree = r.j(&["tree"]).ok().stdout;
    let lines: Vec<&str> = out.lines().collect();
    assert!(tree_row(&tree, "A").contains('●'), "{}", tree);
    assert!(lines[0].starts_with("● ") && lines[0].ends_with("  A"), "{}", out);
    assert!(lines[1].ends_with(" · 2 files"), "{}", out);
    assert_eq!(lines[2..], ["", "  + a.txt", "  + b.txt"], "{}", out);
    // a new commit changes nothing
    r.j(&["new"]).ok();
    let out = r.j(&["focus"]).ok().stdout;
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].starts_with("◌ "), "{}", out);
    assert!(lines[1].ends_with(" · 0 files"), "{}", out);
    assert_eq!(lines.len(), 2, "{}", out);
}

#[test]
fn a_commit_is_shown_where_it_stands_after_the_snapshot() {
    // a commit's block read its parent from the repository as loaded, before
    // the snapshot, while the commit came from the repository the expression
    // was given, where the snapshot has rebased every descendant of the
    // focus: a dirty focus's edits showed as its child's own changes, and an
    // empty grandchild was drawn `○` with a path it does not change (§5.1)
    let r = setup();
    r.write("a.txt", "one\n");
    r.j(&["describe \"A\""]).ok();
    r.j(&["new"]).ok();
    r.write("c.txt", "c\n");
    r.j(&["describe \"C\""]).ok();
    r.j(&["new"]).ok();
    r.j(&["describe \"E\""]).ok();
    r.j(&["prev"]).ok();
    r.j(&["prev"]).ok();
    r.write("a.txt", "one\ntwo\n");
    let out = r.j(&["focus . next"]).ok().stdout;
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].starts_with("○ ") && lines[0].ends_with("  C"), "{}", out);
    assert!(lines[1].ends_with(" · 1 files"), "{}", out);
    assert_eq!(lines[2..], ["", "  + c.txt"], "{}", out);
    let tree = r.j(&["tree"]).ok().stdout;
    assert!(tree_row(&tree, "E").contains('◌'), "{}", tree);
    let out = r.j(&["focus . next . next"]).ok().stdout;
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].starts_with("◌ ") && lines[0].ends_with("  E"), "{}", out);
    assert!(lines[1].ends_with(" · 0 files"), "{}", out);
    assert_eq!(lines.len(), 2, "{}", out);
    // a commit table draws each row as the commit's line, glyph included
    r.j(&["new"]).ok();
    r.write("d.txt", "d\n");
    r.j(&["describe \"N\""]).ok();
    let tree = r.j(&["tree"]).ok().stdout;
    let out = r.j(&["ancestors"]).ok().stdout;
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "{}", out);
    assert!(tree_row(&tree, "N").contains('◉'), "{}", tree);
    assert!(lines[0].starts_with("◉ ") && lines[0].ends_with("  N"), "{}", out);
    assert!(tree_row(&tree, "A").contains('●'), "{}", tree);
    assert!(lines[1].starts_with("● ") && lines[1].ends_with("  A"), "{}", out);
    assert!(lines[2].starts_with("⌂ "), "{}", out);
}

#[test]
fn a_commit_table_reads_its_commits_after_the_snapshot() {
    // a list of ids looked its commits up in the repository as loaded, before
    // the snapshot rebased the focus's descendants: a child that the working
    // directory's edit puts in conflict was drawn `○` where `tree` and the
    // child's own line draw `⊗` (§5.1)
    let r = setup();
    r.write("a.txt", "one\n");
    r.j(&["describe \"A\""]).ok();
    r.j(&["new"]).ok();
    r.write("a.txt", "C\n");
    r.j(&["describe \"C\""]).ok();
    r.j(&["prev"]).ok();
    r.write("a.txt", "zero\n");
    let tree = r.j(&["tree"]).ok().stdout;
    assert!(tree_row(&tree, "C").contains('⊗'), "{}", tree);
    let out = r.j(&["\\r -> [(next r).root.id]"]).ok().stdout;
    assert!(out.starts_with("⊗ ") && out.ends_with("  C\n"), "{}", out);
    let out = r.j(&["\\r -> { c = (next r).root }"]).ok().stdout;
    assert!(out.starts_with("c  ⊗ ") && out.ends_with("  C\n"), "{}", out);
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
fn editing_a_conflict_keeps_its_long_markers() {
    // a conflict whose sides hold a 7-dash line (a markdown underline) is
    // written with longer markers, and the working copy remembers their
    // length. Checking out after a snapshot reset the state of every path
    // the snapshot changed, forgetting it, and when the checkout did not
    // rewrite the file the next run read it back with 7-character markers:
    // the side's own `-------` broke the parse, so a conflict edited with its
    // markers kept was recorded as resolved, markers and all (§7.3, §7.4)
    let r = setup();
    r.write("README.md", "Intro\n");
    r.j(&["describe \"base\""]).ok();
    r.j(&["new"]).ok();
    r.write("README.md", "Intro\n\nUsage\n-------\nA text\n");
    r.j(&["describe \"A\""]).ok();
    r.j(&["prev"]).ok();
    r.j(&["new"]).ok();
    r.write("README.md", "Intro\n\nInstall\n-------\nB text\n");
    r.j(&["describe \"B\""]).ok();
    r.j(&["rebase (matching (\\c -> c.message == \"A\") all)"]).ok();
    let text = r.read("README.md");
    assert!(text.contains("\n<<<<<<<<<<< "), "not written with long markers:\n{}", text);
    let conflicted = "\\r -> show (conflicted r.root.files)";
    // a persisting run that leaves the edited file as the snapshot read it
    r.write("README.md", &text.replace("B text\n", "B text, edited\n"));
    r.j(&["describe \"B edited\""]).ok();
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"README.md\"]]");
    r.j(&["id"]).ok();
    let content = "\\r -> show (contentAt [\"README.md\"] r.root.files)";
    let stored = r.j(&[content]).ok().stdout;
    assert!(stored.contains("unresolved") && stored.contains("B text, edited"), "{}", stored);
    // one that moves the focus to a child with the same files
    r.write("README.md", &text.replace("B text\n", "B text, edited twice\n"));
    r.j(&["new"]).ok();
    let status = r.j(&["\\r -> show [(status r).changed (status r).conflicts]"]).ok().stdout;
    assert_eq!(status.trim(), "[[] [[\"README.md\"]]]");
    assert!(r.read("README.md").contains("B text, edited twice\n"));
}

#[test]
fn a_directory_that_cannot_be_scanned_mid_run_does_not_fail_the_checkout() {
    // §7.4, §7.7: persist's checkout scans the working directory again. An
    // entry that appeared during evaluation and cannot be read (a directory
    // deeper than PATH_MAX here; an unreadable mount, a temporary file that
    // vanishes mid-scan) failed that scan after the operation had been
    // published: the run crashed with it recorded and the working copy never
    // advanced, and once the directory was gone the next run snapshotted the
    // child's files into the new focus, its parent. The checkout now
    // describes the snapshot instead, as it did before it rescanned.
    let r = setup();
    r.write("a", "p\n");
    r.j(&["describe \"P\""]).ok();
    r.j(&["new"]).ok();
    r.write("a", "x1\n");
    r.j(&["describe \"X\""]).ok();
    r.write("a", "x2\n");
    // built outside the working directory: 20 levels of 250 bytes
    let half: PathBuf = (0..10).map(|_| "d".repeat(250)).collect();
    let deep = r.cfg.join("deep");
    std::fs::create_dir_all(deep.join(&half)).unwrap();
    std::fs::create_dir_all(r.cfg.join("more").join(&half)).unwrap();
    std::fs::rename(r.cfg.join("more"), deep.join(&half).join("more")).unwrap();
    let ops_dir = r.dir.join(".jj/repo/op_store/operations");
    let count = || std::fs::read_dir(&ops_dir).unwrap().count();
    let before = count();
    let mut child = Command::new(j_bin())
        .arg("\\r -> prev (describe (show (foldl (+) 0 (range 0 1000000))) r)")
        .current_dir(&r.dir)
        .env("XDG_CONFIG_HOME", &r.cfg)
        .env("NO_COLOR", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // the snapshot's operation is written before evaluation: move the
    // directory in while the expression runs
    while count() == before && child.try_wait().unwrap().is_none() {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    std::fs::rename(&deep, r.dir.join("deep")).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(r.read("a"), "p\n");
    std::fs::remove_dir_all(r.dir.join("deep")).unwrap();
    let files = r.j(&[CONTENT_OF_A]).ok().stdout;
    assert_eq!(
        files.trim(),
        r#"[["" (blob "")] ["P" (blob "p\n")] ["499999500000" (blob "x2\n")]]"#
    );
}

/// A repository whose working copy is stale, as jj leaves it when it moves
/// the working-copy commit without updating the working directory (a
/// command run with `--ignore-working-copy`, or in another workspace): the
/// focus is B, while the directory and the working-copy state hold its
/// parent A. `a` is 1 in A and 2 in B, `b` is only in A, `z` only in B.
fn stale_working_copy() -> Repo {
    let r = setup();
    r.write("a", "1\n");
    r.write("b", "b\n");
    r.j(&["describe \"A\""]).ok();
    let saved: Vec<(PathBuf, Vec<u8>)> = std::fs::read_dir(r.dir.join(".jj/working_copy"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .map(|p| (p.clone(), std::fs::read(p).unwrap()))
        .collect();
    r.j(&["new"]).ok();
    r.write("a", "2\n");
    std::fs::remove_file(r.dir.join("b")).unwrap();
    r.write("z", "z\n");
    r.j(&["describe \"B\""]).ok();
    for (path, bytes) in saved {
        std::fs::write(path, bytes).unwrap();
    }
    r.write("a", "1\n");
    r.write("b", "b\n");
    std::fs::remove_file(r.dir.join("z")).unwrap();
    r
}

/// Each commit's message and paths, top-down
const PATHS_BY_COMMIT: &str =
    "\\r -> show (map (\\c -> [c.message (map (\\e -> e.path) c.files)]) (commits (top r)))";

#[test]
fn a_run_over_a_stale_working_copy_writes_the_focus() {
    // §7.4: with nothing snapshotted, persist's checkout took the directory
    // to hold the focus as loaded. Its rescan found otherwise, so it reset
    // the state to B, whose tree the focus shares: the checkout wrote
    // nothing, and the state was saved claiming B's files over A's. The
    // next run recorded A's files into the focus, a silent revert of B.
    let r = stale_working_copy();
    r.j(&["describe \"B2\""]).ok();
    assert_eq!(r.read("a"), "2\n");
    assert!(!r.dir.join("b").exists());
    assert_eq!(r.read("z"), "z\n");
    // the directory now holds the focus, so there is nothing to record
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops);
    let files = r.j(&[CONTENT_OF_A]).ok().stdout;
    assert_eq!(files.trim(), r#"[["" (blob "")] ["A" (blob "1\n")] ["B2" (blob "2\n")]]"#);
    let paths = r.j(&[PATHS_BY_COMMIT]).ok().stdout;
    assert_eq!(paths.trim(), r#"[["" []] ["A" [["a"] ["b"]]] ["B2" [["a"] ["z"]]]]"#);
}

#[test]
fn a_failed_checkout_over_a_stale_working_copy_is_carried_on_from() {
    // §7.4, §7.5 step 7: a checkout that fails over a stale working copy
    // leaves it stale, recording what the directory held when the
    // checkout began, so the next run takes what the checkout wrote for
    // the focus's own; `j undo` goes back, and checks out B, which the
    // directory did not hold before
    let r = stale_working_copy();
    let log = r.j(&["log"]).ok().stdout;
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    // written in path order: `a`, `b` (removed), then the path under `m`
    // fails before `z` is reached
    let edit = format!(
        "mapRoot (\\c -> c {{ files = c.files ++ [{{ path = [{}], content = blob \"deep\" }}] }})",
        too_deep("m")
    );
    let out = r.j(&[&edit]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("cannot check out"), "{}", out.stderr);
    assert!(out.stderr.contains("the operation is recorded"), "{}", out.stderr);
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    assert_eq!(r.read("a"), "2\n");
    assert!(!r.dir.join("b").exists());
    assert!(!r.dir.join("z").exists());
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    let files = r.j(&[CONTENT_OF_A]).ok().stdout;
    assert_eq!(files.trim(), r#"[["" (blob "")] ["A" (blob "1\n")] ["B" (blob "2\n")]]"#);
    let z = r.j(&["\\r -> show (contentAt [\"z\"] (files r))"]).ok().stdout;
    assert_eq!(z.trim(), r#"blob "z\n""#);
    r.j(&["undo"]).ok();
    assert_eq!(r.j(&["log"]).ok().stdout, log);
    assert_eq!(r.read("a"), "2\n");
    assert!(!r.dir.join("b").exists());
    assert_eq!(r.read("z"), "z\n");
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 2);
}

#[test]
fn a_stale_working_copy_with_changes_records_them_onto_the_focus() {
    // §7.4: the snapshot read the directory against the stale state and
    // recorded all of it into the focus, so one new file beside A's files
    // undid B; then every run, printing or not, was refused until the
    // directory was put back by hand. What the directory holds is a change
    // to what the state records, and only that change is recorded, onto
    // the focus.
    let r = stale_working_copy();
    r.write("notes.txt", "note\n");
    let paths = r.j(&["\\r -> show (map (\\e -> e.path) (files r))"]).ok().stdout;
    assert_eq!(paths.trim(), r#"[["a"] ["notes.txt"] ["z"]]"#);
    r.j(&["id"]).ok();
    let paths = r.j(&[PATHS_BY_COMMIT]).ok().stdout;
    assert_eq!(paths.trim(), r#"[["" []] ["A" [["a"] ["b"]]] ["B" [["a"] ["notes.txt"] ["z"]]]]"#);
    let files = r.j(&[CONTENT_OF_A]).ok().stdout;
    assert_eq!(files.trim(), r#"[["" (blob "")] ["A" (blob "1\n")] ["B" (blob "2\n")]]"#);
    // and the focus is checked out
    assert_eq!(r.read("a"), "2\n");
    assert!(!r.dir.join("b").exists());
    assert_eq!(r.read("z"), "z\n");
    assert_eq!(r.read("notes.txt"), "note\n");
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops);
}

#[test]
fn an_edit_over_a_stale_working_copy_to_a_file_the_focus_changed_is_a_conflict() {
    // §7.4: B changed `a` since the state was checked out, and so did the
    // user, differently: neither change is lost
    let r = stale_working_copy();
    r.write("a", "mine\n");
    r.j(&["id"]).ok();
    let conflicted = r.j(&["\\r -> show (conflicted (files r))"]).ok().stdout;
    assert_eq!(conflicted.trim(), r#"[["a"]]"#);
    let text = r.read("a");
    assert!(text.contains("mine\n") && text.contains("2\n"), "{}", text);
}

#[test]
fn a_directory_swapped_with_a_file_after_a_failed_checkout_is_kept() {
    // §7.4: over a stale working copy, the snapshot merged what the
    // directory changed with what the commit changed. Where the user had
    // put a directory in place of a file the commit changes, or a file in
    // place of a directory holding one, that merge gave one conflict at the
    // path between files and a directory, whose directory side j reads as
    // empty, and the next checkout wrote jj's description of it there,
    // deleting the files the directory held. The snapshot now takes what
    // the directory holds at such a path, as over a working copy that is
    // not stale; the commit's change there is left in the operation before.
    for (before, after) in [("z", "z/new"), ("z/x", "z")] {
        let r = setup();
        std::fs::create_dir_all(r.dir.join(before).parent().unwrap()).unwrap();
        r.write(before, "1\n");
        r.j(&["describe \"A\""]).ok();
        // the checkout fails at the path under `m`, before it reaches `z`
        let path: Vec<String> = before.split('/').map(|c| format!("\"{}\"", c)).collect();
        let edit = format!(
            "mapRoot (\\c -> c {{ files = [({{ path = [{}], content = blob \"2\\n\" }}) ({{ path = [{}], content = blob \"deep\" }})] }})",
            path.join(" "),
            too_deep("m")
        );
        let out = r.j(&[&edit]);
        assert_eq!(out.code, 1, "{}: {}", before, out.stderr);
        assert!(out.stderr.contains("the operation is recorded"), "{}: {}", before, out.stderr);
        assert_eq!(r.read(before), "1\n");
        let ops = r.j(&["ops"]).ok().stdout.lines().count();
        if before == "z" {
            std::fs::remove_file(r.dir.join("z")).unwrap();
        } else {
            std::fs::remove_dir_all(r.dir.join("z")).unwrap();
        }
        std::fs::create_dir_all(r.dir.join(after).parent().unwrap()).unwrap();
        r.write(after, "mine\n");
        // the path the checkout failed on is left out of the focus
        r.j(&["mapRoot (\\c -> c { files = restrict (neg (under ./m)) c.files })"]).ok();
        assert_eq!(r.read(after), "mine\n", "{}", before);
        let path: Vec<String> = after.split('/').map(|c| format!("\"{}\"", c)).collect();
        let paths = r.j(&[FOCUS_PATHS]).ok().stdout;
        assert_eq!(paths.trim(), format!("[[{}]]", path.join(" ")), "{}", before);
        let conflicted = r.j(&["\\r -> show (conflicted (files r))"]).ok().stdout;
        assert_eq!(conflicted.trim(), "[]", "{}", before);
        assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
        r.j(&["id"]).ok();
        assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1, "{}", before);
        assert_eq!(r.read(after), "mine\n", "{}", before);
    }
}

#[test]
fn a_conflict_with_a_directory_side_is_kept_where_a_failed_checkout_left_the_path() {
    // §7.4, §7.7: R's `z` is a conflict between L's deletion of a file and
    // R's directory in its place. A checkout from O, a child with its own
    // file `z`, back to R wrote `a` and then failed before it reached `z`.
    // The snapshot took what the directory held at every conflict of its
    // merge that set a directory against a file, R's own included, although
    // the directory held what the last checkout left at `z`: `j undo` was
    // refused as holding changes not in @, and the next persisting run
    // recorded O's file into R, its conflict and R's `z/x` gone. Only a
    // path the directory changed takes what it holds there.
    let conflicted = "\\r -> show (conflicted (files r))";
    let goto = |m: &str| format!("goto (matching (\\c -> c.message == \"{}\") all)", m);
    for undo in [true, false] {
        let r = setup();
        r.write("z", "base\n");
        r.j(&["describe \"P\""]).ok();
        r.j(&["new"]).ok();
        std::fs::remove_file(r.dir.join("z")).unwrap();
        r.j(&["describe \"L\""]).ok();
        r.j(&["new . goto parents"]).ok();
        std::fs::remove_file(r.dir.join("z")).unwrap();
        std::fs::create_dir(r.dir.join("z")).unwrap();
        r.write("z/x", "x\n");
        r.j(&["describe \"R\""]).ok();
        r.j(&["rebase (matching (\\c -> c.message == \"L\") all)"]).ok();
        assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"z\"]]");
        assert!(r.read("z").starts_with("Conflict:\n"), "{}", r.read("z"));
        r.j(&["new"]).ok();
        r.write("z", "other\n");
        r.j(&["describe \"O\""]).ok();
        let log = r.j(&["log"]).ok().stdout;
        let ops = r.j(&["ops"]).ok().stdout.lines().count();
        // written in path order: `a`, then the path under `m` fails, before
        // `z` is reached
        let edit = format!(
            "mapRoot (\\c -> c {{ files = c.files ++ [({{ path = [\"a\"], content = blob \"a\\n\" }}) ({{ path = [{}], content = blob \"deep\" }})] }}) . {}",
            too_deep("m"),
            goto("R")
        );
        let out = r.j(&[&edit]);
        assert_eq!(out.code, 1, "{}", out.stderr);
        assert!(out.stderr.contains("the operation is recorded"), "{}", out.stderr);
        assert_eq!(r.read("a"), "a\n");
        assert_eq!(r.read("z"), "other\n");
        assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"z\"]]");
        if undo {
            r.j(&["undo"]).ok();
            assert_eq!(r.j(&["log"]).ok().stdout, log);
            assert!(!r.dir.join("a").exists());
            assert_eq!(r.read("z"), "other\n");
            r.j(&["id"]).ok();
            assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 2);
            continue;
        }
        // the path the checkout failed on is left out of the focus
        r.j(&["mapRoot (\\c -> c { files = restrict (neg (under ./m)) c.files })"]).ok();
        assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"z\"]]");
        assert_eq!(r.j(&[FOCUS_PATHS]).ok().stdout.trim(), "[[\"a\"] [\"z\"]]");
        let description = r.read("z");
        assert!(description.starts_with("Conflict:\n") && description.contains("tree"), "{}", description);
        r.j(&["id"]).ok();
        assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 2);
        // the child still holds its own file
        r.j(&[&goto("O")]).ok();
        assert_eq!(r.read("z"), "other\n");
    }
}

#[test]
fn a_stale_working_copy_without_changes_is_undone_and_redone() {
    // §7.4, §7.7: `undo` and `redo` refuse only a directory holding changes
    // no commit has, and a directory holding what the stale state records
    // holds none
    let r = stale_working_copy();
    let log = r.j(&["log"]).ok().stdout;
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    r.j(&["undo"]).ok();
    r.j(&["redo"]).ok();
    assert_eq!(r.j(&["log"]).ok().stdout, log);
    assert_eq!(r.read("a"), "2\n");
    assert!(!r.dir.join("b").exists());
    assert_eq!(r.read("z"), "z\n");
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 2);
}

#[test]
fn a_stale_working_copy_holding_the_focus_records_nothing() {
    // §7.4: a directory that holds the focus's files over a stale state
    // has nothing to record and is not refused; the state is brought up to
    // date, so a change made from there is recorded as usual
    let r = stale_working_copy();
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    r.write("a", "2\n");
    std::fs::remove_file(r.dir.join("b")).unwrap();
    r.write("z", "z\n");
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops);
    r.write("notes.txt", "note\n");
    r.j(&["id"]).ok();
    let paths = r.j(&[PATHS_BY_COMMIT]).ok().stdout;
    assert_eq!(paths.trim(), r#"[["" []] ["A" [["a"] ["b"]]] ["B" [["a"] ["notes.txt"] ["z"]]]]"#);
}

#[test]
fn a_lost_working_copy_state_records_the_directory_as_it_is() {
    // §7.4: jj makes a new working-copy state, recording no file, where it
    // finds none, at the operation the working copy names: the head's. It
    // was taken for a stale one, as it records other files than the
    // commit, and the snapshot merged the directory onto the commit with
    // no file as the base: each file edited since the last checkout became
    // a conflict of the edit with the commit's content, and each one
    // deleted came back. A state saved at the head operation is current,
    // and the directory is recorded as it is, as jj records it.
    let r = setup();
    r.write("f", "1\n");
    r.write("h", "gone\n");
    r.j(&["describe \"A\""]).ok();
    r.j(&["new"]).ok();
    r.write("f", "2\n");
    std::fs::remove_file(r.dir.join("h")).unwrap();
    std::fs::remove_file(r.dir.join(".jj/working_copy/tree_state")).unwrap();
    let conflicted = "\\r -> show (conflicted (files r))";
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[]");
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
    assert_eq!(r.read("f"), "2\n");
    assert!(!r.dir.join("h").exists());
    let paths = r.j(&[PATHS_BY_COMMIT]).ok().stdout;
    assert_eq!(paths.trim(), r#"[["" []] ["A" [["f"] ["h"]]] ["" [["f"]]]]"#);
    let f = r.j(&["\\r -> show (contentAt [\"f\"] (files r))"]).ok().stdout;
    assert_eq!(f.trim(), r#"blob "2\n""#);
    // and the state is saved again, with nothing left to record
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops + 1);
}

#[test]
fn a_conflict_left_as_written_stays_when_the_working_copy_state_is_lost() {
    // §7.4: a working-copy state made again where it was lost records no
    // conflict, so the scan reads each file the checkout wrote for one as
    // the text of its markers. A file holding exactly what a checkout
    // writes for one of the commit's conflicts is read back as that
    // conflict: a directory nobody touched has nothing to record, and
    // `undo` and `redo` do not refuse it.
    let r = setup();
    r.write("a.txt", "base\n");
    r.write("keep.txt", "k\n");
    r.j(&["describe \"base\""]).ok();
    r.j(&["new"]).ok();
    r.write("a.txt", "<<<<<<< left\n");
    r.j(&["describe \"left\""]).ok();
    r.j(&["new . goto parents"]).ok();
    r.write("a.txt", "right\n");
    r.j(&["describe \"right\""]).ok();
    r.j(&["rebase siblings"]).ok();
    let conflicted = "\\r -> show (conflicted (files r))";
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"a.txt\"]]");
    let written = r.read("a.txt");
    let log = r.j(&["log"]).ok().stdout;
    let ops = r.j(&["ops"]).ok().stdout.lines().count();
    std::fs::remove_file(r.dir.join(".jj/working_copy/tree_state")).unwrap();
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"a.txt\"]]");
    r.j(&["id"]).ok();
    assert_eq!(r.j(&["ops"]).ok().stdout.lines().count(), ops);
    r.j(&["undo"]).ok();
    r.j(&["redo"]).ok();
    assert_eq!(r.j(&["log"]).ok().stdout, log);
    assert_eq!(r.read("a.txt"), written);
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"a.txt\"]]");
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

#[test]
fn a_replayed_conflict_keeps_no_sides_that_cancel() {
    // replay kept every term of the merge of a conflicted input, sides that
    // cancel included: rebasing a stack in which each commit conflicts with
    // the new base gave the second its parent's conflict with a pair of
    // sides added that cancel, so it listed its parent's path as changed,
    // its inherited conflict was not its parent's, and each commit up the
    // stack stored two sides more than the one below it (§7.3)
    let named = |m: &str| format!("(matching (\\c -> c.message == \"{}\") all)", m);
    let r = setup();
    r.write("a", "a1\na2\na3\n");
    r.write("b", "b1\nb2\nb3\n");
    r.j(&["describe \"trunk\""]).ok();
    r.j(&["new"]).ok();
    r.write("a", "a1\nA2 one\na3\n");
    r.j(&["describe \"one\""]).ok();
    r.j(&["new"]).ok();
    r.write("b", "b1\nB2 two\nb3\n");
    r.j(&["describe \"two\""]).ok();
    r.j(&[&format!("new . goto {}", named("trunk"))]).ok();
    r.write("a", "a1\nA2 trunk\na3\n");
    r.write("b", "b1\nB2 trunk\nb3\n");
    r.j(&["describe \"trunk 2\""]).ok();
    r.j(&[&format!("goto {}", named("one"))]).ok();
    r.j(&[&format!("rebase {}", named("trunk 2"))]).ok();
    r.j(&[&format!("goto {}", named("two"))]).ok();
    let conflicted = "\\r -> show (conflicted (files r))";
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"a\"] [\"b\"]]");
    // two changed `b` only; its `a` is one's conflict, of three terms
    assert_eq!(r.j(&["\\r -> show (changed r)"]).ok().stdout.trim(), "[[\"b\"]]");
    let inherited = "\\r -> contentAt [\"a\"] (files r) == contentAt [\"a\"] (files (up r))";
    assert_eq!(r.j(&[inherited]).ok().stdout.trim(), "true");
    let text = r.j(&["\\r -> text (contentAt [\"a\"] (files r))"]).ok().stdout;
    assert_eq!(text.matches("%%%%%%%").count(), 1, "{}", text);
    // and each is stored as a merge of three trees
    let repo = jj_repo(&r);
    for m in ["one", "two"] {
        let commit = jj_commit(&r, &repo, &format!("head ({} r)", named(m)));
        assert_eq!(commit.tree_ids().iter().count(), 3, "{}", m);
    }
}

#[test]
fn a_conflict_jj_merged_reads_without_the_sides_that_cancel_and_keeps_its_tree() {
    // jj's rebase keeps each term of a merge it cannot resolve, so in a
    // stack jj rebased, where each commit conflicts with the new base, the
    // second commit stores its parent's conflict with a pair of sides that
    // cancel added. `j` read it so, as a change the commit did not make.
    // Read without the pair, the conflict is the parent's; and the tree is
    // written as jj stored it wherever the files are still its, so `new`
    // leaves an empty child, as jj's `new` does, not one of another tree
    // for the same files (§7.3, §7.5)
    let named = |m: &str| format!("(matching (\\c -> c.message == \"{}\") all)", m);
    let r = setup();
    r.write("a", "a1\na2\na3\n");
    r.write("b", "b1\nb2\nb3\n");
    r.j(&["describe \"trunk\""]).ok();
    r.j(&["new"]).ok();
    r.write("a", "a1\nA2 one\na3\n");
    r.j(&["describe \"one\""]).ok();
    r.j(&["new"]).ok();
    r.write("b", "b1\nB2 two\nb3\n");
    r.j(&["describe \"two\""]).ok();
    r.j(&[&format!("new . goto {}", named("trunk"))]).ok();
    r.write("a", "a1\nA2 trunk\na3\n");
    r.write("b", "b1\nB2 trunk\nb3\n");
    r.j(&["describe \"trunk 2\""]).ok();
    // one onto trunk 2 as jj rebases it, and two with it
    jj_rebase(&r, &format!("head ({} r)", named("one")), "r.root.id");
    let repo = jj_repo(&r);
    let tree = |m: &str| jj_commit(&r, &repo, &format!("head ({} r)", named(m))).tree_ids().clone();
    let stored = tree("two");
    assert_eq!(stored.iter().count(), 5);
    r.j(&[&format!("goto {}", named("two"))]).ok();
    let conflicted = "\\r -> show (conflicted (files r))";
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"a\"] [\"b\"]]");
    assert_eq!(r.j(&["\\r -> show (changed r)"]).ok().stdout.trim(), "[[\"b\"]]");
    let inherited = "\\r -> contentAt [\"a\"] (files r) == contentAt [\"a\"] (files (up r))";
    assert_eq!(r.j(&[inherited]).ok().stdout.trim(), "true");
    // describing it, and its parent, keeps the tree jj stored
    r.j(&["describe \"two, described\""]).ok();
    r.j(&["at parents (describe \"one, described\")"]).ok();
    let repo = jj_repo(&r);
    let tree = |id: &str| jj_commit(&r, &repo, id).tree_ids().clone();
    assert_eq!(tree("r.root.id"), stored);
    // and a new child holds it too
    r.j(&["new"]).ok();
    let repo = jj_repo(&r);
    let tree = |id: &str| jj_commit(&r, &repo, id).tree_ids().clone();
    assert_eq!(tree("r.root.id"), stored);
    assert_eq!(tree("(up r).root.id"), stored);
    assert_eq!(r.j(&["\\r -> show (changed r)"]).ok().stdout.trim(), "[]");
}

#[test]
fn a_commit_left_with_its_parents_files_is_written_with_the_parents_tree() {
    // `squash`, `rebase` and `abandon` build a commit's files anew, and a
    // tree built from them holds each conflict less the sides that cancel.
    // Where the parent is a tree jj merged, which keeps them, the child
    // left with its parent's files got another tree for them: jj took it
    // for a change at every conflicted path, `tree` did not mark it empty,
    // and the checkout rewrote the conflict files without jj's labels
    // (§7.5). Here the parent is made only with `j`: the snapshot rebases
    // the stack onto its edited base as jj does
    let named = |m: &str| format!("(matching (\\c -> c.message == \"{}\") all)", m);
    let r = setup();
    r.write("a", "a1\na2\na3\n");
    r.write("b", "b1\nb2\nb3\n");
    r.j(&["describe \"B\""]).ok();
    r.j(&["new"]).ok();
    r.write("a", "a1\nA2 one\na3\n");
    r.j(&["describe \"one\""]).ok();
    r.j(&["new"]).ok();
    r.write("b", "b1\nB2 two\nb3\n");
    r.j(&["describe \"two\""]).ok();
    r.j(&[&format!("goto {}", named("B"))]).ok();
    r.write("a", "a1\nA2 B\na3\n");
    r.write("b", "b1\nB2 B\nb3\n");
    r.j(&["id"]).ok();
    let two = format!("head ({} r)", named("two"));
    let stored = jj_commit(&r, &jj_repo(&r), &two).tree_ids().clone();
    assert_eq!(stored.iter().count(), 5);
    // the commit `id` names holds two's tree, and jj takes it for empty
    let empty_on_two = |id: &str, what: &str| {
        let repo = jj_repo(&r);
        let c = jj_commit(&r, &repo, id);
        assert_eq!(jj_commit(&r, &repo, &two).tree_ids(), &stored, "{}", what);
        assert_eq!(c.tree_ids(), &stored, "{}", what);
        assert!(pollster::block_on(c.is_empty(repo.as_ref())).unwrap(), "{}", what);
        assert_eq!(r.j(&[&format!("\\r -> show (changed (by ({}) r))", id)]).ok().stdout.trim(), "[]");
    };
    // squash: two is rewritten with its own files, and the new focus on it
    // holds them
    r.j(&[&format!("goto {}", named("two"))]).ok();
    r.j(&["new"]).ok();
    let labelled = r.read("a");
    assert!(labelled.contains("\"two\" (rebased revision)"), "{}", labelled);
    r.j(&["squash"]).ok();
    empty_on_two("r.root.id", "squash");
    assert_eq!(r.read("a"), labelled);
    // squash into one, whose tree of three terms the list builds again with
    // the same ones but no labels: the new focus keeps jj's labels too
    r.j(&[&format!("new . goto {}", named("one"))]).ok();
    let labelled = r.read("a");
    assert!(labelled.contains("\"one\" (rebased revision)"), "{}", labelled);
    r.j(&["squash"]).ok();
    let repo = jj_repo(&r);
    let one = jj_commit(&r, &repo, "(up r).root.id").tree();
    assert_eq!(one.tree_ids().iter().count(), 3);
    assert_eq!(jj_commit(&r, &repo, "r.root.id").tree().tree_ids_and_labels(), one.tree_ids_and_labels());
    assert_eq!(r.read("a"), labelled);
    // rebase: an empty commit replayed onto two
    r.j(&[&format!("new . goto {}", named("B"))]).ok();
    r.j(&["describe \"empty\""]).ok();
    r.j(&[&format!("rebase {}", named("two"))]).ok();
    empty_on_two("r.root.id", "rebase");
    // abandon: Y, empty, replayed from X onto two
    r.j(&[&format!("new . goto {}", named("two"))]).ok();
    r.write("c", "c\n");
    r.j(&["describe \"X\""]).ok();
    r.j(&["new"]).ok();
    r.j(&["describe \"Y\""]).ok();
    r.j(&[&format!("goto {}", named("X"))]).ok();
    r.j(&["abandon"]).ok();
    empty_on_two(&format!("head ({} r)", named("Y")), "abandon");
    // a squash that changes the parent's files gives it a tree of its own,
    // which the new focus holds
    r.j(&[&format!("new . goto {}", named("two"))]).ok();
    r.write("d", "d\n");
    r.j(&["squash"]).ok();
    let repo = jj_repo(&r);
    let c = jj_commit(&r, &repo, "r.root.id");
    let parent = jj_commit(&r, &repo, "(up r).root.id").tree_ids().clone();
    assert_ne!(parent, stored);
    assert_eq!(c.tree_ids(), &parent);
    assert!(pollster::block_on(c.is_empty(repo.as_ref())).unwrap());
    let parent_files = "\\r -> show (map (.path) (files (up r)))";
    assert_eq!(r.j(&[parent_files]).ok().stdout.trim(), "[[\"a\"] [\"b\"] [\"d\"]]");
}

#[test]
fn a_conflict_whose_sides_left_are_directories_keeps_its_files() {
    // a conflict at `d` whose file sides cancel, leaving directories only.
    // jj keeps it as one conflict at the path; without its file sides it
    // would be one between the directories, which jj merges entry by
    // entry. So `j` reads it with every side, and a new child keeps it
    // (§7.3)
    let r = setup();
    let dir = |x: &str| {
        let _ = std::fs::remove_file(r.dir.join("d"));
        std::fs::create_dir_all(r.dir.join("d")).unwrap();
        r.write("d/x", x);
    };
    let file = || {
        let _ = std::fs::remove_dir_all(r.dir.join("d"));
        r.write("d", "f\n");
    };
    let sides: [(&str, &dyn Fn()); 5] =
        [("A", &|| dir("1\n")), ("B", &file), ("C", &file), ("D", &|| dir("0\n")), ("E", &|| dir("2\n"))];
    for (m, write) in sides {
        write();
        r.write("e", m);
        r.j(&[&format!("describe \"{}\"", m)]).ok();
        r.j(&["new"]).ok();
    }
    // A - B + C - D + E, as a commit X on E
    use jj_lib::repo::Repo as _;
    let named = |m: &str| format!("head ((matching (\\c -> c.message == \"{}\") all) r)", m);
    let repo = jj_repo(&r);
    let tree = |m: &str| jj_commit(&r, &repo, &named(m)).tree_ids().as_resolved().unwrap().clone();
    let trees = jj_lib::merge::Merge::from_vec(["A", "B", "C", "D", "E"].map(tree).to_vec());
    let tree = jj_lib::merged_tree::MergedTree::new(
        repo.store().clone(),
        trees,
        jj_lib::conflict_labels::ConflictLabels::unlabeled(),
    );
    let mut tx = repo.start_transaction();
    let parent = jj_commit(&r, &repo, &named("E")).id().clone();
    pollster::block_on(tx.repo_mut().new_commit(vec![parent], tree.clone()).set_description("X").write()).unwrap();
    pollster::block_on(tx.commit("X")).unwrap();
    r.j(&["goto (matching (\\c -> c.message == \"X\") all)"]).ok();
    let conflicted = "\\r -> show (conflicted (files r))";
    assert_eq!(r.j(&[conflicted]).ok().stdout.trim(), "[[\"d\"] [\"e\"]]");
    assert_eq!(r.j(&["\\r -> show (map (.path) (files r))"]).ok().stdout.trim(), "[[\"d\"] [\"e\"]]");
    let text = r.j(&["\\r -> text (contentAt [\"d\"] (files r))"]).ok().stdout;
    assert_eq!(text.matches("%%%%%%%").count(), 2, "{}", text);
    r.j(&["new"]).ok();
    let repo = jj_repo(&r);
    assert_eq!(jj_commit(&r, &repo, "r.root.id").tree_ids(), tree.tree_ids());
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
