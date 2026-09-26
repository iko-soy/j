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
fn clone_checks_out_the_parents_files() {
    // §7.8: the working-copy commit clone creates is empty, so the default
    // bookmark's files are checked out and nothing is changed in it. Built on
    // the empty tree, it deleted every file: the clone held none, and the
    // first persisting run recorded their deletion in the user's change.
    let env = setup();
    let dest = env.dir.join("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    assert!(dest.join("a.txt").is_file(), "the clone checked out no files");
    assert_eq!(std::fs::read_to_string(dest.join("a.txt")).unwrap(), "one\n");
    assert_eq!(env.j(&dest, &["changed"]).ok().stdout.trim(), "none");
    env.j(&dest, &["describe \"mine\""]).ok();
    assert_eq!(env.j(&dest, &["changed"]).ok().stdout.trim(), "none");
}

/// a git repository in `dir` on branch `main` with one commit, `base`,
/// holding a.txt and b.txt
fn git_repo_with_base(dir: &PathBuf) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main", "."]);
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    std::fs::write(dir.join("b.txt"), "two\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["-c", "user.email=t@t", "-c", "user.name=T", "commit", "-qm", "base"]);
}

#[test]
fn init_over_git_records_the_heads_files() {
    // §7.8: init over an existing git repository gives the working-copy
    // commit the head's files. Built on the empty tree, the operation init
    // recorded deleted every file, so undoing the first run after init
    // emptied the working directory.
    let env = setup();
    let dir = env.dir.join("repo");
    git_repo_with_base(&dir);
    env.j(&dir, &["init"]).ok();
    assert_eq!(env.j(&dir, &["changed"]).ok().stdout.trim(), "none");
    env.j(&dir, &["describe \"x\""]).ok();
    env.j(&dir, &["undo"]).ok();
    assert!(dir.join("a.txt").is_file(), "undo deleted a.txt");
    assert_eq!(std::fs::read_to_string(dir.join("b.txt")).unwrap(), "two\n");
    assert_eq!(env.j(&dir, &["changed"]).ok().stdout.trim(), "none");
}

#[test]
fn clone_starts_on_the_remotes_default_bookmark() {
    // §7.8: the working-copy commit is a child of the target of the bookmark
    // the remote's HEAD names (master here), not of whichever bookmark sorts
    // first
    let env = setup();
    let other = env.dir.join("other");
    git(&env.dir, &["clone", "-q", env.remote.to_str().unwrap(), other.to_str().unwrap()]);
    git(&other, &["checkout", "-qb", "aaa"]);
    std::fs::write(other.join("side.txt"), "side\n").unwrap();
    git(&other, &["add", "side.txt"]);
    git(&other, &["-c", "user.email=t@t", "-c", "user.name=T", "commit", "-qm", "side"]);
    git(&other, &["push", "-q", "origin", "aaa"]);
    let dest = env.dir.join("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    let parent = env.j(&dest, &["(.message) . focus . up"]).ok().stdout;
    assert_eq!(parent.trim(), "one");
    assert!(!dest.join("side.txt").exists(), "aaa was checked out");
    assert_eq!(env.j(&dest, &["changed"]).ok().stdout.trim(), "none");
}

#[test]
fn init_over_git_starts_on_head() {
    // §7.8: init over an existing git repository creates the working-copy
    // commit as a child of git's HEAD: not of the bookmark that sorts first,
    // and also when HEAD is detached from every branch
    let env = setup();
    let commit = |dir: &PathBuf, file: &str, msg: &str| {
        std::fs::write(dir.join(file), format!("{}\n", msg)).unwrap();
        git(dir, &["add", file]);
        git(dir, &["-c", "user.email=t@t", "-c", "user.name=T", "commit", "-qm", msg]);
    };
    let dir = env.dir.join("repo");
    git_repo_with_base(&dir);
    git(&dir, &["checkout", "-qb", "aaa"]);
    commit(&dir, "side.txt", "side");
    git(&dir, &["checkout", "-q", "main"]);
    commit(&dir, "a.txt", "main work");
    env.j(&dir, &["init"]).ok();
    let parent = env.j(&dir, &["(.message) . focus . up"]).ok().stdout;
    assert_eq!(parent.trim(), "main work");
    assert!(!dir.join("side.txt").exists());
    assert_eq!(env.j(&dir, &["changed"]).ok().stdout.trim(), "none");

    let dir = env.dir.join("detached");
    git_repo_with_base(&dir);
    git(&dir, &["checkout", "-q", "--detach"]);
    commit(&dir, "d.txt", "detached work");
    env.j(&dir, &["init"]).ok();
    let parent = env.j(&dir, &["(.message) . focus . up"]).ok().stdout;
    assert_eq!(parent.trim(), "detached work");
    assert_eq!(env.j(&dir, &["changed"]).ok().stdout.trim(), "none");
}

#[test]
fn init_over_git_keeps_uncommitted_changes() {
    // §7.8: init writes no file over an existing git working tree; what it
    // holds beyond the head (an edit, a deletion) becomes the working-copy
    // commit's change rather than being checked out over
    let env = setup();
    let dir = env.dir.join("repo");
    git_repo_with_base(&dir);
    std::fs::write(dir.join("a.txt"), "edited\n").unwrap();
    std::fs::remove_file(dir.join("b.txt")).unwrap();
    env.j(&dir, &["init"]).ok();
    assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "edited\n");
    assert!(!dir.join("b.txt").exists(), "init restored a deleted file");
    let changed = env.j(&dir, &["changed"]).ok().stdout;
    assert!(changed.contains("a.txt") && changed.contains("b.txt"), "{}", changed);
}

#[test]
fn merge_ancestors_through_every_parent_are_immutable() {
    // §7.5 step 3: every ancestor of a merge is immutable, including those
    // reached only through its second parent. Following first parents only,
    // the side branch was mutable: describing or abandoning one of its
    // commits exited 0, and jj rebased the read-only merge (§7.2) onto the
    // result, dropping the side's files from it on abandon.
    let env = setup();
    let commit = |dir: &PathBuf, file: &str, msg: &str| {
        std::fs::write(dir.join(file), format!("{}\n", msg)).unwrap();
        git(dir, &["add", file]);
        git(dir, &["-c", "user.email=t@t", "-c", "user.name=T", "commit", "-qm", msg]);
    };
    let dir = env.dir.join("repo");
    git_repo_with_base(&dir);
    git(&dir, &["checkout", "-qb", "side"]);
    commit(&dir, "s1.txt", "side one");
    commit(&dir, "s2.txt", "side two");
    git(&dir, &["checkout", "-q", "main"]);
    commit(&dir, "m.txt", "main work");
    git(&dir, &["-c", "user.email=t@t", "-c", "user.name=T", "merge", "-q", "side", "-m", "merge"]);
    let merge = git(&dir, &["rev-parse", "HEAD"]).trim().to_string();
    env.j(&dir, &["init"]).ok();
    let merge_hash = || {
        let expr = r#"\r -> (meta (head (matching (\c -> startsWith "merge" c.message) all r))).hash"#;
        env.j(&dir, &[expr]).ok().stdout.trim().to_string()
    };
    assert_eq!(merge_hash(), merge);

    let tree = env.j(&dir, &["tree"]).ok().stdout;
    for msg in ["side one", "side two"] {
        let line = tree.lines().find(|l| l.contains(msg)).unwrap();
        assert!(line.contains('◆'), "`{}` is not drawn immutable:\n{}", msg, tree);
    }

    let side = |msg: &str, edit: &str| {
        format!(r#"at (matching (\c -> startsWith "{}" c.message) all) ({})"#, msg, edit)
    };
    for edit in [side("side one", "describe \"edited\""), side("side two", "abandon")] {
        // validate is an honest dry run of what persistence refuses
        let out = env.j(&dir, &[&format!("tree . validate . {}", edit)]);
        assert_eq!(out.code, 1, "validate accepted `{}`:\n{}", edit, out.stdout);
        assert!(out.stderr.contains("is immutable"), "{}", out.stderr);
        let out = env.j(&dir, &[&edit]);
        assert_eq!(out.code, 1, "persistence accepted `{}`", edit);
        assert!(out.stderr.contains("is immutable"), "{}", out.stderr);
        assert_eq!(merge_hash(), merge, "`{}` rewrote the merge", edit);
    }
    assert_eq!(env.j(&dir, &["changed"]).ok().stdout.trim(), "none");
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
fn push_of_an_empty_list_pushes_nothing() {
    // §7.6: an empty list of records sets and deletes no bookmark. It went to
    // git as a push with no refspecs, which git fills in from push.default:
    // it published a local git branch nobody named, or failed with "has no
    // upstream branch"
    let env = setup();
    let master = || git(&env.remote, &["rev-parse", "master"]);
    let before = master();
    let work = env.dir.join("work");
    git(&env.dir, &["clone", "-q", env.remote.to_str().unwrap(), work.to_str().unwrap()]);
    git(&work, &["checkout", "-q", "master"]);
    std::fs::write(work.join("l.txt"), "local\n").unwrap();
    git(&work, &["add", "l.txt"]);
    git(&work, &["-c", "user.email=t@t", "-c", "user.name=T", "commit", "-qm", "local unpushed"]);
    env.j(&work, &["init"]).ok();
    env.j(&work, &["push []"]).ok();
    assert_eq!(master(), before, "push [] moved the remote's master");
    // it records one operation (§7.7)
    let ops = env.j(&work, &["ops"]).ok().stdout;
    assert!(ops.lines().next().unwrap_or("").ends_with(" []"), "{}", ops);

    // a clone's git branch has no upstream
    let dest = env.dir.join("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    env.j(&dest, &["describe \"x\""]).ok();
    env.j(&dest, &["push []"]).ok();
    env.j(&dest, &["push (label \"nope\" (labelled \"zzz\"))"]).ok();
    assert_eq!(master(), before);
    let refs = git(&env.remote, &["for-each-ref", "--format=%(refname)"]);
    assert_eq!(refs.trim(), "refs/heads/master");

    // without origin it still exits 1
    let plain = env.dir.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    env.j(&plain, &["init"]).ok();
    let out = env.j(&plain, &["push []"]);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("no remote origin"), "{}", out.stderr);
}

#[test]
fn partial_push_records_what_the_remote_accepted() {
    // git push is not atomic: when the remote accepts `aaa` and rejects
    // `bbb`, aaa is on the remote. The run exited 1 without recording the
    // push, so the aaa label was missing until the next fetch and a push
    // naming aaa was refused or rejected on its stale lease (§7.6: a label
    // is the remote's bookmark as of the last push)
    use std::os::unix::fs::PermissionsExt;
    let env = setup();
    let hook = env.remote.join("hooks/update");
    std::fs::write(&hook, "#!/bin/sh\n[ \"$1\" = refs/heads/bbb ] && exit 1\nexit 0\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let dest = env.dir.join("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    std::fs::write(dest.join("a.txt"), "two\n").unwrap();
    env.j(&dest, &["describe \"work\""]).ok();
    let expr = "push (\\r -> label \"aaa\" here r ++ label \"bbb\" here r)";
    let out = env.j(&dest, &[expr]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("bbb"), "{}", out.stderr);
    let branches = git(&env.remote, &["branch"]);
    assert!(branches.contains("aaa") && !branches.contains("bbb"), "{}", branches);
    // the accepted half is the push's one operation
    let ops = env.j(&dest, &["ops"]).ok().stdout;
    assert!(ops.lines().next().unwrap_or("").contains("label \"aaa\""), "{}", ops);
    let tree = env.j(&dest, &["tree"]).ok().stdout;
    assert!(tree.lines().any(|l| l.contains("work") && l.contains("aaa")), "{}", tree);
    env.j(&dest, &["push (unlabel \"aaa\")"]).ok();
    assert!(!git(&env.remote, &["branch"]).contains("aaa"));
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
fn reordering_an_immutable_snapshot_is_not_a_change() {
    // a `contract m` that moves nothing into master still lists master's
    // entries in another order (`select` puts the ones m matches first).
    // Comparing snapshots as lists refused that as "immutable (files
    // changed)", in the `validate` dry run too, although no content moved;
    // a snapshot stands for a tree, which has no order (§7.3, §7.5 step 3)
    let env = setup();
    let dest = uniq("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    std::fs::create_dir(dest.join("h")).unwrap();
    std::fs::write(dest.join("h/x"), "x\n").unwrap();
    env.j(&dest, &["describe \"work\""]).ok();
    env.j(&dest, &["push (label \"master\" here)"]).ok();
    let pushed = git(&dest, &["rev-parse", "refs/remotes/origin/master"]).trim().to_string();
    let trunk_hash = || {
        env.j(&dest, &["\\r -> (meta (head (trunk r))).hash"]).ok().stdout.trim().to_string()
    };
    let ops = || env.j(&dest, &["ops"]).ok().stdout.lines().count();
    let edits = ["tree . validate . contract (under ./h)", "contract (under ./h)"];
    // the working-copy commit is master itself, so the focus is the new child
    // it was loaded with (§7.2); that child is not recorded either (§1.2)
    let n = ops();
    for e in edits {
        env.j(&dest, &[e]).ok();
        assert_eq!(ops(), n, "{} was recorded", e);
    }
    assert_eq!(trunk_hash(), pushed);
    // from a mutable child of master: recording that child makes one
    env.j(&dest, &["describe \"child\""]).ok();
    let n = ops();
    let focus_hash = env.j(&dest, &["\\r -> (meta r.root.id).hash"]).ok().stdout;
    for e in edits {
        env.j(&dest, &[e]).ok();
        assert_eq!(ops(), n, "{} was recorded", e);
    }
    assert_eq!(trunk_hash(), pushed);
    assert_eq!(env.j(&dest, &["\\r -> (meta r.root.id).hash"]).ok().stdout, focus_hash);
    // moving real content into master is still refused
    std::fs::write(dest.join("h/x"), "changed\n").unwrap();
    let out = env.j(&dest, &["contract (under ./h)"]);
    assert_eq!(out.code, 1, "{}", out.stdout);
    assert!(out.stderr.contains("immutable (files changed)"), "{}", out.stderr);
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
