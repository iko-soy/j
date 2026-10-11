//! Follow the tutorial as a new user would, including the files that reach a
//! remote and the difference between a preview and a Repo-returning edit.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const TUTORIAL: &str = include_str!("../docs/tutorial.md");

struct Session {
    root: tempfile::TempDir,
    config: PathBuf,
}

impl Session {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config");
        std::fs::create_dir_all(config.join("j")).unwrap();
        let source = include_str!("../config.j")
            .replace("Your Name", "Tutorial User")
            .replace("you@example.com", "tutorial@example.org");
        std::fs::write(config.join("j/config.j"), source).unwrap();
        Self { root, config }
    }

    fn command(&self, program: &str, cwd: &Path) -> Command {
        let mut command = Command::new(program);
        command.current_dir(cwd)
            .env("XDG_CONFIG_HOME", &self.config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("NO_COLOR", "1");
        command
    }

    fn j(&self, cwd: &Path, expression: &str) -> String {
        let mut command = self.command(env!("CARGO_BIN_EXE_j"), cwd);
        command.arg(expression);
        output(&mut command)
    }

    fn git(&self, cwd: &Path, args: &[&str]) -> String {
        let mut command = self.command("git", cwd);
        command.args(args);
        output(&mut command)
    }

    fn shell(&self, cwd: &Path, script: &str) {
        // Run the actual fenced commands, preserving shell quoting and `cd`.
        // Only these two explicitly selected tutorial sections are executable.
        let binary_dir = Path::new(env!("CARGO_BIN_EXE_j")).parent().unwrap();
        let mut paths = vec![binary_dir.to_path_buf()];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
        let mut command = self.command("sh", cwd);
        command.env("PATH", std::env::join_paths(paths).unwrap())
            .args(["-eu", "-c", script]);
        output(&mut command);
    }

    fn clone_main(&self) -> (PathBuf, PathBuf) {
        let remote = self.root.path().join("origin.git");
        let seed = self.root.path().join("seed");
        std::fs::create_dir(&seed).unwrap();
        self.git(self.root.path(), &["init", "--bare", "--initial-branch=main", remote.to_str().unwrap()]);
        self.git(&seed, &["init", "--initial-branch=main"]);
        std::fs::write(seed.join("README.md"), "A project to try j with.\n").unwrap();
        self.git(&seed, &["add", "README.md"]);
        self.git(&seed, &["-c", "user.name=Tutorial User", "-c", "user.email=tutorial@example.org",
            "-c", "commit.gpgsign=false", "commit", "-m", "Initial main"]);
        self.git(&seed, &["push", remote.to_str().unwrap(), "main"]);

        let repo = self.root.path().join("work");
        let mut clone = self.command(env!("CARGO_BIN_EXE_j"), self.root.path());
        clone.arg("clone").arg(&remote).arg(&repo);
        output(&mut clone);
        (repo, remote)
    }
}

fn output(command: &mut Command) -> String {
    let Output { status, stdout, stderr } = command.output().unwrap();
    assert!(status.success(), "{command:?} failed ({status}):\n{}\n{}",
        String::from_utf8_lossy(&stdout), String::from_utf8_lossy(&stderr));
    String::from_utf8(stdout).unwrap()
}

fn tutorial_blocks(heading: &str) -> Vec<String> {
    let section = TUTORIAL.split_once(heading).expect("tutorial section exists").1;
    let section = section.split("\n## ").next().unwrap();
    let mut blocks = Vec::new();
    let mut current = None::<String>;
    for line in section.lines() {
        if line.starts_with("```") {
            match current.take() {
                Some(block) => blocks.push(block),
                None => current = Some(String::new()),
            }
        } else if let Some(block) = &mut current {
            block.push_str(line);
            block.push('\n');
        }
    }
    assert!(current.is_none(), "unclosed tutorial code fence");
    blocks
}

fn operation_heads(repo: &Path) -> Vec<OsString> {
    let mut heads: Vec<_> = std::fs::read_dir(repo.join(".jj/repo/op_heads/heads"))
        .unwrap().map(|entry| entry.unwrap().file_name()).collect();
    heads.sort();
    heads
}

fn working_files(repo: &Path) -> BTreeMap<OsString, Vec<u8>> {
    std::fs::read_dir(repo).unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_type().unwrap().is_file())
        .map(|entry| (entry.file_name(), std::fs::read(entry.path()).unwrap()))
        .collect()
}

#[test]
fn tutorial_first_changes_put_each_description_on_the_files_it_names() {
    let session = Session::new();
    let blocks = tutorial_blocks("## Part 1 — Ten minutes to a working repository");
    // Output examples share this section with shell examples. Select the
    // executable fences by their first command, never their sample IDs.
    let script = blocks.into_iter()
        // The alternative `clone URL` example needs a user's remote; it is
        // separate from the preceding two-change session exercised here.
        .take_while(|block| !block.trim_start().starts_with("j clone "))
        .filter(|block| {
        let first = block.lines().find(|line| !line.trim().is_empty()).unwrap_or("");
        first.starts_with("mkdir ") || first.starts_with("echo ") || first.starts_with("j ")
    }).collect::<Vec<_>>().join("\n");
    assert!(script.contains("main.rs") && script.contains("lib.rs"), "{script}");
    session.shell(session.root.path(), &script);
    let repo = session.root.path().join("play");

    assert_eq!(session.j(&repo, "\\r -> r.root.message"), "add a library\n");
    assert_eq!(session.j(&repo, "changed"), "lib.rs\n");
    assert_eq!(session.j(&repo, "\\r -> (up r).root.message"), "start the project\n");
    assert_eq!(session.j(&repo, "changed . prev"), "main.rs\n");
    assert_eq!(session.j(&repo, "length . commits . top"), "3\n",
        "the first file must belong to the described commit, without an extra empty child");
}

#[test]
fn tutorial_realistic_session_pushes_both_parts_of_the_parser_to_main_based_branch() {
    let session = Session::new();
    let (repo, remote) = session.clone_main();
    let blocks = tutorial_blocks("## Part 10 — A realistic session, start to finish");
    let script = blocks.first().expect("the complete session is the first fence");
    assert!(script.contains("parser.rs") && script.contains("push"), "{script}");
    session.shell(&repo, script);

    assert_eq!(session.git(&remote, &["show", "parser:parser.rs"]), "fn parse() {}\n");
    assert_eq!(session.git(&remote, &["show", "parser:lib.rs"]), "mod parser;\n");
    assert_eq!(session.git(&remote, &["log", "-1", "--format=%s", "parser"]), "add the parser\n");
    assert_eq!(session.git(&remote, &["diff", "--name-only", "main..parser"]), "lib.rs\nparser.rs\n");
    assert_eq!(session.git(&remote, &["rev-parse", "parser^"]), session.git(&remote, &["rev-parse", "main"]));
}

#[test]
fn preview_squash_keeps_recorded_history_and_working_edits_while_validate_persists() {
    let session = Session::new();
    let repo = session.root.path().join("preview");
    std::fs::create_dir(&repo).unwrap();
    session.j(&repo, "init");
    std::fs::write(repo.join("first.txt"), "first part\n").unwrap();
    session.j(&repo, "describe \"first\"");
    session.j(&repo, "new");
    std::fs::write(repo.join("second.txt"), "second part\n").unwrap();
    session.j(&repo, "describe \"second\"");

    // On a clean working copy these are the actual recorded commit hashes,
    // descriptions, and IDs, rather than hashes of an unpublished snapshot.
    let commits = "\\r -> show (map (\\c -> { id = c.id, hash = (meta c.id).hash, message = c.message }) (commits (top r)))";
    let recorded = session.j(&repo, commits);
    let heads = operation_heads(&repo);
    std::fs::write(repo.join("pending.txt"), "an edit not yet recorded\n").unwrap();
    let files = working_files(&repo);
    let preview = session.j(&repo, "preview squash");
    assert!(preview.contains("first"), "{preview}");
    assert_eq!(operation_heads(&repo), heads, "preview must not publish an operation");
    assert_eq!(working_files(&repo), files, "preview must not checkout its result");

    // Restoring the original disk contents lets the metadata inspection read
    // recorded commits directly again, without a transient dirty snapshot.
    std::fs::remove_file(repo.join("pending.txt")).unwrap();
    assert_eq!(session.j(&repo, commits), recorded, "preview must not rewrite recorded commits");
    assert_eq!(operation_heads(&repo), heads);

    session.j(&repo, "validate . squash");
    assert_ne!(operation_heads(&repo), heads, "validate returns a Repo, so this command saves the squash");
    assert_ne!(session.j(&repo, commits), recorded);
    assert_eq!(session.j(&repo, "changed . prev"), "first.txt\nsecond.txt\n");
}
