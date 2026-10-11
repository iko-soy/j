//! First-run configuration must be complete, safe to repeat, and independent
//! of repository state.

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

struct SetupCli {
    dir: tempfile::TempDir,
    config: tempfile::TempDir,
}

impl SetupCli {
    fn new() -> Self {
        Self { dir: tempfile::tempdir().unwrap(), config: tempfile::tempdir().unwrap() }
    }

    fn path(&self) -> PathBuf { self.config.path().join("j/config.j") }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_j"));
        command.args(args).current_dir(self.dir.path()).env("XDG_CONFIG_HOME", self.config.path())
            .env("NO_COLOR", "1").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        command
    }

    fn run(&self, args: &[&str]) -> Output { self.command(args).output().unwrap() }
}

fn success(output: Output) -> String {
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(output.stderr.is_empty(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn setup_with_identity_creates_a_complete_usable_config_without_a_repo() {
    let cli = SetupCli::new();
    let output = success(cli.run(&["setup", "Ada Lovelace", "ada@example.org"]));
    assert!(output.contains("Configuration ready"));
    assert!(output.contains("j init"));
    assert!(!cli.dir.path().join(".jj").exists());
    let source = std::fs::read_to_string(cli.path()).unwrap();
    let cfg = j::config::load_config(&source).unwrap();
    assert!(cfg.defs.iter().any(|(name, _)| name == "new"));
    assert!(cfg.defs.iter().any(|(name, _)| name == "save"));
    assert_eq!(j::config::eval_user_only(&mut j::eval::Interp::dummy(), &cfg).unwrap(), ("Ada Lovelace".into(), "ada@example.org".into()));
    success(cli.run(&["init"]));
    success(cli.run(&["status"]));

    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(cli.path()).unwrap().permissions().mode() & 0o777, 0o600);
    }
}

#[test]
fn bare_setup_creates_the_entire_default_and_explains_the_identity_edit() {
    let cli = SetupCli::new();
    let output = success(cli.run(&["setup"]));
    assert!(output.contains("user = { name ="));
    assert!(output.contains("Keep the rest of the file"));
    assert_eq!(std::fs::read_to_string(cli.path()).unwrap(), include_str!("../config.j"));
    assert_eq!(std::fs::read_dir(cli.dir.path()).unwrap().count(), 0);
    let output = cli.run(&["init"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&output.stderr).contains("identity is not configured"));
}

#[test]
fn identity_strings_round_trip_quotes_backslashes_unicode_and_controls() {
    let cli = SetupCli::new();
    let name = "Ада \"Lovelace\" \\ Test\nSecond line\t";
    let email = "ada+\"quote\"\\suffix@example.org";
    success(cli.run(&["setup", name, email]));
    let cfg = j::config::load_config(&std::fs::read_to_string(cli.path()).unwrap()).unwrap();
    assert_eq!(j::config::eval_user_only(&mut j::eval::Interp::dummy(), &cfg).unwrap(), (name.into(), email.into()));
}

#[test]
fn existing_custom_or_broken_configs_are_never_replaced() {
    let cli = SetupCli::new();
    success(cli.run(&["setup", "Existing Person", "existing@example.org"]));
    let source = format!("{}\nmyCommand = \\repo -> repo\n", std::fs::read_to_string(cli.path()).unwrap());
    std::fs::write(cli.path(), &source).unwrap();
    let output = success(cli.run(&["setup"]));
    assert!(output.contains("already exists") && output.contains("ready"));
    assert!(output.contains("Existing Person"));
    let output = cli.run(&["setup", "Replacement", "replacement@example.org"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("was not changed"));
    assert_eq!(std::fs::read_to_string(cli.path()).unwrap(), source);

    std::fs::write(cli.path(), "broken =\n").unwrap();
    let output = success(cli.run(&["setup"]));
    assert!(output.contains("j help config"));
    assert!(output.contains("line 1:") && output.contains("unexpected"), "{}", output);
    assert_eq!(std::fs::read_to_string(cli.path()).unwrap(), "broken =\n");
}

#[test]
fn usage_and_invalid_identities_do_not_create_files() {
    for args in [
        vec!["setup", "one"],
        vec!["setup", "one", "two", "three"],
        vec!["setup", "", "ada@example.org"],
        vec!["setup", "Ada", "   "],
        vec!["setup", "Your Name", "you@example.com"],
    ] {
        let cli = SetupCli::new();
        let output = cli.run(&args);
        assert!(!output.status.success(), "{:?}", args);
        assert!(!cli.path().exists());
        assert_eq!(std::fs::read_dir(cli.config.path()).unwrap().count(), 0);
    }
}

#[test]
fn concurrent_setup_publishes_one_complete_config_without_overwriting() {
    let cli = SetupCli::new();
    let first = cli.command(&["setup", "First Person", "first@example.org"]).spawn().unwrap();
    let second = cli.command(&["setup", "Second Person", "second@example.org"]).spawn().unwrap();
    let first = first.wait_with_output().unwrap();
    let second = second.wait_with_output().unwrap();
    assert_ne!(first.status.success(), second.status.success());
    let cfg = j::config::load_config(&std::fs::read_to_string(cli.path()).unwrap()).unwrap();
    let identity = j::config::eval_user_only(&mut j::eval::Interp::dummy(), &cfg).unwrap();
    assert!(identity == ("First Person".into(), "first@example.org".into())
        || identity == ("Second Person".into(), "second@example.org".into()));
    assert_eq!(std::fs::read_dir(cli.path().parent().unwrap()).unwrap().count(), 1);
}

#[test]
#[cfg(unix)]
fn setup_does_not_follow_existing_symlinks_to_overwrite_files() {
    let cli = SetupCli::new();
    let target = cli.config.path().join("personal-config");
    std::fs::write(&target, "my private config").unwrap();
    std::fs::create_dir_all(cli.path().parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&target, cli.path()).unwrap();
    let output = cli.run(&["setup", "Ada", "ada@example.org"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "my private config");
    assert!(std::fs::symlink_metadata(cli.path()).unwrap().file_type().is_symlink());
}
