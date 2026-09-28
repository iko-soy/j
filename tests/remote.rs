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
        self.j_env(workdir, args, &[])
    }

    /// `j` with `vars` added to its environment
    fn j_env(&self, workdir: &PathBuf, args: &[&str], vars: &[(&str, &std::path::Path)]) -> Out {
        let out = Command::new(j_bin())
            .args(args)
            .current_dir(workdir)
            .env("XDG_CONFIG_HOME", &self.cfg)
            .env("NO_COLOR", "1")
            .envs(vars.iter().copied())
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
    // clone removes the DIR it created and each missing parent it created
    // that is then empty, and nothing else.
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
    // A parent it created keeps what something else put there while the
    // clone ran: removing each one whole deleted a sibling clone, made in
    // parallel into the same new `vendor/`, when this one failed. Here the
    // `git` that fetches writes into the parent, then fails
    use std::os::unix::fs::PermissionsExt;
    let wrapper = env.dir.join("git-fetch-fails");
    for (keep, left) in [
        ("a/keep.txt", &["a", "a/keep.txt"][..]),
        ("a/b/keep.txt", &["a", "a/b", "a/b/keep.txt"][..]),
    ] {
        let script = format!(
            "#!/bin/sh\ncase \" $* \" in *\" fetch \"*) echo k > '{}'; exit 1;; esac\nexec git \"$@\"\n",
            work.join(keep).display()
        );
        std::fs::write(&wrapper, script).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let out = Command::new(j_bin())
            .args(["clone", env.remote.to_str().unwrap(), "a/b/c"])
            .current_dir(&work)
            .env("XDG_CONFIG_HOME", &env.cfg)
            .env("NO_COLOR", "1")
            .env("J_GIT", &wrapper)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{}", stderr);
        assert!(stderr.contains("fetch failed"), "{}", stderr);
        let mut expected = before.clone();
        expected.extend(left.iter().map(|p| p.to_string()));
        expected.sort();
        assert_eq!(listing(&work), expected, "{} written during the clone", keep);
        assert_eq!(std::fs::read_to_string(work.join(keep)).unwrap(), "k\n");
        std::fs::remove_dir_all(work.join("a")).unwrap();
    }
    env.j(&work, &["clone", env.remote.to_str().unwrap(), "new/../fresh"]).ok();
    assert_eq!(std::fs::read_to_string(work.join("fresh/a.txt")).unwrap(), "one\n");
}

#[test]
fn failed_clone_does_not_follow_a_symlink_put_in_place_of_its_directories() {
    // §7.8: a failed clone removed DIR with `remove_dir_all` and emptied a
    // DIR it had found empty through `read_dir`, each by its path. A symlink
    // put in place of a parent it created, or of the DIR it found empty,
    // while the clone ran had that cleanup follow it and delete what the
    // symlink names: `outside/c` and `outside/keep.txt` here. One put in
    // place of the DIR it created was removed. A path that no longer holds
    // the directory the clone created or found is now left, and the error
    // names it.
    use std::os::unix::fs::PermissionsExt;
    let env = setup();
    let work = env.dir.join("work");
    let outside = env.dir.join("outside");
    let moved = env.dir.join("moved");
    let wrapper = env.dir.join("git-fetch-swaps");
    for (dest, swap, named) in [
        // a parent it created, through which DIR now leads elsewhere too
        ("a/b/c", "a/b", "left `a/b/c` and `a/b`, as they are no longer the directories this clone created"),
        // the DIR it created
        ("a/b/c", "a/b/c", "left `a/b/c`, as it is no longer the directory this clone created"),
        // the DIR it found empty
        ("e", "e", "left what `e` holds, as it is no longer the directory this clone found empty"),
    ] {
        for d in [&work, &outside, &moved] {
            let _ = std::fs::remove_dir_all(d);
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::create_dir(outside.join("c")).unwrap();
        std::fs::write(outside.join("keep.txt"), "k\n").unwrap();
        std::fs::write(outside.join("c/keep.txt"), "k\n").unwrap();
        if dest == "e" {
            std::fs::create_dir(work.join("e")).unwrap();
        }
        // the `git` that fetches puts the symlink in place, then fails
        let script = format!(
            "#!/bin/sh\ncase \" $* \" in *\" fetch \"*) mv '{}' '{}' && ln -s '{}' '{}'; exit 1;; esac\nexec git \"$@\"\n",
            work.join(swap).display(),
            moved.join("was").display(),
            outside.display(),
            work.join(swap).display(),
        );
        std::fs::write(&wrapper, script).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let out = Command::new(j_bin())
            .args(["clone", env.remote.to_str().unwrap(), dest])
            .current_dir(&work)
            .env("XDG_CONFIG_HOME", &env.cfg)
            .env("NO_COLOR", "1")
            .env("J_GIT", &wrapper)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{}", stderr);
        assert!(stderr.contains("fetch failed"), "{}", stderr);
        assert!(stderr.contains(named), "{} in place of {}: {}", swap, dest, stderr);
        assert_eq!(std::fs::read_to_string(outside.join("keep.txt")).unwrap(), "k\n", "{}", swap);
        assert_eq!(std::fs::read_to_string(outside.join("c/keep.txt")).unwrap(), "k\n", "{}", swap);
        assert!(work.join(swap).symlink_metadata().unwrap().is_symlink(), "{}", swap);
        // what the clone had made is where the symlink moved it to
        assert!(moved.join("was").is_dir(), "{}", swap);
    }
    std::fs::remove_dir_all(&work).unwrap();
}

/// Whether the signal `sig` sent to the process `pid` as a whole is still
/// waiting for one of its threads to take it
#[cfg(target_os = "linux")]
fn signal_pending(pid: libc::pid_t, sig: libc::c_int) -> bool {
    let status = std::fs::read_to_string(format!("/proc/{}/status", pid)).unwrap_or_default();
    status
        .lines()
        .find_map(|l| l.strip_prefix("ShdPnd:"))
        .is_some_and(|mask| u64::from_str_radix(mask.trim(), 16).unwrap() & (1 << (sig - 1)) != 0)
}

#[test]
#[cfg(target_os = "linux")]
fn an_interrupted_clone_leaves_nothing_behind() {
    // §1.3, §7.8: a clone removed what it made only when it failed. Ctrl-C
    // (SIGINT), SIGTERM, SIGHUP or SIGQUIT while it fetched killed it at
    // once, leaving DIR holding `.git` and `.jj`, with no remote and no
    // working-copy commit, and a retry was refused: "already contains a jj
    // repository". A signal sent to `j` alone also left its `git fetch`
    // writing there. Such a signal now waits for the step in progress, the
    // git it runs included, then fails the clone, which removes what it
    // made, and then takes effect. One the clone was started ignoring, as
    // `nohup` starts it, is still ignored, by the git it runs too.
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    let env = setup();
    let work = env.dir.join("work");
    std::fs::create_dir_all(work.join("e")).unwrap();
    let (started, go) = (env.dir.join("started"), env.dir.join("go"));
    let wrapper = env.dir.join("git-waits");
    let names = |dir: &PathBuf| {
        let mut all: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        all.sort();
        all
    };
    let remote = env.remote.to_str().unwrap();
    // `j clone remote dest`, whose `git` running `step` says it has started
    // and waits to be let go, sent `sig` meanwhile, to its process group
    // (as a Ctrl-C at the terminal is) when `group`, else to it alone; it
    // starts ignoring `sig` when `ignored`
    let clone = |step: &str, sig: libc::c_int, group: bool, ignored: bool, dest: &str| {
        let _ = std::fs::remove_file(&started);
        let _ = std::fs::remove_file(&go);
        let script = format!(
            "#!/bin/sh\ncase \" $* \" in *\" {} \"*) : > '{}'; while [ ! -e '{}' ]; do sleep 0.01; done;; esac\nexec git \"$@\"\n",
            step,
            started.display(),
            go.display()
        );
        std::fs::write(&wrapper, script).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut command = Command::new(j_bin());
        command
            .args(["clone", remote, dest])
            .current_dir(&work)
            .env("XDG_CONFIG_HOME", &env.cfg)
            .env("NO_COLOR", "1")
            .env("J_GIT", &wrapper)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .process_group(0);
        // an ignored signal stays ignored across exec; SIGQUIT dumps no core
        unsafe {
            command.pre_exec(move || {
                if ignored {
                    libc::signal(sig, libc::SIG_IGN);
                }
                libc::setrlimit(libc::RLIMIT_CORE, &libc::rlimit { rlim_cur: 0, rlim_max: 0 });
                Ok(())
            });
        }
        let mut child = command.spawn().unwrap();
        let pid = child.id() as libc::pid_t;
        while !started.exists() {
            assert!(child.try_wait().unwrap().is_none(), "ended before {} started", step);
            std::thread::yield_now();
        }
        assert_eq!(unsafe { libc::kill(if group { -pid } else { pid }, sig) }, 0);
        // `j` takes the signal before its git can go on
        while signal_pending(pid, sig) && child.try_wait().unwrap().is_none() {
            std::thread::yield_now();
        }
        std::fs::write(&go, "").unwrap();
        let out = child.wait_with_output().unwrap();
        (out.status, String::from_utf8_lossy(&out.stderr).to_string())
    };
    for (step, sig, group, dest) in [
        ("fetch", libc::SIGINT, true, "a/b/c"),
        ("fetch", libc::SIGTERM, false, "e"),
        ("fetch", libc::SIGQUIT, false, "c"),
        ("ls-remote", libc::SIGHUP, false, "c"),
    ] {
        let (status, stderr) = clone(step, sig, group, false, dest);
        // the DIR it created is gone, its parents too; the one it found
        // empty is empty
        assert_eq!(names(&work), ["e"], "{} during {} into {}", sig, step, dest);
        assert_eq!(names(&work.join("e")), [] as [String; 0], "{} during {}", sig, step);
        assert_eq!(status.signal(), Some(sig), "{} during {}: {}", sig, step, stderr);
        // a Ctrl-C stops the git too (here the shell that runs it), and `j`
        // may take it only once the fetch has failed
        assert!(
            stderr.contains("clone interrupted") || group && stderr.contains("fetch failed: git was stopped"),
            "{}",
            stderr
        );
        // and a retry clones there
        env.j(&work, &["clone", remote, dest]).ok();
        assert_eq!(std::fs::read_to_string(work.join(dest).join("a.txt")).unwrap(), "one\n");
        std::fs::remove_dir_all(work.join(dest.split('/').next().unwrap())).unwrap();
        std::fs::create_dir_all(work.join("e")).unwrap();
    }
    let (status, stderr) = clone("fetch", libc::SIGHUP, true, true, "c");
    assert!(status.success(), "{}", stderr);
    assert_eq!(std::fs::read_to_string(work.join("c/a.txt")).unwrap(), "one\n");
    env.j(&work.join("c"), &["fetch"]).ok();
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

/// A clone `w` of `env`'s remote, whose master holds `one`, then `s` and
/// `t`, each adding the file of its name, and `seed`, a git clone a
/// collaborator pushes from; the clone's working-copy commit is an empty
/// child of `t`
fn clone_above_s_and_t(env: &Env) -> (PathBuf, PathBuf) {
    let seed = env.dir.join("seed");
    git(&env.dir, &["clone", "-q", env.remote.to_str().unwrap(), seed.to_str().unwrap()]);
    for name in ["s", "t"] {
        std::fs::write(seed.join(name), format!("{}\n", name)).unwrap();
        git(&seed, &["add", name]);
        git(&seed, &["-c", "user.email=t@t", "-c", "user.name=T", "commit", "-qm", name]);
    }
    git(&seed, &["push", "-q", "origin", "master"]);
    let w = env.dir.join("w");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), w.to_str().unwrap()]).ok();
    (w, seed)
}

/// Have the collaborator in `seed` replace `t` on master by `u`, as a
/// force-push does: the next fetch abandons `t`
fn force_push_dropping_t(seed: &PathBuf) {
    git(seed, &["reset", "-q", "--hard", "HEAD~1"]);
    std::fs::write(seed.join("u"), "u\n").unwrap();
    git(seed, &["add", "u"]);
    git(seed, &["-c", "user.email=t@t", "-c", "user.name=T", "commit", "-qm", "u"]);
    git(seed, &["push", "-qf", "origin", "master"]);
}

const PARENT_MESSAGE: &str = "\\r -> (focus (prev r)).message";
const FOCUS_PATHS: &str = "\\r -> show (map (\\e -> e.path) (files r))";

#[test]
fn a_fetch_that_abandons_the_working_copys_parent_records_later_edits_onto_it() {
    // §7.4, §7.6: fetch never touches the working directory, so when its
    // import abandons the commit the working-copy commit is built on (a
    // force-push, a deleted branch, a squash-merge upstream), the
    // working-copy commit is rebased and the working copy left stale.
    // Every snapshotting run, printing ones too, then exited 2 once the
    // directory held an edit, blaming jj --ignore-working-copy or another
    // workspace, and `undo` exited 1: only restoring the directory by hand
    // got the repository going again. The directory's changes since the
    // last checkout are now recorded onto the rebased commit.
    let env = setup();
    let (w, seed) = clone_above_s_and_t(&env);
    std::fs::write(w.join("mine"), "mine\n").unwrap();
    std::fs::write(w.join("s"), "s\nedited\n").unwrap();
    force_push_dropping_t(&seed);
    env.j(&w, &["fetch"]).ok();
    assert_eq!(std::fs::read_to_string(w.join("t")).unwrap(), "t\n");
    // a printing run shows the edits on the rebased commit, `t` gone
    assert_eq!(env.j(&w, &[PARENT_MESSAGE]).ok().stdout.trim(), "s");
    let paths = env.j(&w, &[FOCUS_PATHS]).ok().stdout;
    assert_eq!(paths.trim(), r#"[["a.txt"] ["mine"] ["s"]]"#);
    // `undo` refuses to write over edits no commit holds, as ever
    let out = env.j(&w, &["undo"]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("changes not in @"), "{}", out.stderr);
    // `j id` records them, and writes the rebased commit's files
    env.j(&w, &["id"]).ok();
    assert!(!w.join("t").exists());
    assert_eq!(std::fs::read_to_string(w.join("mine")).unwrap(), "mine\n");
    assert_eq!(std::fs::read_to_string(w.join("s")).unwrap(), "s\nedited\n");
    let texts = env.j(&w, &["\\r -> show (map (\\e -> [e.path (text e.content)]) (files r))"]).ok().stdout;
    assert_eq!(
        texts.split_whitespace().collect::<Vec<_>>().join(" "),
        r#"[[["a.txt"] "one\n"] [["mine"] "mine\n"] [["s"] "s\nedited\n"]]"#
    );
    assert_eq!(env.j(&w, &[PARENT_MESSAGE]).ok().stdout.trim(), "s");
    // the directory holds the focus now, so there is nothing more to record
    let ops = env.j(&w, &["ops"]).ok().stdout.lines().count();
    env.j(&w, &["id"]).ok();
    assert_eq!(env.j(&w, &["ops"]).ok().stdout.lines().count(), ops);
}

#[test]
fn a_fetch_that_abandons_the_working_copys_parent_is_undone_and_redone() {
    // §7.6, §7.7: after such a fetch, a directory with no edit in it was
    // refused by `undo` and `redo` too, as holding changes not in @
    let env = setup();
    let (w, seed) = clone_above_s_and_t(&env);
    force_push_dropping_t(&seed);
    env.j(&w, &["fetch"]).ok();
    env.j(&w, &["undo"]).ok();
    assert_eq!(env.j(&w, &[PARENT_MESSAGE]).ok().stdout.trim(), "t");
    assert_eq!(std::fs::read_to_string(w.join("t")).unwrap(), "t\n");
    let ops = env.j(&w, &["ops"]).ok().stdout.lines().count();
    env.j(&w, &["id"]).ok();
    assert_eq!(env.j(&w, &["ops"]).ok().stdout.lines().count(), ops);
    // redoing the fetch checks the rebased commit out, without `t`
    env.j(&w, &["redo"]).ok();
    assert_eq!(env.j(&w, &[PARENT_MESSAGE]).ok().stdout.trim(), "s");
    assert!(!w.join("t").exists());
    env.j(&w, &["id"]).ok();
    assert_eq!(env.j(&w, &["ops"]).ok().stdout.lines().count(), ops + 1);
}

#[test]
fn an_edit_to_a_file_a_fetch_abandoned_is_kept_as_a_conflict() {
    // §7.4, §7.6: the working-copy commit is pushed as `wip`, and the
    // branch deleted upstream: the fetch abandons the commit itself, and a
    // new empty one takes its place on `one`. An edit made since to the
    // file `wip` held is not lost: the commit gets it as a conflict between
    // its deletion and the edit.
    let env = setup();
    let w = env.dir.join("w");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), w.to_str().unwrap()]).ok();
    std::fs::write(w.join("mine"), "mine\n").unwrap();
    env.j(&w, &["describe \"wip\""]).ok();
    env.j(&w, &["push (label \"wip\" here)"]).ok();
    git(&env.dir, &["--git-dir", env.remote.to_str().unwrap(), "branch", "-D", "wip"]);
    env.j(&w, &["fetch"]).ok();
    std::fs::write(w.join("mine"), "mine\nand more\n").unwrap();
    assert_eq!(env.j(&w, &[PARENT_MESSAGE]).ok().stdout.trim(), "one");
    env.j(&w, &["id"]).ok();
    let conflicted = env.j(&w, &["\\r -> show (conflicted (files r))"]).ok().stdout;
    assert_eq!(conflicted.trim(), r#"[["mine"]]"#);
    let text = std::fs::read_to_string(w.join("mine")).unwrap();
    assert!(text.contains("and more"), "{}", text);
}

#[test]
fn a_conflict_a_fetch_makes_with_a_directory_side_is_kept_beside_other_edits() {
    // §7.4, §7.6: `t` replaced the directory `p` of `dir` by a file, which
    // the working-copy commit edits; the fetch abandons `t` and rebases the
    // commit onto `dir`, with a conflict at `p` between the edit and
    // `dir`'s directory, and leaves the working copy stale. The snapshot
    // took what the directory held at every conflict of its merge that set
    // a directory against a file, the commit's own included, where the
    // directory held what the last checkout left: with any other edit
    // there to record, the conflict was resolved to the local file and
    // `p/x` dropped from the commit, and a push would have deleted it
    // upstream. Only a path the directory changed takes what it holds.
    for edit in [false, true] {
        let env = setup();
        let seed = env.dir.join("seed");
        git(&env.dir, &["clone", "-q", env.remote.to_str().unwrap(), seed.to_str().unwrap()]);
        std::fs::create_dir(seed.join("p")).unwrap();
        std::fs::write(seed.join("p/x"), "x\n").unwrap();
        git(&seed, &["add", "."]);
        git(&seed, &["-c", "user.email=t@t", "-c", "user.name=T", "commit", "-qm", "dir"]);
        git(&seed, &["rm", "-rq", "p"]);
        std::fs::write(seed.join("p"), "p1\n").unwrap();
        git(&seed, &["add", "."]);
        git(&seed, &["-c", "user.email=t@t", "-c", "user.name=T", "commit", "-qm", "t"]);
        git(&seed, &["push", "-q", "origin", "master"]);
        let w = env.dir.join("w");
        env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), w.to_str().unwrap()]).ok();
        std::fs::write(w.join("p"), "mine\n").unwrap();
        env.j(&w, &["describe \"mine\""]).ok();
        force_push_dropping_t(&seed);
        env.j(&w, &["fetch"]).ok();
        assert_eq!(env.j(&w, &[PARENT_MESSAGE]).ok().stdout.trim(), "dir");
        let conflicted = "\\r -> show (conflicted (files r))";
        assert_eq!(env.j(&w, &[conflicted]).ok().stdout.trim(), r#"[["p"]]"#, "{}", edit);
        if edit {
            std::fs::write(w.join("a.txt"), "one\nedited\n").unwrap();
        }
        env.j(&w, &["describe \"mine, rebased\""]).ok();
        assert_eq!(env.j(&w, &[conflicted]).ok().stdout.trim(), r#"[["p"]]"#, "{}", edit);
        let a = env.j(&w, &["\\r -> text (contentAt [\"a.txt\"] (files r))"]).ok().stdout;
        assert_eq!(a, if edit { "one\nedited\n" } else { "one\n" });
        let description = std::fs::read_to_string(w.join("p")).unwrap();
        assert!(description.starts_with("Conflict:\n") && description.contains("tree"), "{}", description);
        // the directory holds the focus now, so there is nothing more to record
        let ops = env.j(&w, &["ops"]).ok().stdout.lines().count();
        env.j(&w, &["id"]).ok();
        assert_eq!(env.j(&w, &["ops"]).ok().stdout.lines().count(), ops, "{}", edit);
    }
}

#[test]
#[cfg(unix)]
fn a_fetch_that_replaces_a_symlink_by_a_directory_has_nothing_cleared_through_it() {
    // §7.4, §7.6: the fetch rebases the working-copy commit onto `dir`,
    // where `link`, a symlink to a directory outside the working directory,
    // is a directory holding `link/cache/x`, and leaves the working copy
    // stale. The next run that persisted cleared the empty directories
    // where its checkout wrote a file, looking at each path through the
    // symlinks above it, and so removed the empty tree `cache/x` outside.
    let env = setup();
    let shared = env.dir.join("shared");
    std::fs::create_dir_all(shared.join("cache/x/tmp")).unwrap();
    let seed = env.dir.join("seed");
    git(&env.dir, &["clone", "-q", env.remote.to_str().unwrap(), seed.to_str().unwrap()]);
    std::fs::create_dir_all(seed.join("link/cache")).unwrap();
    std::fs::write(seed.join("link/cache/x"), "x\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["-c", "user.email=t@t", "-c", "user.name=T", "commit", "-qm", "dir"]);
    git(&seed, &["rm", "-rq", "link"]);
    std::os::unix::fs::symlink(&shared, seed.join("link")).unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["-c", "user.email=t@t", "-c", "user.name=T", "commit", "-qm", "use shared"]);
    git(&seed, &["push", "-q", "origin", "master"]);
    let w = env.dir.join("w");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), w.to_str().unwrap()]).ok();
    assert!(w.join("link").symlink_metadata().unwrap().is_symlink());
    // the collaborator drops `use shared`
    git(&seed, &["reset", "-q", "--hard", "HEAD~1"]);
    std::fs::write(seed.join("u"), "u\n").unwrap();
    git(&seed, &["add", "u"]);
    git(&seed, &["-c", "user.email=t@t", "-c", "user.name=T", "commit", "-qm", "u"]);
    git(&seed, &["push", "-qf", "origin", "master"]);
    env.j(&w, &["fetch"]).ok();
    assert_eq!(env.j(&w, &[PARENT_MESSAGE]).ok().stdout.trim(), "dir");
    env.j(&w, &["describe \"mine\""]).ok();
    assert!(shared.join("cache/x/tmp").is_dir());
    assert!(w.join("link").symlink_metadata().unwrap().is_dir());
    assert_eq!(std::fs::read_to_string(w.join("link/cache/x")).unwrap(), "x\n");
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
fn a_local_remote_is_stored_resolved_against_the_current_directory() {
    // §7.8: `clone` and `remote` stored a local path as typed, and git
    // resolves a relative one against the directory each later run is in:
    // cloned by `../remote.git` from a sibling directory, the clone's every
    // `fetch` and `push` failed with "Could not find repository", and so
    // did those of a repository whose `remote` was set from a subdirectory.
    // Such a path is now stored resolved against the current directory, as
    // `git clone` stores it; other URLs are stored unresolved.
    let env = setup();
    let work = env.dir.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let name = env.remote.file_name().unwrap().to_str().unwrap();
    let stored = |dir: &PathBuf| git(dir, &["config", "--get", "remote.origin.url"]).trim().to_string();
    let same_dir = |url: &str, dir: &PathBuf| {
        assert!(std::path::Path::new(url).is_absolute(), "stored {}", url);
        assert_eq!(std::fs::canonicalize(url).unwrap(), std::fs::canonicalize(dir).unwrap());
    };
    // `work` is in `env.dir`, a sibling of the remote
    env.j(&work, &["clone", &format!("../../{}", name), "c"]).ok();
    let c = work.join("c");
    same_dir(&stored(&c), &env.remote);
    // the clone's operation is described with the path as stored
    let described = |dir: &PathBuf, url: &str| {
        let ops = env.j(dir, &["ops"]).ok().stdout;
        assert!(ops.lines().any(|l| l.ends_with(&format!("  clone {}", url))), "{}", ops);
    };
    described(&c, &stored(&c));
    env.j(&c, &["fetch"]).ok();
    std::fs::write(c.join("b.txt"), "b\n").unwrap();
    env.j(&c, &["describe \"b\""]).ok();
    env.j(&c, &["push (label \"feature\" here)"]).ok();
    git(&env.remote, &["rev-parse", "--verify", "-q", "refs/heads/feature"]);

    // set from a subdirectory, fetched from the top
    let other = env.dir.join("other.git");
    git(&env.dir, &["clone", "-q", "--bare", env.remote.to_str().unwrap(), other.to_str().unwrap()]);
    std::fs::create_dir(c.join("sub")).unwrap();
    env.j(&c.join("sub"), &["remote", "../../../other.git"]).ok();
    same_dir(&stored(&c), &other);
    env.j(&c, &["fetch"]).ok();

    for url in [
        "file://../project.git",
        "https://example.com/team/project.git",
        "git@example.com:team/project.git",
        "ssh://git@example.com/team/project.git",
    ] {
        env.j(&c, &["remote", url]).ok();
        assert_eq!(stored(&c), url);
    }
    // any other URL is described as given, which git may store otherwise
    // (it lowercases a host): an ssh that runs the command here
    let ssh = env.dir.join("ssh");
    std::fs::write(&ssh, "#!/bin/sh\nshift\nexec sh -c \"git ${1#git-}\"\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let url = format!("MyHost:{}", env.remote.display());
    let vars = [("GIT_SSH_COMMAND", ssh.as_path()), ("GIT_SSH_VARIANT", std::path::Path::new("simple"))];
    env.j_env(&work, &["clone", &url, "s"], &vars).ok();
    described(&work.join("s"), &url);
    // a URL that cannot be parsed is a usage error, which changes nothing
    let bad = "ssh://example.com:port/project.git";
    let out = env.j(&c, &["remote", bad]);
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert_eq!(stored(&c), "ssh://git@example.com/team/project.git");
    let out = env.j(&work, &["clone", bad, "d"]);
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(!work.join("d").exists());
}

#[test]
fn a_relative_remote_is_reached_from_a_subdirectory() {
    // §7.8: an origin stored relative (by git, or by j before it resolved
    // local paths) was resolved against the directory j ran in, so fetch
    // and push failed from a subdirectory with "Could not find
    // repository"; git resolves it from the top of the working tree
    let env = setup();
    let g = env.dir.join("g");
    std::fs::create_dir(&g).unwrap();
    git(&g, &["init", "-q", "."]);
    let name = env.remote.file_name().unwrap().to_str().unwrap();
    git(&g, &["remote", "add", "origin", &format!("../../{}", name)]);
    env.j(&g, &["init"]).ok();
    let sub = g.join("sub");
    std::fs::create_dir(&sub).unwrap();
    env.j(&sub, &["fetch"]).ok();
    assert!(env.j(&g, &["tree"]).ok().stdout.contains("master"));
    std::fs::write(g.join("b.txt"), "b\n").unwrap();
    env.j(&sub, &["describe \"b\""]).ok();
    env.j(&sub, &["push (label \"feature\" here)"]).ok();
    git(&env.remote, &["rev-parse", "--verify", "-q", "refs/heads/feature"]);
    // the push records where the bookmark went
    let out = env.j(&sub, &["\\r -> (focus r).labels"]).ok();
    assert!(out.stdout.contains("feature"), "{}", out.stdout);
}

#[test]
fn a_local_remote_under_home_is_stored_as_given() {
    // §7.8: a `~` the shell did not expand (`j 'remote ~/r.git'`, or any
    // URL on stdin) was resolved as a directory named `~` in the current
    // one, so every later fetch and push failed with "Could not find
    // repository"; git expands it to the home directory on each fetch and
    // push, wherever it runs, so it is stored as given
    let env = setup();
    let home = env.dir.join("home");
    std::fs::create_dir(&home).unwrap();
    git(&env.dir, &["clone", "-q", "--bare", env.remote.to_str().unwrap(), home.join("r.git").to_str().unwrap()]);
    let vars = [("HOME", home.as_path())];
    let stored = |dir: &PathBuf| git(dir, &["config", "--get", "remote.origin.url"]).trim().to_string();
    let work = env.dir.join("work");
    std::fs::create_dir(&work).unwrap();
    env.j_env(&work, &["clone", "~/r.git", "c"], &vars).ok();
    let c = work.join("c");
    assert_eq!(stored(&c), "~/r.git");
    assert_eq!(std::fs::read_to_string(c.join("a.txt")).unwrap(), "one\n");
    // set from the top, used from a subdirectory
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), "d"]).ok();
    let d = env.dir.join("d");
    env.j_env(&d, &["remote ~/r.git"], &vars).ok();
    assert_eq!(stored(&d), "~/r.git");
    std::fs::create_dir(d.join("sub")).unwrap();
    std::fs::write(d.join("b.txt"), "b\n").unwrap();
    env.j(&d, &["describe \"b\""]).ok();
    env.j_env(&d.join("sub"), &["push (label \"feature\" here)"], &vars).ok();
    git(&home.join("r.git"), &["rev-parse", "--verify", "-q", "refs/heads/feature"]);
    env.j_env(&d.join("sub"), &["fetch"], &vars).ok();
}

#[cfg(unix)]
#[test]
fn a_local_remote_whose_path_is_not_utf8_is_refused() {
    // §7.8: a relative local path resolved into a directory whose name is
    // not UTF-8 was stored as typed, as jj-lib's add_remote takes UTF-8,
    // and then every fetch and push from anywhere else failed with "Could
    // not find repository"; it is a usage error that changes nothing
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let env = setup();
    let nu = env.dir.join(OsStr::from_bytes(b"nu\xff"));
    if std::fs::create_dir(&nu).is_err() {
        // a filesystem that takes only UTF-8 names (APFS) cannot hold one
        return;
    }
    let status = Command::new("git")
        .args(["clone", "-q", "--bare"])
        .arg(&env.remote)
        .arg(nu.join("r.git"))
        .status()
        .unwrap();
    assert!(status.success());
    let out = env.j(&nu, &["clone", "r.git", "c"]);
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(out.stderr.contains("not UTF-8"), "{}", out.stderr);
    assert!(!nu.join("c").exists());
    // `remote` from a clone in that directory
    env.j(&nu, &["clone", env.remote.to_str().unwrap(), "d"]).ok();
    let d = nu.join("d");
    let before = git(&d, &["config", "--get", "remote.origin.url"]);
    let out = env.j(&d, &["remote", "../r.git"]);
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert_eq!(git(&d, &["config", "--get", "remote.origin.url"]), before);
    env.j(&d, &["fetch"]).ok();
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

#[test]
fn clone_names_its_directory_after_the_repository() {
    // §7.8: the default DIR was what followed the URL's last `/`, minus
    // `.git`: empty for `<repo>/.git`, so the clone failed with "cannot
    // create : …" (" already contains a jj repository" inside one), and
    // `host:repo` for an scp-style `host:repo.git`
    let env = setup();
    let proj = env.dir.join("proj");
    git(&env.dir, &["clone", "-q", env.remote.to_str().unwrap(), proj.to_str().unwrap()]);
    for (i, url) in [format!("{}/.git", proj.display()), format!("{}/.git/", proj.display())].iter().enumerate() {
        let work = env.dir.join(format!("work{}", i));
        std::fs::create_dir(&work).unwrap();
        env.j(&work, &["clone", url]).ok();
        assert_eq!(std::fs::read_to_string(work.join("proj/a.txt")).unwrap(), "one\n");
    }
    // `host:repo.git`, through an ssh that runs the command here
    let work = env.dir.join("scp");
    std::fs::create_dir(&work).unwrap();
    git(&env.dir, &["clone", "-q", "--bare", env.remote.to_str().unwrap(), work.join("r.git").to_str().unwrap()]);
    let ssh = env.dir.join("ssh");
    std::fs::write(&ssh, "#!/bin/sh\nshift\nexec sh -c \"git ${1#git-}\"\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let vars = [("GIT_SSH_COMMAND", ssh.as_path()), ("GIT_SSH_VARIANT", std::path::Path::new("simple"))];
    env.j_env(&work, &["clone", "MyHost:r.git"], &vars).ok();
    assert_eq!(std::fs::read_to_string(work.join("r/a.txt")).unwrap(), "one\n");
    // a URL that names no directory needs DIR
    let work = env.dir.join("none");
    std::fs::create_dir(&work).unwrap();
    for url in ["/", "..", "MyHost:"] {
        let out = env.j(&work, &["clone", url]);
        assert_eq!(out.code, 2, "{}: {}", url, out.stderr);
        assert!(out.stderr.contains("j clone URL DIR"), "{}: {}", url, out.stderr);
    }
    assert_eq!(std::fs::read_dir(&work).unwrap().count(), 0);
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
