//! Difftastic receives valid filenames without changing their basenames.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};

const CONFIG: &str = include_str!("../config.j");

#[test]
fn difft_preserves_long_and_special_filenames() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    let config = temp.path().join("config");
    let bin = temp.path().join("bin");
    std::fs::create_dir(&repo).unwrap();
    std::fs::create_dir_all(config.join("j")).unwrap();
    std::fs::create_dir(&bin).unwrap();
    std::fs::write(config.join("j/config.j"), CONFIG).unwrap();
    let stub = bin.join("difft");
    std::fs::write(&stub, r#"#!/bin/sh
case "$1" in */old/*) ;; *) exit 10 ;; esac
case "$2" in */new/*) ;; *) exit 11 ;; esac
[ "${1##*/}" = "${2##*/}" ] || exit 12
[ "$1" != "$2" ] || exit 13
IFS= read -r before < "$1"
IFS= read -r after < "$2"
printf '%s|%s|%s\n' "${1##*/}" "$before" "$after"
"#).unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let run = |expr: &str| -> Output {
        Command::new(env!("CARGO_BIN_EXE_j"))
            .arg(expr)
            .current_dir(&repo)
            .env("XDG_CONFIG_HOME", &config)
            .env("PATH", &bin)
            .env("NO_COLOR", "1")
            .output()
            .unwrap()
    };
    let init = run("init");
    assert!(init.status.success(), "{}", String::from_utf8_lossy(&init.stderr));
    for name in [
        format!("{}.rs", "x".repeat(252)), // NAME_MAX = 255 bytes, including the suffix
        "Makefile".to_string(),
        "file with spaces.rs".to_string(),
    ] {
        let expr = format!("difft [{}] (blob \"before\\n\") (blob \"after\\n\")", j::show::text_literal(&name));
        let result = run(&expr);
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        assert_eq!(String::from_utf8(result.stdout).unwrap(), format!("{}|before|after\n", name));
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
