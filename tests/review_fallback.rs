//! Review works with an isolated PATH and no optional diff renderer.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Fixture {
    _root: tempfile::TempDir,
    repo: PathBuf,
    config: PathBuf,
    bin: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        let config = root.path().join("config");
        let bin = root.path().join("bin");
        std::fs::create_dir(&repo).unwrap();
        std::fs::create_dir(&bin).unwrap();
        std::fs::create_dir_all(config.join("j")).unwrap();
        std::fs::write(
            config.join("j/config.j"),
            include_str!("../config.j")
                .replace("Your Name", "Review Test")
                .replace("you@example.com", "review@example.com"),
        ).unwrap();
        let fixture = Self { _root: root, repo, config, bin };
        fixture.ok("init");
        fixture
    }

    fn run(&self, expression: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_j"))
            .arg(expression)
            .current_dir(&self.repo)
            .env("XDG_CONFIG_HOME", &self.config)
            .env("PATH", &self.bin)
            .env("NO_COLOR", "1")
            .output().unwrap()
    }

    fn ok(&self, expression: &str) -> String {
        let out = self.run(expression);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert!(out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    }
}

#[test]
fn review_without_difft_identifies_added_deleted_and_modified_paths() {
    let fixture = Fixture::new();
    for (dir, content) in [("src", "before\n"), ("other", "gone\n")] {
        std::fs::create_dir(fixture.repo.join(dir)).unwrap();
        std::fs::write(fixture.repo.join(dir).join("file.txt"), content).unwrap();
    }
    fixture.ok("id");
    fixture.ok("new");
    std::fs::write(fixture.repo.join("src/file.txt"), "after").unwrap();
    std::fs::remove_file(fixture.repo.join("other/file.txt")).unwrap();
    std::fs::write(fixture.repo.join("added.txt"), "new\n").unwrap();

    assert_eq!(fixture.ok("review"), concat!(
        "--- a/added.txt\n+++ b/added.txt\n@@ -0,0 +1 @@\n+new\n",
        "--- a/other/file.txt\n+++ b/other/file.txt\n@@ -1 +0,0 @@\n-gone\n",
        "--- a/src/file.txt\n+++ b/src/file.txt\n@@ -1 +1 @@\n-before\n+after\n",
        "\\ No newline at end of file\n",
    ));
}

#[test]
fn fallback_reports_binary_changes_without_corrupting_their_bytes() {
    let fixture = Fixture::new();
    std::fs::write(fixture.repo.join("invalid.bin"), [0xff]).unwrap();
    std::fs::write(fixture.repo.join("nul.bin"), b"a\0").unwrap();
    assert_eq!(fixture.ok("review"), concat!(
        "Binary files a/invalid.bin and b/invalid.bin differ\n",
        "Binary files a/nul.bin and b/nul.bin differ\n",
    ));
    assert_eq!(std::fs::read(fixture.repo.join("invalid.bin")).unwrap(), [0xff]);
    assert_eq!(std::fs::read(fixture.repo.join("nul.bin")).unwrap(), b"a\0");
}

#[test]
fn fallback_quotes_paths_and_omits_equal_content() {
    let fixture = Fixture::new();
    assert_eq!(fixture.ok("difft [\"dir name\" \"file.txt\"] (blob \"a\\n\") (blob \"b\\n\")"), concat!(
        "--- \"a/dir name/file.txt\"\n+++ \"b/dir name/file.txt\"\n",
        "@@ -1 +1 @@\n-a\n+b\n",
    ));
    assert!(fixture.ok("difft ./same (blob \"same\") (blob \"same\")").trim().is_empty());
}

#[test]
fn an_unexecutable_renderer_reports_its_error_without_falling_back() {
    let fixture = Fixture::new();
    let executable = fixture.bin.join("difft");
    std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::write(fixture.repo.join("file.txt"), "new\n").unwrap();
    let out = fixture.run("review");
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let error = String::from_utf8(out.stderr).unwrap();
    assert!(error.contains("cannot execute `difft`"), "{error}");
    assert!(error.contains("Permission denied"), "{error}");
}
