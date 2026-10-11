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

    fn reference_config(&self) {
        self.write_config(&include_str!("../config.j")
            .replace("Your Name", "Help Test")
            .replace("you@example.com", "help@example.com"));
    }

    fn operation_heads(&self) -> Vec<std::ffi::OsString> {
        let mut heads: Vec<_> = std::fs::read_dir(self.dir.path().join(".jj/repo/op_heads/heads"))
            .unwrap().map(|entry| entry.unwrap().file_name()).collect();
        heads.sort();
        heads
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
        "init", "clone", "remote", "fetch", "push", "undo", "redo", "ops", "help", "commands", "setup",
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

    cli.reference_config();
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
    cli.reference_config();
    stdout(cli.run(&["init"]));
    let before = cli.operation_heads();
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
    assert_eq!(cli.operation_heads(), before);
    assert_eq!(
        std::fs::read_to_string(cli.dir.path().join("work.txt")).unwrap(),
        "unsaved work\n"
    );
}

#[test]
fn separate_help_flags_never_execute_edits_or_take_the_repository_lock() {
    use std::os::fd::AsRawFd;
    let cli = HelpCli::new();
    // Help also works without a repository, creating no config.
    assert_eq!(stdout(cli.run(&["abandon", "--help"])), stdout(cli.run(&["help", "abandon"])));
    assert!(stdout(cli.run(&["clone", "--help"])).contains("Clone"));
    assert_eq!(std::fs::read_dir(cli.config.path()).unwrap().count(), 0);

    cli.reference_config();
    stdout(cli.run(&["init"]));
    std::fs::write(cli.dir.path().join("draft.txt"), "valuable work\n").unwrap();
    stdout(cli.run(&["describe \"valuable work\""]));
    let before = cli.operation_heads();
    std::fs::write(cli.dir.path().join("draft.txt"), "unrecorded edits\n").unwrap();
    let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true)
        .open(cli.dir.path().join(".jj/j.lock")).unwrap();
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }, 0);
    for command in ["abandon", "new", "squash", "describe"] {
        for flag in ["--help", "-h"] {
            assert_eq!(stdout(cli.run(&[command, flag])), stdout(cli.run(&["help", command])));
        }
    }
    assert_eq!(cli.operation_heads(), before);
    assert_eq!(std::fs::read_to_string(cli.dir.path().join("draft.txt")).unwrap(), "unrecorded edits\n");
}

#[test]
fn config_help_is_available_when_configuration_is_broken() {
    let cli = HelpCli::new();
    cli.write_config("broken =\n");
    let help = stdout(cli.run(&["help", "config"]));
    assert!(help.contains("user ="));
    assert!(help.contains(cli.config.path().join("j/config.j").to_str().unwrap()));
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

#[test]
fn default_discovery_is_short_and_full_catalog_remains_available() {
    let cli = HelpCli::new();
    let common = stdout(cli.run(&["commands"]));
    assert!(common.lines().count() <= 40, "{}", common);
    for command in ["j setup", "j save", "j 'preview squash'", "j commands all"] {
        assert!(common.contains(command), "{}", common);
    }
    assert!(!common.contains("(++) :"));
    assert!(!common.contains("Subtree ="));
    let all = stdout(cli.run(&["commands", "all"]));
    assert!(all.contains("(++) :"));
    assert!(all.contains("Subtree ="));
    assert!(all.contains("save : Edit"));
    assert_eq!(std::fs::read_dir(cli.config.path()).unwrap().count(), 0);
}

#[test]
fn discovery_understands_beginner_tasks_without_inventing_commands() {
    let cli = HelpCli::new();
    for (query, expected) in [("save", "save : Edit"), ("record", "save : Edit"),
        ("commit", "describe : Text -> Edit"), ("branch", "new : Edit"),
        ("checkout", "goto : Revset -> Edit")] {
        let output = stdout(cli.run(&["commands", query]));
        let first = output.lines().find(|line| line.starts_with("  ")).unwrap();
        assert!(first.contains(expected), "{}", output);
    }
    let stash = stdout(cli.run(&["commands", "stash"]));
    assert!(stash.contains("no direct stash command"));
    assert!(stash.contains("j save") && stash.contains("j new"));
    cli.write_config("user = {}\ntree = 0\nimmutable = []\nlabelled = 0\n-- Custom shelving command.\nstash = 42\n");
    let stash = stdout(cli.run(&["commands", "stash"]));
    assert!(stash.contains("stash (definition; no signature)"));
    assert!(!stash.contains("no direct stash"));
    let record = stdout(cli.run(&["commands", "record"]));
    assert!(!record.contains("save :") && !record.contains("id :"));
}

#[test]
fn command_help_leads_with_plain_explanations_and_setup_is_actionable() {
    let cli = HelpCli::new();
    let describe = stdout(cli.run(&["help", "describe"]));
    assert!(describe.starts_with("Set the message"), "{}", describe);
    assert!(describe.find("Example:").unwrap() < describe.find("Text -> Edit").unwrap());
    let config = stdout(cli.run(&["help", "config"]));
    assert!(config.contains("j setup 'Ada Lovelace' ada@example.org"));
    assert!(config.contains("not a complete config"));
    let clone = stdout(cli.run(&["help", "clone"]));
    assert!(clone.contains("cd project"));
    assert!(clone.contains("does not change your shell's directory"));
}

#[test]
fn everyday_catalog_respects_custom_docs_and_missing_example_dependencies() {
    let cli = HelpCli::new();
    cli.write_config("user = {}\ntree = 0\nimmutable = []\nlabelled = 0\n-- Display my custom report.\npreview : Text -> Text\npreview = \\report -> report\n-- Open our custom dashboard.\nnew : Text -> Text\nnew = \\dashboard -> dashboard\n");
    let common = stdout(cli.run(&["commands"]));
    assert!(common.contains("Display my custom report."));
    assert!(common.contains("Open our custom dashboard."));
    assert!(common.contains("j help preview"));
    assert!(!common.contains("preview squash"));
    assert!(!common.contains("Start the next commit"));
    assert!(!common.contains("j save"));
    let all = stdout(cli.run(&["commands", "all"]));
    assert!(!all.contains("squash :"));
}
