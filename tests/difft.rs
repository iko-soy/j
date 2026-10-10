//! Difftastic receives unchanged basenames in isolated temporary directories.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

#[test]
fn difft_preserves_basenames_and_cleans_up_temporary_files() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    let config = temp.path().join("config");
    let bin = temp.path().join("bin");
    std::fs::create_dir(&repo).unwrap();
    std::fs::create_dir_all(config.join("j")).unwrap();
    std::fs::create_dir(&bin).unwrap();
    std::fs::write(
        config.join("j/config.j"),
        include_str!("../config.j")
            .replace("Your Name", "Test User")
            .replace("you@example.com", "test@example.com"),
    ).unwrap();
    let stub = bin.join("difft");
    std::fs::write(&stub, "#!/bin/sh\nIFS= read -r old < \"$1\"\nIFS= read -r new < \"$2\"\nprintf '%s\\n' \"$1\" \"$2\" \"$old\" \"$new\"\n").unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let run = |expression: &str| -> Output {
        Command::new(env!("CARGO_BIN_EXE_j"))
            .arg(expression)
            .current_dir(&repo)
            .env("XDG_CONFIG_HOME", &config)
            .env("PATH", &bin)
            .env("NO_COLOR", "1")
            .output()
            .unwrap()
    };
    let init = run("init");
    assert!(init.status.success(), "{}", String::from_utf8_lossy(&init.stderr));

    let longest = format!("{}.rs", "a".repeat(252)); // NAME_MAX, including suffix
    for (path, basename) in [
        (format!("[{}]", j::show::text_literal(&longest)), longest.as_str()),
        ("[\"src\" \"main.rs\"]".to_string(), "main.rs"),
        ("[\"Makefile\"]".to_string(), "Makefile"),
        ("[\"file with spaces.rs\"]".to_string(), "file with spaces.rs"),
        ("[]".to_string(), "file"),
    ] {
        let result = run(&format!("difft {} (blob \"old\\n\") (blob \"new\\n\")", path));
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        let output = String::from_utf8(result.stdout).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 4, "{output}");
        assert_eq!(&lines[2..], &["old", "new"]);
        let old = PathBuf::from(lines[0]);
        let new = PathBuf::from(lines[1]);
        assert_eq!(old.file_name().unwrap(), basename);
        assert_eq!(new.file_name().unwrap(), basename);
        assert_eq!(old.parent().unwrap().file_name().unwrap(), "old");
        assert_eq!(new.parent().unwrap().file_name().unwrap(), "new");
        assert_ne!(old.parent(), new.parent());
        assert_eq!(old.parent().unwrap().parent(), new.parent().unwrap().parent());
        assert!(!old.parent().unwrap().parent().unwrap().exists(), "temporary directory leaked");
    }

    // Paths are structural lists at run time. A malformed last component
    // must not let `join` put temporary blob contents outside their directory.
    let escaped = temp.path().join("escaped");
    for name in ["", ".", "..", "../escaped", escaped.to_str().unwrap()] {
        let expr = format!("difft [{}] (blob \"before\") (blob \"after\")", j::show::text_literal(name));
        let result = run(&expr);
        assert_eq!(result.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&result.stderr).contains("last path component"));
    }
    assert!(!escaped.exists());
}
