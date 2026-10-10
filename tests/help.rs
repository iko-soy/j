//! Discovery works before a repository exists and never executes the vocabulary.

use std::process::{Command, Output, Stdio};

struct HelpCli {
    dir: tempfile::TempDir,
    config: tempfile::TempDir,
}

impl HelpCli {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            config: tempfile::tempdir().unwrap(),
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_j"));
        command
            .current_dir(self.dir.path())
            .env("XDG_CONFIG_HOME", self.config.path())
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    fn write_config(&self, source: &str) {
        std::fs::create_dir_all(self.config.path().join("j")).unwrap();
        std::fs::write(self.config.path().join("j/config.j"), source).unwrap();
    }
}

fn stdout(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn help_and_catalog_work_without_creating_config_or_repository() {
    let cli = HelpCli::new();
    let overview = stdout(cli.run(&["help"]));
    for alias in ["--help", "-h"] {
        assert_eq!(stdout(cli.run(&[alias])), overview);
    }
    assert!(overview.contains("j commands"));
    assert!(overview.contains("j help"));
    assert!(overview.contains("describe"));
    let commands = stdout(cli.run(&["commands"]));
    assert!(commands.contains("describe"));
    assert!(commands.contains("fetch"));
    assert!(commands.contains("embedded default"));
    let topic = stdout(cli.run(&["help", "describe"]));
    assert!(topic.contains("Text -> Edit"), "{}", topic);
    assert_eq!(stdout(cli.run(&["--help", "describe"])), topic);
    assert_eq!(
        stdout(cli.run(&["help", "(+)"])),
        stdout(cli.run(&["help", "+"]))
    );
    assert_eq!(std::fs::read_dir(cli.dir.path()).unwrap().count(), 0);
    assert_eq!(std::fs::read_dir(cli.config.path()).unwrap().count(), 0);
}

#[test]
fn overview_and_reserved_topics_survive_broken_config() {
    let cli = HelpCli::new();
    cli.write_config("broken =\n");
    stdout(cli.run(&["help"]));
    for name in [
        "init", "clone", "remote", "fetch", "push", "undo", "redo", "ops", "help", "commands",
    ] {
        assert!(stdout(cli.run(&["help", name])).contains(name));
    }
    for args in [&["commands"][..], &["help", "describe"]] {
        let output = cli.run(args);
        assert_eq!(output.status.code(), Some(3));
        assert!(String::from_utf8_lossy(&output.stderr).contains("config.j"));
        assert!(output.stdout.is_empty());
    }
    assert_eq!(
        std::fs::read_to_string(cli.config.path().join("j/config.j")).unwrap(),
        "broken =\n"
    );
}

#[test]
fn catalog_uses_actual_config_and_searches_custom_documentation() {
    let cli = HelpCli::new();
    // Deliberately omit reference commands and declare functions that must
    // never be evaluated just to learn what is available.
    cli.write_config("crash : Text -> a\nuser = crash \"must not run\"\ntree = 0\nimmutable = []\nlabelled = \\name -> []\n\n-- Publish the release notes.\nship = crash \"must not run\"\n\n-- Transform a callback.\ntransform : (Text -> Text) -> Text -> Text\ntransform = \\f value -> f value\n");
    let commands = stdout(cli.run(&["commands"]));
    assert!(commands.contains("ship"));
    assert!(commands.contains("transform"));
    assert!(!commands.contains("squash"));
    assert!(!commands.contains("touchedPaths"));
    let search = stdout(cli.run(&["commands", "RELEASE"]));
    assert!(search.contains("ship"), "{}", search);
    assert!(!search.contains("transform"));
    let topic = stdout(cli.run(&["help", "ship"]));
    assert!(topic.contains("Publish the release notes."), "{}", topic);
    assert!(topic.contains("config.j"));
    let signature = stdout(cli.run(&["help", "transform"]));
    assert!(
        signature.contains("(Text -> Text) -> Text -> Text"),
        "{}",
        signature
    );
    assert_eq!(std::fs::read_dir(cli.dir.path()).unwrap().count(), 0);
}

#[test]
fn discovery_errors_offer_a_next_step_and_keep_usage_exit_codes() {
    let cli = HelpCli::new();
    for args in [
        &[][..],
        &["  \n  "],
        &["help", "missing-name"],
        &["help", "one", "two"],
        &["push"],
    ] {
        let output = cli.run(args);
        assert_eq!(output.status.code(), Some(2), "{:?}", args);
        assert!(output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            error.contains("j help") || error.contains("j commands"),
            "{}",
            error
        );
    }
    let empty = stdout(cli.run(&["commands", "no-such-command-xyz"]));
    assert!(empty.to_lowercase().contains("no "), "{}", empty);
    assert!(empty.contains("j commands"), "{}", empty);
    assert_eq!(std::fs::read_dir(cli.config.path()).unwrap().count(), 0);
}

#[test]
fn piped_help_and_expression_comments_preserve_input_rules() {
    use std::io::Write;
    let cli = HelpCli::new();
    let mut child = cli.command().stdin(Stdio::piped()).spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"help push\n")
        .unwrap();
    assert_eq!(
        stdout(child.wait_with_output().unwrap()),
        stdout(cli.run(&["help", "push"]))
    );

    cli.write_config(include_str!("../config.j"));
    stdout(cli.run(&["init"]));
    assert_eq!(stdout(cli.run(&["\"--help\""])), "--help\n");
    assert_eq!(stdout(cli.run(&["(help) or 42"])), "42\n");
    assert_eq!(stdout(cli.run(&["(commands) or 42"])), "42\n");
    assert_eq!(stdout(cli.run(&["42 --help"])), "42\n");
}

#[test]
fn help_does_not_take_repository_lock_or_snapshot_changes() {
    use std::os::fd::AsRawFd;
    let cli = HelpCli::new();
    cli.write_config(include_str!("../config.j"));
    stdout(cli.run(&["init"]));
    let before = stdout(cli.run(&["ops"]));
    std::fs::write(cli.dir.path().join("work.txt"), "unsaved work\n").unwrap();
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(cli.dir.path().join(".jj/j.lock"))
        .unwrap();
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    stdout(cli.run(&["help"]));
    stdout(cli.run(&["help", "new"]));
    stdout(cli.run(&["commands", "Edit"]));
    drop(lock);
    assert_eq!(stdout(cli.run(&["ops"])), before);
    assert_eq!(
        std::fs::read_to_string(cli.dir.path().join("work.txt")).unwrap(),
        "unsaved work\n"
    );
}

#[test]
fn help_read_failures_do_not_fall_back_to_reference_commands() {
    let cli = HelpCli::new();
    std::fs::create_dir_all(cli.config.path().join("j/config.j")).unwrap();
    let output = cli.run(&["commands"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot read config"));
    stdout(cli.run(&["help", "clone"]));
}
