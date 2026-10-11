//! User-facing invocation and diagnostic regressions use isolated repositories.

use std::path::Path;
use std::process::{Command, Output};

struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("repo")).unwrap();
        std::fs::create_dir_all(root.path().join("config/j")).unwrap();
        let source = include_str!("../config.j")
            .replace("Your Name", "UX Test")
            .replace("you@example.com", "ux@example.com");
        std::fs::write(root.path().join("config/j/config.j"), source).unwrap();
        Self { root }
    }

    fn run_in(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_j"))
            .args(args)
            .current_dir(cwd)
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("NO_COLOR", "1")
            .output().unwrap()
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_in(&self.root.path().join("repo"), args)
    }
}

fn success(output: Output) -> Output {
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    output
}

#[test]
fn shell_quoted_clone_and_remote_paths_keep_their_spaces() {
    let f = Fixture::new();
    let remote = f.root.path().join("source remote.git");
    success(Command::new("git").args(["init", "--bare", "-q"]).arg(&remote).output().unwrap());
    success(f.run_in(f.root.path(), &["clone", remote.to_str().unwrap(), "clone with spaces"]));
    let clone = f.root.path().join("clone with spaces");
    assert!(clone.join(".jj").is_dir());
    success(f.run_in(&clone, &["status"]));

    let other = f.root.path().join("different remote.git");
    success(Command::new("git").args(["init", "--bare", "-q"]).arg(&other).output().unwrap());
    success(f.run_in(&clone, &["remote", other.to_str().unwrap()]));
    let url = success(Command::new("git").arg("-C").arg(&clone)
        .args(["config", "--get", "remote.origin.url"]).output().unwrap());
    assert_eq!(String::from_utf8(url.stdout).unwrap().trim(), other.to_str().unwrap());
    success(f.run_in(&clone, &["fetch"]));
}

#[test]
fn expression_diagnostics_offer_usable_next_steps_without_editing() {
    let f = Fixture::new();
    success(f.run(&["init"]));
    let head_dir = f.root.path().join("repo/.jj/repo/op_heads/heads");
    let heads = || {
        let mut ids: Vec<_> = std::fs::read_dir(&head_dir).unwrap()
            .map(|e| e.unwrap().file_name()).collect();
        ids.sort();
        ids
    };
    let before = heads();
    for (args, expected) in [
        (vec!["describe", "hello"], "j 'describe \"hello\"'"),
        (vec!["neww"], "new"),
        (vec!["describe"], "j help describe"),
        (vec!["rebase"], "j help rebase"),
    ] {
        let output = f.run(&args);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains(expected), "{args:?}: {error}");
    }
    assert_eq!(heads(), before);

    let output = f.run(&["describe (\n"]);
    assert_eq!(output.status.code(), Some(3));
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("expression"), "{error}");
    assert!(error.contains("line"), "{error}");
}

#[test]
fn broken_configuration_names_the_exact_file_and_recovery_help() {
    let f = Fixture::new();
    let path = f.root.path().join("config/j/config.j");
    std::fs::write(&path, "broken =\n").unwrap();
    let output = f.run(&["status"]);
    assert_eq!(output.status.code(), Some(3));
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains(path.to_str().unwrap()), "{error}");
    assert!(error.contains("broken ="), "{error}");
    assert!(error.contains("j help config"), "{error}");
    assert_eq!(std::fs::read_to_string(path).unwrap(), "broken =\n");
}
