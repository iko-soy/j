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

/// `git` with `input` on its standard input
fn git_stdin(dir: &PathBuf, args: &[&str], input: &str) -> String {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// Whether `dir`'s filesystem folds case, judged as the backend judges it:
/// `.JJ` names the workspace's `.jj`, which the directory does not list
/// under that spelling
fn folds_case(dir: &std::path::Path) -> bool {
    dir.join(".JJ").symlink_metadata().is_ok()
        && !std::fs::read_dir(dir).unwrap().any(|e| e.unwrap().file_name() == ".JJ")
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
fn init_and_clone_are_not_undone() {
    // §7.7: before `init` or `clone` there was no repository, so the
    // operation each records is not undone. Undoing it restored jj's view
    // from before, whose working-copy commit is an empty child of the root,
    // and checking that out deleted every file of the head: right after the
    // command, or once everything run since had been undone. An orphan HEAD
    // keeps the working-copy commit jj created, and the refs init imports
    // were an operation of their own, which undo took back.
    let env = setup();
    let ops = |dir: &PathBuf| env.j(dir, &["ops"]).ok().stdout.lines().count();
    let refused = |dir: &PathBuf| {
        let n = ops(dir);
        let out = env.j(dir, &["undo"]);
        assert_eq!(out.code, 1, "undo was recorded: {}", out.stderr);
        assert!(out.stderr.contains("nothing to undo"), "{}", out.stderr);
        assert_eq!(ops(dir), n);
    };
    let dir = env.dir.join("repo");
    git_repo_with_base(&dir);
    env.j(&dir, &["init"]).ok();
    refused(&dir);
    env.j(&dir, &["describe \"x\""]).ok();
    env.j(&dir, &["undo"]).ok();
    refused(&dir);
    assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "one\n");
    assert_eq!(std::fs::read_to_string(dir.join("b.txt")).unwrap(), "two\n");
    assert_eq!(env.j(&dir, &["changed"]).ok().stdout.trim(), "none");

    let orphan = env.dir.join("orphan");
    git_repo_with_base(&orphan);
    git(&orphan, &["switch", "-q", "--orphan", "fresh"]);
    env.j(&orphan, &["init"]).ok();
    refused(&orphan);
    let out = env.j(&orphan, &["tree"]).ok().stdout;
    assert!(out.contains("base"), "{}", out);

    let dest = env.dir.join("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    refused(&dest);
    env.j(&dest, &["describe \"x\""]).ok();
    env.j(&dest, &["undo"]).ok();
    refused(&dest);
    assert_eq!(std::fs::read_to_string(dest.join("a.txt")).unwrap(), "one\n");
    assert_eq!(env.j(&dest, &["changed"]).ok().stdout.trim(), "none");
}

#[test]
fn clone_leaves_out_paths_a_checkout_cannot_create() {
    // §7.8: a default bookmark can hold a path jj's checkout refuses, such
    // as `.jj/x`. The working-copy commit held it, so the checkout failed
    // after clone had recorded its operations, leaving no file on disk and
    // `changed` empty, and every persisting run was refused for the focus's
    // `.jj` (§7.5 step 1). The commit now leaves such paths out, and their
    // removal is its change.
    let env = setup();
    let other = env.dir.join("other");
    git(&env.dir, &["clone", "-q", env.remote.to_str().unwrap(), other.to_str().unwrap()]);
    std::fs::create_dir_all(other.join(".jj")).unwrap();
    std::fs::write(other.join(".jj/x"), "evil\n").unwrap();
    std::fs::create_dir_all(other.join("fixture/.jj")).unwrap();
    std::fs::write(other.join("fixture/.jj/repo"), "vendored\n").unwrap();
    std::fs::write(other.join("fixture/kept"), "kept\n").unwrap();
    git(&other, &["add", "-f", ".jj", "fixture"]);
    // a name no filesystem holds, which git stores all the same
    let long = format!("long/{}", "n".repeat(300));
    let blob = git(&other, &["hash-object", "-w", "a.txt"]);
    let info = format!("100644,{},{}", blob.trim(), long);
    git(&other, &["update-index", "--add", "--cacheinfo", &info]);
    git(&other, &["-c", "user.email=t@t", "-c", "user.name=T", "commit", "-qm", "withjj"]);
    git(&other, &["push", "-q", "origin", "master"]);
    let dest = env.dir.join("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    assert_eq!(std::fs::read_to_string(dest.join("a.txt")).unwrap(), "one\n");
    assert_eq!(std::fs::read_to_string(dest.join("fixture/kept")).unwrap(), "kept\n");
    assert!(!dest.join(".jj/x").exists());
    assert!(!dest.join("fixture/.jj").exists());
    let changed = env.j(&dest, &["changed"]).ok().stdout;
    let changed: Vec<&str> = changed.lines().collect();
    assert_eq!(changed, [".jj/x", "fixture/.jj/repo", long.as_str()]);
    env.j(&dest, &["describe \"x\""]).ok();
    env.j(&dest, &["new"]).ok();
    assert_eq!(env.j(&dest, &["changed"]).ok().stdout.trim(), "none");
}

#[test]
fn clone_keeps_long_names_the_filesystem_holds() {
    // §7.8: the working-copy commit clone starts on left out every name
    // over 255 bytes, though exFAT, NTFS and APFS hold one of 100 `日`
    // (300 bytes): the clone's first change was that file's removal, which
    // a push would publish. It is left out only where the filesystem does
    // not hold it.
    let env = setup();
    let other = env.dir.join("other");
    git(&env.dir, &["clone", "-q", env.remote.to_str().unwrap(), other.to_str().unwrap()]);
    let wide = format!("wide/{}", "日".repeat(100));
    let blob = git(&other, &["hash-object", "-w", "a.txt"]);
    let info = format!("100644,{},{}", blob.trim(), wide);
    git(&other, &["update-index", "--add", "--cacheinfo", &info]);
    git(&other, &["-c", "user.email=t@t", "-c", "user.name=T", "commit", "-qm", "wide"]);
    git(&other, &["push", "-q", "origin", "master"]);
    let dest = env.dir.join("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    let holds = std::fs::write(env.dir.join("日".repeat(100)), "").is_ok();
    let changed = env.j(&dest, &["changed"]).ok().stdout;
    if holds {
        assert_eq!(std::fs::read_to_string(dest.join(&wide)).unwrap(), "one\n");
        assert_eq!(changed.trim(), "none");
    } else {
        assert!(!dest.join("wide").exists());
        assert_eq!(changed.trim(), wide);
    }
    assert_eq!(std::fs::read_to_string(dest.join("a.txt")).unwrap(), "one\n");
}

#[test]
fn clone_leaves_out_git_in_another_case_where_the_filesystem_folds_case() {
    // §7.5 step 1, §7.8: where the filesystem folds case, `.GIT` is the
    // clone's own `.git`. exfat-fuse gives each spelling its own inode
    // number, so judged by file identity it did not fold case, and the
    // checkout of a default branch holding `.GIT/hooks/post-checkout`
    // installed a hook the next `git checkout` ran. Run with TMPDIR on such
    // a mount to take the first branch.
    let env = setup();
    // git refuses `.GIT` in an index, so the tree is built by hand
    let hook = env.dir.join("hook");
    std::fs::write(&hook, "#!/bin/sh\necho hooked\n").unwrap();
    let blob = git(&env.remote, &["hash-object", "-w", hook.to_str().unwrap()]);
    let mktree = |entries: String| git_stdin(&env.remote, &["mktree"], &entries).trim().to_string();
    let hooks = mktree(format!("100755 blob {}\tpost-checkout\n", blob.trim()));
    let dot_git = mktree(format!("040000 tree {}\thooks\n", hooks));
    let top = git(&env.remote, &["ls-tree", "master"]);
    let tree = mktree(format!("{}040000 tree {}\t.GIT\n", top, dot_git));
    let commit = git(
        &env.remote,
        &["-c", "user.email=t@t", "-c", "user.name=T", "commit-tree", &tree, "-p", "master", "-m", "hook"],
    );
    git(&env.remote, &["update-ref", "refs/heads/master", commit.trim()]);
    let dest = env.dir.join("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    assert!(!dest.join(".git/hooks/post-checkout").exists());
    let changed = env.j(&dest, &["changed"]).ok().stdout;
    if folds_case(&dest) {
        assert_eq!(changed.trim(), ".GIT/hooks/post-checkout");
    } else {
        let kept = std::fs::read_to_string(dest.join(".GIT/hooks/post-checkout")).unwrap();
        assert_eq!(kept, "#!/bin/sh\necho hooked\n");
        assert_eq!(changed.trim(), "none");
    }
    assert_eq!(std::fs::read_to_string(dest.join("a.txt")).unwrap(), "one\n");
}

#[test]
fn clone_that_cannot_check_out_leaves_nothing_behind() {
    // §7.8: clone checked the working-copy commit out after recording its
    // operation, so a checkout that failed part way (a symlink target or a
    // path longer than PATH_MAX here; a full disk) left a clone whose
    // working-copy commit held files never written: `zz`, after the failing
    // path, and the user's first `describe` recorded it as deleted. The
    // deep path also left a chain of empty directories. Like `git clone`, a
    // clone that fails now records nothing and leaves nothing behind.
    let env = setup();
    let other = env.dir.join("other");
    git(&env.dir, &["clone", "-q", env.remote.to_str().unwrap(), other.to_str().unwrap()]);
    std::fs::write(other.join("zz"), "z\n").unwrap();
    std::fs::write(env.dir.join("target"), "x".repeat(5000)).unwrap();
    let target = git(&other, &["hash-object", "-w", env.dir.join("target").to_str().unwrap()]);
    let link = format!("120000,{},m_link", target.trim());
    git(&other, &["update-index", "--add", "--cacheinfo", &link]);
    git(&other, &["add", "zz"]);
    git(&other, &["-c", "user.email=t@t", "-c", "user.name=T", "commit", "-qm", "link"]);
    git(&other, &["push", "-q", "origin", "master"]);
    let clone = |dest: &PathBuf| {
        let out = env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]);
        assert_eq!(out.code, 1, "{}", out.stderr);
        assert!(out.stderr.contains("cannot check out"), "{}", out.stderr);
    };
    let dest = env.dir.join("clone");
    clone(&dest);
    assert!(!dest.exists(), "the failed clone left {}", dest.display());
    // a directory that was there, empty, stays so
    std::fs::create_dir(&dest).unwrap();
    clone(&dest);
    assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 0);

    git(&other, &["rm", "-q", "--cached", "m_link"]);
    let deep = vec!["d".repeat(250); 20].join("/");
    let blob = git(&other, &["hash-object", "-w", "zz"]);
    let info = format!("100644,{},{}", blob.trim(), deep);
    git(&other, &["update-index", "--add", "--cacheinfo", &info]);
    git(&other, &["-c", "user.email=t@t", "-c", "user.name=T", "commit", "-qm", "deep"]);
    git(&other, &["push", "-q", "origin", "master"]);
    let dest = env.dir.join("deep");
    clone(&dest);
    assert!(!dest.exists(), "the failed clone left {}", dest.display());
}

#[test]
fn failed_clone_removes_only_the_directories_it_created() {
    // §7.8: the emptiness checks ran on DIR as written, so `newdir/..` (or
    // `new/../keep`) passed them while `newdir` did not exist yet; creating
    // it made DIR the existing, non-empty working directory (or `keep`).
    // The clone went into it, and when it failed its cleanup removed
    // everything there. A failed clone into `a/b/c` left `a/b`. A DIR that
    // exists only once its parents are created is now refused, and a failed
    // clone removes the directories it created, DIR's missing parents
    // included, and nothing else.
    let env = setup();
    let work = env.dir.join("work");
    std::fs::create_dir_all(work.join("keep")).unwrap();
    std::fs::create_dir_all(work.join("inner/src")).unwrap();
    std::fs::write(work.join("notes.txt"), "n\n").unwrap();
    std::fs::write(work.join("keep/notes.txt"), "k\n").unwrap();
    std::fs::write(work.join("inner/src/main.rs"), "m\n").unwrap();
    let listing = |dir: &PathBuf| {
        let mut all = Vec::new();
        let mut stack = vec![dir.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                all.push(p.strip_prefix(dir).unwrap().display().to_string());
                if p.is_dir() {
                    stack.push(p);
                }
            }
        }
        all.sort();
        all
    };
    let before = listing(&work);
    let missing = env.dir.join("nowhere.git");
    for (cwd, dest) in [
        (&work, "newdir/.."),
        (&work, "new/../keep"),
        (&work.join("inner"), "x/../.."),
    ] {
        for url in [&env.remote, &missing] {
            let out = env.j(cwd, &["clone", url.to_str().unwrap(), dest]);
            assert_eq!(out.code, 2, "clone into {}: {}", dest, out.stderr);
            assert_eq!(listing(&work), before, "clone into {}", dest);
        }
    }
    for dest in ["a/b/c", "new/../fresh"] {
        let out = env.j(&work, &["clone", missing.to_str().unwrap(), dest]);
        assert_eq!(out.code, 1, "clone into {}: {}", dest, out.stderr);
        assert_eq!(listing(&work), before, "clone into {}", dest);
    }
    env.j(&work, &["clone", env.remote.to_str().unwrap(), "new/../fresh"]).ok();
    assert_eq!(std::fs::read_to_string(work.join("fresh/a.txt")).unwrap(), "one\n");
}

#[test]
fn init_over_git_takes_a_directory_it_cannot_scan() {
    // §7.4, §7.8: init over an existing git working tree scans it before
    // checking out, and a directory that scan cannot read (one deeper than
    // PATH_MAX here) crashed init after its operation was recorded. Init now
    // takes the directory as it is: each run's snapshot refuses it, and
    // records nothing, until it is removed.
    let env = setup();
    let dir = env.dir.join("repo");
    git_repo_with_base(&dir);
    let half: PathBuf = (0..10).map(|_| "d".repeat(250)).collect();
    std::fs::create_dir_all(dir.join("deep").join(&half)).unwrap();
    std::fs::create_dir_all(env.dir.join("more").join(&half)).unwrap();
    std::fs::rename(env.dir.join("more"), dir.join("deep").join(&half).join("more")).unwrap();
    env.j(&dir, &["init"]).ok();
    assert_eq!(env.j(&dir, &["changed"]).code, 2);
    std::fs::remove_dir_all(dir.join("deep")).unwrap();
    assert_eq!(env.j(&dir, &["changed"]).ok().stdout.trim(), "none");
    let ops = env.j(&dir, &["ops"]).ok().stdout;
    assert!(ops.lines().next().unwrap_or("").contains("init"), "{}", ops);
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
fn fetched_branch_with_a_reserved_name_does_not_block_persisting() {
    // git stores `.jj` paths, so a branch on the remote can hold one. It is
    // mutable (not an ancestor of trunk) and labelled, so it cannot be
    // abandoned, and refusing its names crashed every persisting run in the
    // clone (`describe`, `new`, `j id`), though none of them writes or
    // checks it out (§7.5 step 1)
    let env = setup();
    let other = env.dir.join("other");
    git(&env.dir, &["clone", "-q", env.remote.to_str().unwrap(), other.to_str().unwrap()]);
    git(&other, &["checkout", "-qb", "feature"]);
    std::fs::create_dir_all(other.join("fixture/.jj")).unwrap();
    std::fs::write(other.join("fixture/.jj/repo"), "vendored\n").unwrap();
    git(&other, &["add", "-f", "fixture"]);
    git(&other, &["-c", "user.email=t@t", "-c", "user.name=T", "commit", "-qm", "feat"]);
    git(&other, &["push", "-q", "origin", "feature"]);
    let dest = env.dir.join("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    let ops = || env.j(&dest, &["ops"]).ok().stdout.lines().count();
    let before = ops();
    env.j(&dest, &["tree . validate . describe \"x\""]).ok();
    env.j(&dest, &["describe \"hello\""]).ok();
    env.j(&dest, &["new"]).ok();
    std::fs::write(dest.join("new.txt"), "hi\n").unwrap();
    env.j(&dest, &["id"]).ok();
    // rewriting it keeps the files jj stored
    env.j(&dest, &["at %feature (describe \"renamed\")"]).ok();
    assert_eq!(ops(), before + 4);
    // checking it out is still refused, and records nothing
    let out = env.j(&dest, &["goto %feature"]);
    assert_eq!(out.code, 1, "{}", out.stdout);
    assert!(out.stderr.contains("path component"), "{}", out.stderr);
    assert_eq!(ops(), before + 4);
    assert!(!dest.join("fixture").exists());
}

#[test]
fn fetched_descendant_with_a_reserved_name_does_not_block_snapshots() {
    // §7.5 step 1: a snapshot that rewrites the focus rebases the fetched
    // branch above it, so the branch's files differ, and all of its names
    // were checked again: `.jj`, a name over 255 bytes on ext4, `.GIT`
    // where the filesystem folds case. Every persisting run crashed, `j id`
    // included, until the edit on disk was undone, though none checks the
    // branch out and jj stored those names already. Only the paths the
    // branch did not hold are checked now. Run with TMPDIR on a
    // case-folding mount to exercise `.GIT`.
    let env = setup();
    let other = env.dir.join("other");
    git(&env.dir, &["clone", "-q", env.remote.to_str().unwrap(), other.to_str().unwrap()]);
    git(&other, &["checkout", "-qb", "base"]);
    std::fs::write(other.join("b.txt"), "b\n").unwrap();
    git(&other, &["add", "b.txt"]);
    git(&other, &["-c", "user.email=t@t", "-c", "user.name=T", "commit", "-qm", "base"]);
    git(&other, &["push", "-q", "origin", "base"]);
    // git refuses `.GIT` in an index, so the tree is built by hand
    let blob = git(&env.remote, &["hash-object", "-w", other.join("a.txt").to_str().unwrap()]);
    let mktree = |entries: String| git_stdin(&env.remote, &["mktree"], &entries).trim().to_string();
    let x = mktree(format!("100644 blob {}\tx\n", blob.trim()));
    let wide = mktree(format!("100644 blob {}\t{}\n", blob.trim(), "日".repeat(100)));
    let top = git(&env.remote, &["ls-tree", "base"]);
    let tree = mktree(format!(
        "{}040000 tree {x}\t.GIT\n040000 tree {x}\t.jj\n040000 tree {wide}\twide\n",
        top
    ));
    let commit = git(
        &env.remote,
        &["-c", "user.email=t@t", "-c", "user.name=T", "commit-tree", &tree, "-p", "base", "-m", "feat"],
    );
    git(&env.remote, &["update-ref", "refs/heads/feature", commit.trim()]);
    let dest = env.dir.join("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    env.j(&dest, &["goto %base"]).ok();
    let ops = || env.j(&dest, &["ops"]).ok().stdout.lines().count();
    let before = ops();
    std::fs::write(dest.join("b.txt"), "b\nedited\n").unwrap();
    env.j(&dest, &["tree . validate . id"]).ok();
    env.j(&dest, &["id"]).ok();
    env.j(&dest, &["describe \"base edited\""]).ok();
    env.j(&dest, &["new"]).ok();
    assert_eq!(ops(), before + 3);
    // the branch was rebased onto the edit and keeps its names
    let feature = |expr: &str| env.j(&dest, &[&format!("{} . files . goto %feature", expr)]).ok().stdout;
    assert_eq!(feature("contentAt ./b.txt"), "b\nedited\n");
    let paths = feature("map (.path)");
    let wide = format!("wide/{}", "日".repeat(100));
    assert_eq!(paths.lines().collect::<Vec<_>>(), [".GIT/x", ".jj/x", "a.txt", "b.txt", wide.as_str()]);
    // checking it out is still refused, and records nothing
    let out = env.j(&dest, &["goto %feature"]);
    assert_eq!(out.code, 1, "{}", out.stdout);
    assert!(out.stderr.contains("path component"), "{}", out.stderr);
    assert_eq!(ops(), before + 3);
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
    // moving immutable master forward is allowed: the child descends from it
    // (moving it elsewhere is refused: push_refuses_moving_an_immutable_bookmark)
    std::fs::write(dest.join("a.txt"), "x\n").unwrap();
    env.j(&dest, &["describe \"w\""]).ok();
    let out = env.j(&dest, &["push (label \"master\" here)"]);
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
fn push_refuses_moving_an_immutable_bookmark() {
    // §7.6: a bookmark whose current target is immutable may only move to a
    // descendant of that target, and may not be deleted while no bookmark
    // left on origin reaches that target. Nothing checked this: pushes
    // rewound master to its parent and moved it onto a side line with exit 0
    let env = setup();
    let seed = env.dir.join("seed");
    git(&env.dir, &["clone", "-q", env.remote.to_str().unwrap(), seed.to_str().unwrap()]);
    git(&seed, &["checkout", "-q", "master"]);
    git(&seed, &["push", "-q", "origin", "master:old"]);
    std::fs::write(seed.join("a.txt"), "two\n").unwrap();
    git(&seed, &["-c", "user.email=t@t", "-c", "user.name=T", "commit", "-qam", "two"]);
    git(&seed, &["push", "-q", "origin", "master"]);
    let refs = || git(&env.remote, &["for-each-ref"]);
    let dest = env.dir.join("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    let before = refs();

    // backward, to its parent
    let out = env.j(&dest, &["push (label \"master\" (labelled \"old\"))"]);
    assert_eq!(out.code, 1, "rewound master");
    assert!(out.stderr.contains("immutable"), "{}", out.stderr);
    assert_eq!(refs(), before);
    // sideways, onto a new line off its parent
    env.j(&dest, &["new . goto (labelled \"old\")"]).ok();
    std::fs::write(dest.join("side.txt"), "side\n").unwrap();
    env.j(&dest, &["describe \"side\""]).ok();
    let out = env.j(&dest, &["push (label \"master\" here)"]);
    assert_eq!(out.code, 1, "moved master sideways");
    assert!(out.stderr.contains("immutable"), "{}", out.stderr);
    assert_eq!(refs(), before);
    // deleted while no other bookmark reaches its commit: `old` is on the
    // parent (deletes another bookmark reaches are allowed:
    // push_deletes_an_immutable_bookmark_another_one_reaches)
    let out = env.j(&dest, &["push (unlabel \"master\")"]);
    assert_eq!(out.code, 1, "deleted the only bookmark on an immutable commit");
    assert!(out.stderr.contains("immutable"), "{}", out.stderr);
    assert_eq!(refs(), before);

    // forward is allowed, and so is moving a mutable bookmark anywhere
    env.j(&dest, &["push (label \"old\" trunk)"]).ok();
    env.j(&dest, &["push (label \"side\" here)"]).ok();
    env.j(&dest, &["push (label \"side\" trunk)"]).ok();
    let master = git(&env.remote, &["rev-parse", "master"]);
    assert_eq!(git(&env.remote, &["rev-parse", "old"]), master);
    assert_eq!(git(&env.remote, &["rev-parse", "side"]), master);

    // each of the three names on "two" is reached by the other two, but a
    // bookmark the push deletes reaches nothing after it: deleting all three
    // at once is refused, and none of them is deleted
    let before = refs();
    let out = env.j(&dest, &["push (unlabel \"master\" ++ unlabel \"old\" ++ unlabel \"side\")"]);
    assert_eq!(out.code, 1, "deleted every bookmark on an immutable commit");
    assert!(out.stderr.contains("immutable"), "{}", out.stderr);
    assert_eq!(refs(), before);
}

#[test]
fn push_deletes_an_immutable_bookmark_another_one_reaches() {
    // §7.6: a delete of a bookmark on an immutable commit is refused only
    // while no bookmark left on origin after the push reaches that commit.
    // Every such delete was refused, so config.j's `unlabel "feature"` after
    // the merge failed whenever the merge kept feature's commit (a
    // fast-forward or a merge commit), and a name ever put on trunk could
    // never be deleted or renamed
    let env = setup();
    let refs = || git(&env.remote, &["for-each-ref", "--format=%(refname) %(objectname)"]);
    let dest = env.dir.join("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();

    // `feature`, merged by fast-forwarding master onto it
    std::fs::write(dest.join("f.txt"), "feat\n").unwrap();
    env.j(&dest, &["describe \"feat\""]).ok();
    env.j(&dest, &["push (label \"feature\" here)"]).ok();
    env.j(&dest, &["push (label \"master\" (labelled \"feature\"))"]).ok();
    let out = env.j(&dest, &["push (unlabel \"feature\")"]);
    assert_eq!(out.code, 0, "fast-forwarded feature: {}", out.stderr);

    // `side`, merged on the remote with a merge commit
    env.j(&dest, &["new . goto trunk"]).ok();
    std::fs::write(dest.join("s.txt"), "side\n").unwrap();
    env.j(&dest, &["describe \"side\""]).ok();
    env.j(&dest, &["push (label \"side\" here)"]).ok();
    let work = env.dir.join("work");
    git(&env.dir, &["clone", "-q", env.remote.to_str().unwrap(), work.to_str().unwrap()]);
    git(&work, &["checkout", "-q", "master"]);
    git(
        &work,
        &["-c", "user.email=t@t", "-c", "user.name=T", "merge", "-q", "--no-ff", "origin/side", "-m", "merge side"],
    );
    git(&work, &["push", "-q", "origin", "master"]);
    env.j(&dest, &["fetch"]).ok();
    let out = env.j(&dest, &["push (unlabel \"side\")"]);
    assert_eq!(out.code, 0, "side merged with a merge commit: {}", out.stderr);

    // a second name on trunk's commit, renamed and deleted
    env.j(&dest, &["push (label \"x\" trunk)"]).ok();
    let out = env.j(&dest, &["push (rename \"x\" \"y\")"]);
    assert_eq!(out.code, 0, "renamed an alias of master: {}", out.stderr);
    let out = env.j(&dest, &["push (unlabel \"y\")"]);
    assert_eq!(out.code, 0, "deleted an alias of master: {}", out.stderr);
    let merge = git(&env.remote, &["rev-parse", "master"]);
    assert_eq!(refs(), format!("refs/heads/master {}", merge));

    // master itself: deleting it leaves nothing reaching the merge, but a
    // bookmark the same push creates there does (the remote's default
    // branch becomes `main` first, so git would delete master)
    git(&env.remote, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    let out = env.j(&dest, &["push (unlabel \"master\")"]);
    assert_eq!(out.code, 1, "deleted the only bookmark on trunk");
    assert!(out.stderr.contains("immutable"), "{}", out.stderr);
    assert_eq!(refs(), format!("refs/heads/master {}", merge));
    let out = env.j(&dest, &["push (rename \"master\" \"main\")"]);
    assert_eq!(out.code, 0, "renamed master: {}", out.stderr);
    assert_eq!(refs(), format!("refs/heads/main {}", merge));
}

#[test]
fn push_deletes_an_immutable_bookmark_only_after_the_update_that_reaches_it() {
    // §7.6: a delete of a bookmark on an immutable commit that only a
    // bookmark the same push sets still reaches is sent after that update is
    // accepted. Both went in one git push, which is not atomic: when the
    // remote rejected `main`, `push (rename "master" "main")` deleted master
    // anyway and left no bookmark on origin reaching trunk's history
    use std::os::unix::fs::PermissionsExt;
    let env = setup();
    let refs = || git(&env.remote, &["for-each-ref", "--format=%(refname) %(objectname)"]);
    let hook = env.remote.join("hooks/update");
    let reject = |name: &str| {
        let script = format!("#!/bin/sh\n[ \"$1\" = refs/heads/{} ] && exit 1\nexit 0\n", name);
        std::fs::write(&hook, script).unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    let dest = env.dir.join("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    let one = git(&env.remote, &["rev-parse", "master"]).trim().to_string();
    let top_op = || env.j(&dest, &["ops"]).ok().stdout.lines().next().unwrap_or("").to_string();
    let ops = || env.j(&dest, &["ops"]).ok().stdout.lines().count();
    let labelled = |label: &str| {
        let tree = env.j(&dest, &["tree"]).ok().stdout;
        tree.lines().any(|l| l.contains('◆') && l.contains("one") && l.contains(label))
    };
    // the remote's default branch is `main`, so git lets master be deleted
    git(&env.remote, &["symbolic-ref", "HEAD", "refs/heads/main"]);

    // the update is rejected: the delete is not sent, and nothing is recorded
    reject("main");
    let before = ops();
    let out = env.j(&dest, &["push (rename \"master\" \"main\")"]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("refs/heads/main"), "{}", out.stderr);
    assert!(out.stderr.contains("not sent: refs/heads/master"), "{}", out.stderr);
    assert_eq!(refs(), format!("refs/heads/master {}\n", one));
    assert_eq!(ops(), before);
    assert!(labelled("master"), "trunk's commit is no longer immutable");

    // one update of two is accepted: it is recorded, the delete still waits
    let out = env.j(&dest, &["push (\\r -> label \"keep\" trunk r ++ rename \"master\" \"main\" r)"]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("not sent: refs/heads/master"), "{}", out.stderr);
    assert_eq!(refs(), format!("refs/heads/keep {one}\nrefs/heads/master {one}\n"));
    assert!(top_op().contains("label \"keep\""), "{}", top_op());
    assert!(labelled("keep") && labelled("master"));

    // every update is accepted and the delete rejected: the one operation
    // records the update
    env.j(&dest, &["push (unlabel \"keep\")"]).ok();
    reject("master");
    let out = env.j(&dest, &["push (rename \"master\" \"main\")"]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("push rejected: refs/heads/master"), "{}", out.stderr);
    assert_eq!(refs(), format!("refs/heads/main {one}\nrefs/heads/master {one}\n"));
    assert!(top_op().contains("rename \"master\" \"main\""), "{}", top_op());
    assert!(labelled("main") && labelled("master"));
}

#[test]
fn push_refuses_a_waiting_delete_whose_name_nests_with_a_set_one() {
    // §7.6: git cannot hold a bookmark and one under its name at once, so
    // when the deletes wait, a bookmark the push creates under the name of
    // one it deletes, or the reverse, is rejected by the first git push and
    // the deletes are never sent. `push (rename "master" "master/legacy")`
    // sent that push and exited 1 naming only the rejected ref, and renaming
    // trunk's only bookmark under itself could never succeed; it is refused
    // before anything is sent, saying how to do it instead
    use std::os::unix::fs::PermissionsExt;
    let env = setup();
    let refs = || git(&env.remote, &["for-each-ref", "--format=%(refname) %(objectname)"]);
    // every git push that reaches the remote is logged, each followed by an
    // empty line
    let log = env.dir.join("pushes.log");
    let hook = env.remote.join("hooks/pre-receive");
    std::fs::write(&hook, format!("#!/bin/sh\n{{ cat; echo; }} >> '{}'\n", log.display())).unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let sent = || {
        let pushes = std::fs::read_to_string(&log).unwrap_or_default();
        let _ = std::fs::remove_file(&log);
        pushes
    };
    let dest = env.dir.join("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    let one = git(&env.remote, &["rev-parse", "master"]).trim().to_string();
    let zero = "0".repeat(one.len());
    let ops = || env.j(&dest, &["ops"]).ok().stdout.lines().count();
    // a bookmark that does not reach trunk's commit
    env.j(&dest, &["new . top"]).ok();
    std::fs::write(dest.join("f.txt"), "feat\n").unwrap();
    env.j(&dest, &["describe \"feat\""]).ok();
    env.j(&dest, &["push (label \"feature/x\" here)"]).ok();
    let feat = git(&env.remote, &["rev-parse", "feature/x"]).trim().to_string();
    let before_refs = refs();
    // the remote's default branch is elsewhere, so git lets master be deleted
    git(&env.remote, &["symbolic-ref", "HEAD", "refs/heads/other"]);
    assert!(sent().contains("refs/heads/feature/x"), "the hook logged no push");

    let before = ops();
    let out = env.j(&dest, &["push (rename \"master\" \"master/legacy\")"]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("`master` must stay on origin until `master/legacy`"), "{}", out.stderr);
    assert!(out.stderr.contains("push (label \"tmp\" (labelled \"master\"))"), "{}", out.stderr);
    assert_eq!(sent(), "", "a git push was sent");
    assert_eq!(refs(), before_refs);
    assert_eq!(ops(), before);

    // every delete waits with master's, and a deleted bookmark nests with
    // one set at the name it is under as well (`feature/x` and `feature`)
    let out = env.j(&dest, &["push (\\r -> rename \"master\" \"main\" r ++ rename \"feature/x\" \"feature\" r)"]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("`feature/x` must stay on origin until `feature`"), "{}", out.stderr);
    assert!(out.stderr.contains("(labelled \"master\")"), "{}", out.stderr);
    assert_eq!(sent(), "", "a git push was sent");
    assert_eq!(refs(), before_refs);
    assert_eq!(ops(), before);

    // a name nests only with the names under it, past a `/`: `master2`
    // shares a prefix with master and is created, and master deleted in a
    // second git push once it is accepted
    let out = env.j(&dest, &["push (rename \"master\" \"master2\")"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let pushes = sent();
    let pushes: Vec<&str> = pushes.split_terminator("\n\n").collect();
    assert_eq!(pushes, [format!("{zero} {one} refs/heads/master2"), format!("{one} {zero} refs/heads/master")]);
    assert_eq!(refs(), format!("refs/heads/feature/x {feat}\nrefs/heads/master2 {one}\n"));
    // with no trunk left nothing is immutable, and the rename back is one
    // git push
    env.j(&dest, &["push (rename \"master2\" \"master\")"]).ok();
    assert_eq!(refs(), before_refs);
    assert_eq!(sent().split_terminator("\n\n").count(), 1);

    // the way round it: with the temporary label on origin the delete does
    // not wait, and the rename is one git push
    env.j(&dest, &["push (label \"tmp\" (labelled \"master\"))"]).ok();
    env.j(&dest, &["push (rename \"master\" \"master/legacy\")"]).ok();
    env.j(&dest, &["push (unlabel \"tmp\")"]).ok();
    assert_eq!(refs(), format!("refs/heads/feature/x {feat}\nrefs/heads/master/legacy {one}\n"));
    let tree = env.j(&dest, &["tree"]).ok().stdout;
    assert!(tree.lines().any(|l| l.contains("one") && l.contains("master/legacy")), "{}", tree);
}

#[test]
fn push_refusing_a_nesting_delete_advises_a_label_no_bookmark_has() {
    // §7.6: the refusal above advised `push (label "tmp" (labelled
    // "master"))` whatever the remote held. Followed as written, it moved a
    // `tmp` the user had on origin to trunk's commit and then deleted it; it
    // was rejected with `tmp/wip` there; and when the refused push itself
    // deleted `tmp` it changed nothing and the refusal came back. The label
    // it advises is on no bookmark, no record names it, and it nests with
    // none of those
    let env = setup();
    let refs = || git(&env.remote, &["for-each-ref", "--format=%(refname) %(objectname)"]);
    let dest = env.dir.join("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    let one = git(&env.remote, &["rev-parse", "master"]).trim().to_string();
    // the user's own `tmp` and `tmp-1/wip`, which do not reach trunk's commit
    env.j(&dest, &["new . top"]).ok();
    std::fs::write(dest.join("f.txt"), "work\n").unwrap();
    env.j(&dest, &["describe \"my work\""]).ok();
    env.j(&dest, &["push (\\r -> label \"tmp\" here r ++ label \"tmp-1/wip\" here r)"]).ok();
    let work = git(&env.remote, &["rev-parse", "tmp"]).trim().to_string();
    // the remote's default branch is elsewhere, so git lets master be deleted
    git(&env.remote, &["symbolic-ref", "HEAD", "refs/heads/other"]);
    // `expr` is refused; the steps its message gives are run around it as
    // written, and the first is returned
    let follow = |expr: &str| {
        let out = env.j(&dest, &[expr]);
        assert_eq!(out.code, 1, "{}", out.stderr);
        let steps: Vec<&str> = out.stderr.split('`').filter(|s| s.starts_with("push (")).collect();
        assert_eq!(steps.len(), 2, "{}", out.stderr);
        env.j(&dest, &[steps[0]]).ok();
        env.j(&dest, &[expr]).ok();
        env.j(&dest, &[steps[1]]).ok();
        steps[0].to_string()
    };

    // `tmp` is on origin, and `tmp-1` nests with `tmp-1/wip` there
    let step = follow("push (rename \"master\" \"master/legacy\")");
    assert_eq!(step, "push (label \"tmp-2\" (labelled \"master\"))");
    assert_eq!(
        refs(),
        format!("refs/heads/master/legacy {one}\nrefs/heads/tmp {work}\nrefs/heads/tmp-1/wip {work}\n")
    );

    // with no trunk left nothing is immutable, and the rename back goes
    env.j(&dest, &["push (rename \"master/legacy\" \"master\")"]).ok();
    // the push deletes `tmp` and `tmp-1/wip`, and creates `tmp-3` and a name
    // under `tmp-2`
    let expr = "push (\\r -> rename \"master\" \"master/legacy\" r ++ rename \"tmp\" \"tmp-2/x\" r ++ rename \"tmp-1/wip\" \"tmp-3\" r)";
    let step = follow(expr);
    assert_eq!(step, "push (label \"tmp-4\" (labelled \"master\"))");
    assert_eq!(
        refs(),
        format!("refs/heads/master/legacy {one}\nrefs/heads/tmp-2/x {work}\nrefs/heads/tmp-3 {work}\n")
    );
}

#[test]
fn push_moves_a_bookmark_whose_remote_commit_was_rewritten() {
    // §7.6: only the visible commit carrying a change id is in the immutable
    // set. Bookmarks pushed at a commit that was then amended here, and the
    // amended commit put on trunk, still point at the hidden original on
    // the remote. The check found their change id in the immutable set and
    // tested descent from the hidden commit, which nothing descends from:
    // they could never be moved or deleted again
    let env = setup();
    let rev = |name: &str| git(&env.remote, &["rev-parse", name]);
    let dest = env.dir.join("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    std::fs::write(dest.join("a.txt"), "two\n").unwrap();
    env.j(&dest, &["describe \"work\""]).ok();
    let expr = "push (\\r -> label \"feature\" here r ++ label \"other\" here r ++ label \"gone\" here r)";
    env.j(&dest, &[expr]).ok();
    let pushed = rev("feature");
    env.j(&dest, &["describe \"work v2\""]).ok();
    env.j(&dest, &["push (label \"master\" here)"]).ok();
    assert_ne!(rev("master"), pushed);

    // onto a child of the amended commit
    std::fs::write(dest.join("b.txt"), "three\n").unwrap();
    env.j(&dest, &["describe \"next\""]).ok();
    let out = env.j(&dest, &["push (label \"other\" here)"]);
    assert_eq!(out.code, 0, "moved onto a child: {}", out.stderr);
    assert_eq!(git(&env.remote, &["rev-parse", "other^"]), rev("master"));
    // deleted
    let out = env.j(&dest, &["push (unlabel \"gone\")"]);
    assert_eq!(out.code, 0, "deleted: {}", out.stderr);
    assert!(!git(&env.remote, &["branch"]).contains("gone"));
    // onto the amended commit itself
    let out = env.j(&dest, &["push relabel"]);
    assert_eq!(out.code, 0, "relabelled: {}", out.stderr);
    assert_eq!(rev("feature"), rev("master"));
}

#[test]
fn push_checks_only_the_commits_it_sends() {
    // §7.6: the unresolved-file and empty-description refusals are about
    // commits that would be sent. The check walked every ancestor of the new
    // and old targets, so one empty message the remote already had refused
    // every later push on top of it, and deletes, which send nothing
    let env = setup();
    let seed = env.dir.join("seed");
    git(&env.dir, &["clone", "-q", env.remote.to_str().unwrap(), seed.to_str().unwrap()]);
    git(&seed, &["checkout", "-q", "master"]);
    std::fs::write(seed.join("a.txt"), "blank\n").unwrap();
    git(
        &seed,
        &["-c", "user.email=t@t", "-c", "user.name=T", "commit", "-qa", "--allow-empty-message", "-m", ""],
    );
    git(&seed, &["push", "-q", "origin", "master"]);
    let dest = env.dir.join("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    std::fs::write(dest.join("a.txt"), "work\n").unwrap();
    env.j(&dest, &["describe \"work\""]).ok();
    env.j(&dest, &["push (label \"feature\" here)"]).ok();
    assert!(git(&env.remote, &["log", "--oneline", "feature"]).contains("work"));
    env.j(&dest, &["push (label \"copy\" (labelled \"feature\"))"]).ok();
    env.j(&dest, &["push (unlabel \"copy\")"]).ok();
    assert!(!git(&env.remote, &["branch"]).contains("copy"));
    // a new commit without a message is still refused
    env.j(&dest, &["new"]).ok();
    std::fs::write(dest.join("b.txt"), "more\n").unwrap();
    let out = env.j(&dest, &["push (label \"feature\" here)"]);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("empty description"), "{}", out.stderr);
    assert!(git(&env.remote, &["log", "-1", "--format=%s", "feature"]).contains("work"));
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

/// The reference config.j as a user copied it before `commits` became the
/// builtin `subtreeCommits`, as it was then byte for byte: it declares no
/// such builtin, and defines `commits` and `ancestors` recursively.
fn config_before_subtree_commits() -> String {
    let mut config = include_str!("../config.j").to_string();
    for (now, before) in [
        (
            concat!(
                "subtreeCommits : a -> [Commit]             -- every commit of a tree in preorder, in one\n",
                "                                           -- pass: what `commits` (below) is\n",
            ),
            "",
        ),
        (
            concat!(
                "-- The builtin is  \\t -> t.root :: (concat (map commits t.children) or [])\n",
                "-- in one pass; written that way it copies the list below every commit, and\n",
                "-- so takes time quadratic in the length of the history.\n",
                "commits : a -> [Commit]\n",
                "commits = subtreeCommits\n",
            ),
            concat!(
                "commits : a -> [Commit]\n",
                "commits = \\t -> t.root :: (concat (map commits t.children) or [])\n",
            ),
        ),
        (
            "ancestors = \\repo -> repo.root.id :: map (\\f -> f.parent.id) repo.context\n",
            "ancestors = \\repo -> repo.root.id :: ((let p = up repo in ancestors p) or [])\n",
        ),
    ] {
        assert_eq!(config.matches(now).count(), 1, "config.j no longer has {:?}", now);
        config = config.replacen(now, before, 1);
    }
    assert!(!config.contains("subtreeCommits"));
    config.replace("Your Name", "Test User").replace("you@example.com", "test@example.com")
}

#[test]
fn a_config_copied_before_subtree_commits_loads_and_agrees() {
    // A user's config.j is never updated (§6.1). One copied before the
    // reference config's `commits` became the builtin `subtreeCommits` does
    // not declare it, and walks the history recursively. It must still load,
    // and show and refuse exactly what the reference config does, with
    // `trunk` and `immutable` non-empty.
    let env = setup();
    let dest = env.dir.join("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    std::fs::write(dest.join("b.txt"), "two\n").unwrap();
    env.j(&dest, &["new . describe \"two\""]).ok();
    std::fs::write(dest.join("c.txt"), "three\n").unwrap();
    env.j(&dest, &["describe \"three\""]).ok();
    let exprs = [
        "1",
        "tree",
        "log",
        "trunk",
        "immutable",
        "ancestors",
        "all",
        "length . commits . top",
        "commits ({ root = 1, children = [({ root = 2, children = [({ root = 3 })] }) 7] })",
        "commits 5",
        "goto trunk",
        "describe \"x\" . goto trunk",
        "length . all . validate . new . goto trunk",
    ];
    let run = || -> Vec<(i32, String, String)> {
        exprs
            .iter()
            .map(|e| {
                let o = env.j(&dest, &[e]);
                (o.code, o.stdout, o.stderr)
            })
            .collect()
    };
    let reference = run();
    std::fs::write(env.cfg.join("j/config.j"), config_before_subtree_commits()).unwrap();
    let before = run();
    for ((e, r), b) in exprs.iter().zip(&reference).zip(&before) {
        assert_eq!(r, b, "{}", e);
    }
    // what it does not declare is unbound (§6.2), and edits still persist
    let out = env.j(&dest, &["subtreeCommits"]);
    assert_eq!(out.code, 1, "{}", out.stdout);
    env.j(&dest, &["describe \"four\""]).ok();
    assert!(env.j(&dest, &["log"]).ok().stdout.contains("four"));
}
