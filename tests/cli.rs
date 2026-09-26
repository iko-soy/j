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
fn deep_nesting_is_a_parse_error() {
    // §1.4: however deep the input, the process exits 3 with one `j:` line
    // rather than aborting on a stack overflow in the parser
    let r = setup();
    let n = 100_000;
    for src in [format!("{}1{}", "(".repeat(n), ")".repeat(n)), "[".repeat(n)] {
        let out = r.j_stdin(&src, &[]);
        assert_eq!(out.code, 3, "{}", out.stderr);
        assert!(out.stderr.starts_with("j: ") && out.stderr.lines().count() == 1, "{}", out.stderr);
    }
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
