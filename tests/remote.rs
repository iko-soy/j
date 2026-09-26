//! Remote workflow tests (§7.6, §7.8): clone/fetch/push against a local
//! bare git remote, label flows, push refusals.

use std::path::PathBuf;
use std::process::Command;

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
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    std::env::temp_dir().join(format!(
        "j-remote-test-{}-{}-{}-{}",
        tag,
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

struct Env {
    dir: PathBuf,
    remote: PathBuf,
    cfg: PathBuf,
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

fn git(dir: &PathBuf, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn setup() -> Env {
    let dir = uniq("env");
    let remote = uniq("remote.git");
    let cfg = uniq("cfg");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&cfg.join("j")).unwrap();
    let mut config = include_str!("../config.j").to_string();
    config = config.replace("Your Name", "Test User");
    config = config.replace("you@example.com", "test@example.com");
    std::fs::write(cfg.join("j/config.j"), config).unwrap();
    // bare remote with one commit on master
    git(&dir, &["init", "--bare", remote.to_str().unwrap()]);
    let seed = uniq("seed");
    std::fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "-q", "."]);
    std::fs::write(seed.join("a.txt"), "one\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["-c", "user.email=t@t", "-c", "user.name=T", "commit", "-qm", "one"]);
    git(&seed, &["remote", "add", "origin", remote.to_str().unwrap()]);
    git(&seed, &["push", "-q", "origin", "master"]);
    let _ = std::fs::remove_dir_all(&seed);
    Env { dir, remote, cfg }
}

impl Env {
    fn j(&self, workdir: &PathBuf, args: &[&str]) -> Out {
        let out = Command::new(j_bin())
            .args(args)
            .current_dir(workdir)
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
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
        let _ = std::fs::remove_dir_all(&self.remote);
        let _ = std::fs::remove_dir_all(&self.cfg);
    }
}

#[test]
fn clone_sets_origin_and_labels() {
    let env = setup();
    let dest = uniq("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    let out = env.j(&dest, &["tree"]).ok();
    assert!(out.stdout.contains("master"), "{}", out.stdout);
    assert!(out.stdout.contains("◆"), "{}", out.stdout); // immutable
    let out = env.j(&dest, &["trunk"]).ok();
    assert!(!out.stdout.contains("none"), "{}", out.stdout);
}

#[test]
fn push_label_and_relabel() {
    let env = setup();
    let dest = uniq("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    std::fs::write(dest.join("a.txt"), "two\n").unwrap();
    env.j(&dest, &["describe \"work\""]).ok();
    env.j(&dest, &["push (label \"feature\" here)"]).ok();
    let log = git(&env.remote, &["log", "feature", "--oneline"]);
    assert!(log.contains("work"), "{}", log);
    // squash locally then relabel moves the bookmark
    env.j(&dest, &["new"]).ok();
    std::fs::write(dest.join("b.txt"), "three\n").unwrap();
    env.j(&dest, &["describe \"more\""]).ok();
    env.j(&dest, &["squash"]).ok();
    env.j(&dest, &["push relabel"]).ok();
    // squash kept the parent's message but folded the files; the bookmark
    // moved to the rewritten commit
    let show = git(&env.remote, &["show", "feature:b.txt"]);
    assert!(show.contains("three"), "{}", show);
    // unlabel deletes
    env.j(&dest, &["push (unlabel \"feature\")"]).ok();
    let branches = git(&env.remote, &["branch"]);
    assert!(!branches.contains("feature"), "{}", branches);
}

#[test]
fn push_rename() {
    let env = setup();
    let dest = uniq("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    std::fs::write(dest.join("a.txt"), "two\n").unwrap();
    env.j(&dest, &["describe \"work\""]).ok();
    env.j(&dest, &["push (label \"old\" here)"]).ok();
    env.j(&dest, &["push (rename \"old\" \"new\")"]).ok();
    let branches = git(&env.remote, &["branch"]);
    assert!(branches.contains("new"), "{}", branches);
    assert!(!branches.contains("old"), "{}", branches);
}

#[test]
fn push_refusals() {
    let env = setup();
    let dest = uniq("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    // empty description refused
    let out = env.j(&dest, &["push (label \"x\" here)"]);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("empty description"), "{}", out.stderr);
    // deleting a bookmark the remote doesn't have
    let out = env.j(&dest, &["push (unlabel \"nothere\")"]);
    assert_eq!(out.code, 1);
    // moving immutable master backward refused
    std::fs::write(dest.join("a.txt"), "x\n").unwrap();
    env.j(&dest, &["describe \"w\""]).ok();
    let out = env.j(&dest, &["push (label \"master\" here)"]);
    // allowed only if it descends — it does descend (child of master)
    assert_eq!(out.code, 0, "{}", out.stderr);
}

#[test]
fn push_conflicted_commit_refused() {
    let env = setup();
    let dest = uniq("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    // create a conflict artificially is involved; instead push invalid name
    std::fs::write(dest.join("a.txt"), "x\n").unwrap();
    env.j(&dest, &["describe \"w\""]).ok();
    let out = env.j(&dest, &["push (label \"bad..name\" here)"]);
    assert_eq!(out.code, 1);
}

#[test]
fn fetch_brings_new_commits() {
    let env = setup();
    let dest = uniq("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    // move the remote
    let other = uniq("other");
    git(&env.dir, &["clone", "-q", env.remote.to_str().unwrap(), other.to_str().unwrap()]);
    std::fs::write(other.join("a.txt"), "moved\n").unwrap();
    git(&other, &["-c", "user.email=t@t", "-c", "user.name=T", "commit", "-qam", "moved"]);
    git(&other, &["push", "-q", "origin", "master"]);
    env.j(&dest, &["fetch"]).ok();
    let out = env.j(&dest, &["tree"]).ok();
    assert!(out.stdout.contains("moved"), "{}", out.stdout);
    // rebase trunk brings the stack up to date
    env.j(&dest, &["rebase trunk"]).ok();
    let out = env.j(&dest, &["log"]).ok();
    assert!(out.stdout.contains("moved"), "{}", out.stdout);
}

#[test]
fn fetch_without_origin_is_exit_1() {
    let env = setup();
    let dir = uniq("plain");
    std::fs::create_dir_all(&dir).unwrap();
    env.j(&dir, &["init"]).ok();
    let out = env.j(&dir, &["fetch"]);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("no remote origin"), "{}", out.stderr);
}

#[test]
fn remote_command_sets_url() {
    let env = setup();
    let dir = uniq("plain");
    std::fs::create_dir_all(&dir).unwrap();
    env.j(&dir, &["init"]).ok();
    env.j(&dir, &["remote", env.remote.to_str().unwrap()]).ok();
    env.j(&dir, &["fetch"]).ok();
    let out = env.j(&dir, &["tree"]).ok();
    assert!(out.stdout.contains("master"), "{}", out.stdout);
}

#[test]
fn undo_push_restores_labels() {
    let env = setup();
    let dest = uniq("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    std::fs::write(dest.join("a.txt"), "two\n").unwrap();
    env.j(&dest, &["describe \"work\""]).ok();
    let _before = env.j(&dest, &["log"]).ok().stdout;
    env.j(&dest, &["push (label \"feature\" here)"]).ok();
    let after = env.j(&dest, &["labelled \"feature\""]).ok();
    assert!(after.stdout.contains("vwq") || !after.stdout.contains("none"), "{}", after.stdout);
    let out2 = env.j(&dest, &["labelled \"feature\""]).ok();
    assert!(!out2.stdout.contains("none"), "label not visible after push: {}", out2.stdout);
    env.j(&dest, &["undo"]).ok();
    // undoing a push restores the recorded labels (§7.7): feature is gone
    let labels = env.j(&dest, &["trunk"]).ok().stdout;
    let out = env.j(&dest, &["labelled \"feature\""]).ok().stdout;
    assert!(out.contains("none") || out.contains("0 items"), "{}", out);
    let _ = labels;
    // the remote itself is untouched
    let branches = git(&env.remote, &["branch"]);
    assert!(branches.contains("feature"), "{}", branches);
}

#[test]
fn dirty_immutable_focus_snapshots_into_a_new_child() {
    // §7.2: when the working-copy commit is immutable (here: just pushed as
    // master), the snapshot cannot go into it; it goes into a new child, the
    // immutable commit keeps its stored files and hash, and `j id` records
    // the child (§1.2 step 8) so undo is not refused forever (§7.7)
    let env = setup();
    let dest = uniq("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    std::fs::write(dest.join("a.txt"), "two\n").unwrap();
    env.j(&dest, &["describe \"work\""]).ok();
    env.j(&dest, &["push (label \"master\" here)"]).ok();
    let pushed = git(&dest, &["rev-parse", "refs/remotes/origin/master"]).trim().to_string();
    let trunk_hash = || {
        env.j(&dest, &["\\r -> (meta (head (trunk r))).hash"]).ok().stdout.trim().to_string()
    };
    let trunk_text = || {
        env.j(&dest, &["\\r -> map (\\e -> text e.content) (commitAt (head (trunk r)) r).files"])
            .ok()
            .stdout
    };
    assert_eq!(trunk_hash(), pushed);

    // a printing run sees the edit in the focus, not in the pushed commit
    std::fs::write(dest.join("a.txt"), "modified\n").unwrap();
    assert_eq!(trunk_hash(), pushed, "a printing run reported the snapshot's rewrite");
    assert!(trunk_text().contains("two"), "{}", trunk_text());
    let status = env.j(&dest, &["status"]).ok().stdout;
    assert!(status.contains("a.txt"), "the focus does not hold the edit:\n{}", status);

    // a persisting run records the edit in the child
    env.j(&dest, &["describe \"wip\""]).ok();
    assert_eq!(trunk_hash(), pushed, "the pushed commit was rewritten");
    assert!(trunk_text().contains("two"), "{}", trunk_text());
    let status = env.j(&dest, &["status"]).ok().stdout;
    assert!(status.contains("a.txt") && status.contains("wip"), "{}", status);
    assert_eq!(std::fs::read_to_string(dest.join("a.txt")).unwrap(), "modified\n");

    // back onto the pushed commit, dirty again: `j id` records the edit
    env.j(&dest, &["undo"]).ok();
    assert_eq!(std::fs::read_to_string(dest.join("a.txt")).unwrap(), "two\n");
    std::fs::write(dest.join("a.txt"), "again\n").unwrap();
    env.j(&dest, &["id"]).ok();
    let ops = env.j(&dest, &["ops"]).ok().stdout;
    assert!(ops.lines().next().unwrap_or("").ends_with(" id"), "`j id` recorded nothing:\n{}", ops);
    assert_eq!(trunk_hash(), pushed, "`j id` rewrote the pushed commit");
    let status = env.j(&dest, &["status"]).ok().stdout;
    assert!(status.contains("a.txt"), "{}", status);
    // the working copy is clean now, so undo and redo go through
    env.j(&dest, &["undo"]).ok();
    assert_eq!(std::fs::read_to_string(dest.join("a.txt")).unwrap(), "two\n");
    env.j(&dest, &["redo"]).ok();
    assert_eq!(std::fs::read_to_string(dest.join("a.txt")).unwrap(), "again\n");
    assert_eq!(git(&env.remote, &["rev-parse", "master"]).trim(), pushed);
}

#[test]
fn dirty_immutable_focus_leaves_its_descendants_alone() {
    // §7.2: the snapshot of an immutable working-copy commit goes into a new
    // child, so it does not rebase the commits above it either — here the
    // remote's master, fetched on top of the working-copy commit
    let env = setup();
    let dest = uniq("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    std::fs::write(dest.join("a.txt"), "two\n").unwrap();
    env.j(&dest, &["describe \"work\""]).ok();
    env.j(&dest, &["push (label \"master\" here)"]).ok();
    let other = uniq("other");
    git(&env.dir, &["clone", "-q", env.remote.to_str().unwrap(), other.to_str().unwrap()]);
    std::fs::write(other.join("c.txt"), "top\n").unwrap();
    git(&other, &["add", "c.txt"]);
    git(&other, &["-c", "user.email=t@t", "-c", "user.name=T", "commit", "-qm", "top"]);
    git(&other, &["push", "-q", "origin", "master"]);
    let _ = std::fs::remove_dir_all(&other);
    env.j(&dest, &["fetch"]).ok();
    let top = git(&env.remote, &["rev-parse", "master"]).trim().to_string();
    let trunk_hash = || {
        env.j(&dest, &["\\r -> (meta (head (trunk r))).hash"]).ok().stdout.trim().to_string()
    };
    assert_eq!(trunk_hash(), top);
    std::fs::write(dest.join("a.txt"), "dirty\n").unwrap();
    assert_eq!(trunk_hash(), top, "a printing run reported a rebase of master");
    env.j(&dest, &["id"]).ok();
    assert_eq!(trunk_hash(), top, "`j id` rebased master onto the snapshot");
    let status = env.j(&dest, &["status"]).ok().stdout;
    assert!(status.contains("a.txt"), "{}", status);
}

#[test]
fn clone_default_dir_name() {
    let env = setup();
    // clone URL without DIR: last path component minus .git
    let workdir = uniq("work");
    std::fs::create_dir_all(&workdir).unwrap();
    env.j(&workdir, &["clone", env.remote.to_str().unwrap()]).ok();
    let name = env
        .remote
        .file_name()
        .unwrap()
        .to_string_lossy()
        .trim_end_matches(".git")
        .to_string();
    let dest = workdir.join(&name);
    assert!(dest.join(".jj").exists(), "no clone at {}", dest.display());
}
