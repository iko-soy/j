//! Interop tests: `jj` (the real binary) reads what `j` writes and `j` reads
//! what `jj` writes (§7 preamble). Skipped automatically if jj is absent.

use std::path::PathBuf;
use std::process::Command;

fn jj_available() -> bool {
    Command::new("jj")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn j_bin() -> PathBuf {
    option_env!("CARGO_BIN_EXE_j").map(PathBuf::from).unwrap_or_else(|| {
        let mut p = std::env::current_exe().unwrap();
        p.pop();
        p.pop();
        p.push("j");
        p
    })
}

fn uniq(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "j-interop-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

struct T {
    dir: PathBuf,
    cfg: PathBuf,
}

fn setup() -> Option<T> {
    if !jj_available() {
        return None;
    }
    let dir = uniq("repo");
    let cfg = uniq("cfg");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&cfg.join("j")).unwrap();
    let mut config = include_str!("../config.j").to_string();
    config = config.replace("Your Name", "Test User");
    config = config.replace("you@example.com", "test@example.com");
    std::fs::write(cfg.join("j/config.j"), config).unwrap();
    Some(T { dir, cfg })
}

impl T {
    fn j(&self, args: &[&str]) -> (i32, String, String) {
        let out = Command::new(j_bin())
            .args(args)
            .current_dir(&self.dir)
            .env("XDG_CONFIG_HOME", &self.cfg)
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    }

    fn jj(&self, args: &[&str]) -> (i32, String, String) {
        let out = Command::new("jj")
            .args(args)
            .current_dir(&self.dir)
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    }
}

impl Drop for T {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
        let _ = std::fs::remove_dir_all(&self.cfg);
    }
}

#[test]
fn jj_reads_js_commits() {
    let Some(t) = setup() else { return };
    let (code, _, err) = t.j(&["init"]);
    assert_eq!(code, 0, "{}", err);
    std::fs::write(t.dir.join("a.txt"), "content\n").unwrap();
    t.j(&["describe \"from j\""]);
    let (code, log, _) = t.jj(&["log", "--no-pager", "-T", "description"]);
    assert_eq!(code, 0);
    assert!(log.contains("from j"), "{}", log);
}

#[test]
fn j_reads_jjs_commits() {
    let Some(t) = setup() else { return };
    // init with jj, then drive with j
    std::fs::write(t.dir.join("a.txt"), "one\n").unwrap();
    let (code, _, err) = t.jj(&["git", "init", "--colocate"]);
    assert_eq!(code, 0, "{}", err);
    let (code, _, err) = t.jj(&["describe", "-m", "from jj"]);
    assert_eq!(code, 0, "{}", err);
    let (code, status, err) = t.j(&["status"]);
    assert_eq!(code, 0, "{}", err);
    assert!(status.contains("from jj"), "{}", status);
}

#[test]
fn jj_sees_js_operations() {
    let Some(t) = setup() else { return };
    t.j(&["init"]);
    std::fs::write(t.dir.join("a.txt"), "x\n").unwrap();
    t.j(&["describe \"work\""]);
    t.j(&["new"]);
    let (code, ops, _) = t.jj(&["op", "log", "--no-pager"]);
    assert_eq!(code, 0);
    assert!(ops.contains("describe \"work\""), "{}", ops);
    assert!(ops.contains("new"), "{}", ops);
}

#[test]
fn jj_can_continue_from_js_state() {
    let Some(t) = setup() else { return };
    t.j(&["init"]);
    std::fs::write(t.dir.join("a.txt"), "x\n").unwrap();
    t.j(&["describe \"base\""]);
    // jj edits and snapshots
    std::fs::write(t.dir.join("b.txt"), "y\n").unwrap();
    let (code, _, _) = t.jj(&["describe", "-m", "jj continues"]);
    assert_eq!(code, 0);
    let (code, log, _) = t.jj(&["log", "--no-pager", "-T", "description"]);
    assert_eq!(code, 0);
    assert!(log.contains("jj continues"), "{}", log);
    // j reads it back
    let (code, status, _) = t.j(&["status"]);
    assert_eq!(code, 0);
    assert!(status.contains("jj continues"), "{}", status);
}

#[test]
fn jj_undo_interop_marker() {
    let Some(t) = setup() else { return };
    t.j(&["init"]);
    std::fs::write(t.dir.join("a.txt"), "x\n").unwrap();
    t.j(&["describe \"one\""]);
    // jj's own undo
    let (code, _, _) = t.jj(&["undo"]);
    assert_eq!(code, 0);
    // j's ops shows jj's undo
    let (code, ops, _) = t.j(&["ops"]);
    assert_eq!(code, 0);
    assert!(ops.contains("undo"), "{}", ops);
}

#[test]
fn jj_reads_conflicts_written_by_j() {
    let Some(t) = setup() else { return };
    let j_ok = |expr: &str| {
        let (code, _, err) = t.j(&[expr]);
        assert_eq!(code, 0, "{}: {}", expr, err);
    };
    j_ok("init");
    std::fs::write(t.dir.join("f.txt"), "base\n").unwrap();
    j_ok("describe \"base\"");
    // side 1
    j_ok("new");
    std::fs::write(t.dir.join("f.txt"), "one\n").unwrap();
    j_ok("describe \"one\"");
    // side 2 from the parent
    j_ok("new . prev");
    std::fs::write(t.dir.join("f.txt"), "two\n").unwrap();
    j_ok("describe \"two\"");
    // rebase two onto one: conflicts
    let (code, _, err) = t.j(&["rebase (matching (\\c -> c.message == \"one\") all)"]);
    assert_eq!(code, 0, "{}", err);
    // jj sees the conflict
    let (code, log, _) = t.jj(&["log", "--no-pager"]);
    assert_eq!(code, 0);
    assert!(log.contains("conflict"), "{}", log);
    // resolve via jj's working copy and j records it
    std::fs::write(t.dir.join("f.txt"), "resolved\n").unwrap();
    let (code, _, _) = t.j(&["id"]);
    assert_eq!(code, 0);
    let (code, log, _) = t.jj(&["log", "--no-pager"]);
    assert_eq!(code, 0);
    assert!(!log.contains("conflict"), "{}", log);
}
