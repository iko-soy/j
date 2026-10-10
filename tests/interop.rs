//! Interop tests: `jj` (the real binary) reads what `j` writes and `j` reads
//! what `jj` writes (§7 preamble). Skipped automatically if jj is absent.

use std::path::PathBuf;
use std::process::Command;

fn jj_available() -> bool {
    Command::new("jj")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

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
    std::env::temp_dir().join(format!(
        "j-interop-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

struct T {
    dir: PathBuf,
    cfg: PathBuf,
}

fn setup() -> Option<T> {
    if !jj_available() {
        return None;
    }
    let dir = uniq("repo");
    let cfg = uniq("cfg");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&cfg.join("j")).unwrap();
    let mut config = include_str!("../config.j").to_string();
    config = config.replace("Your Name", "Test User");
    config = config.replace("you@example.com", "test@example.com");
    std::fs::write(cfg.join("j/config.j"), config).unwrap();
    Some(T { dir, cfg })
}

impl T {
    fn j(&self, args: &[&str]) -> (i32, String, String) {
        let out = Command::new(j_bin())
            .args(args)
            .current_dir(&self.dir)
            .env("XDG_CONFIG_HOME", &self.cfg)
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    }

    fn jj(&self, args: &[&str]) -> (i32, String, String) {
        let out = Command::new("jj")
            .args(args)
            .current_dir(&self.dir)
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    }
}

impl Drop for T {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
        let _ = std::fs::remove_dir_all(&self.cfg);
    }
}

#[test]
fn sparse_working_copies_are_refused_without_changing_files_or_operations() {
    for partial in [false, true] {
        let Some(t) = setup() else { return };
        let (code, _, err) = t.j(&["init"]);
        assert_eq!(code, 0, "{}", err);
        std::fs::write(t.dir.join("a"), "original\n").unwrap();
        let (code, _, err) = t.j(&["describe \"base\""]);
        assert_eq!(code, 0, "{}", err);
        let args = if partial {
            vec!["sparse", "set", "--clear", "--add", "included"]
        } else {
            vec!["sparse", "set", "--clear"]
        };
        let (code, _, err) = t.jj(&args);
        assert_eq!(code, 0, "{}", err);
        assert!(!t.dir.join("a").exists());
        std::fs::write(t.dir.join("a"), "edited while excluded\n").unwrap();
        std::fs::write(t.dir.join("b"), "new while excluded\n").unwrap();
        let operations = || {
            let (code, out, err) = t.jj(&[
                "--ignore-working-copy", "op", "log", "--no-graph", "-T", "id ++ \"\\n\"",
            ]);
            assert_eq!(code, 0, "{}", err);
            out
        };
        let patterns = || {
            let (code, out, err) = t.jj(&["--ignore-working-copy", "sparse", "list"]);
            assert_eq!(code, 0, "{}", err);
            out
        };
        let before = operations();
        let sparse = patterns();
        for expr in ["status", "id", "describe \"must not run\"", "undo", "ops"] {
            let (code, _, err) = t.j(&[expr]);
            assert_eq!(code, 2, "{}: {}", expr, err);
            assert!(err.contains("sparse working copies are not supported"), "{}", err);
            assert!(err.contains("jj sparse set --clear --add ."), "{}", err);
            assert_eq!(std::fs::read_to_string(t.dir.join("a")).unwrap(), "edited while excluded\n");
            assert_eq!(std::fs::read_to_string(t.dir.join("b")).unwrap(), "new while excluded\n");
        }
        assert_eq!(operations(), before);
        assert_eq!(patterns(), sparse);

        // Restoring full patterns explicitly makes j usable again. Files
        // created while excluded remain intact and can now be recorded.
        let (code, _, err) = t.jj(&["sparse", "set", "--clear", "--add", "."]);
        assert_eq!(code, 0, "{}", err);
        let (code, _, err) = t.j(&["id"]);
        assert_eq!(code, 0, "{}", err);
        let (code, files, err) = t.j(&["show . files"]);
        assert_eq!(code, 0, "{}", err);
        assert!(files.contains("edited while excluded"), "{}", files);
        assert!(files.contains("new while excluded"), "{}", files);
    }
}

#[test]
fn jj_reads_js_commits() {
    let Some(t) = setup() else { return };
    let (code, _, err) = t.j(&["init"]);
    assert_eq!(code, 0, "{}", err);
    std::fs::write(t.dir.join("a.txt"), "content\n").unwrap();
    t.j(&["describe \"from j\""]);
    let (code, log, _) = t.jj(&["log", "--no-pager", "-T", "description"]);
    assert_eq!(code, 0);
    assert!(log.contains("from j"), "{}", log);
}

#[test]
fn j_reads_jjs_commits() {
    let Some(t) = setup() else { return };
    // init with jj, then drive with j
    std::fs::write(t.dir.join("a.txt"), "one\n").unwrap();
    let (code, _, err) = t.jj(&["git", "init", "--colocate"]);
    assert_eq!(code, 0, "{}", err);
    let (code, _, err) = t.jj(&["describe", "-m", "from jj"]);
    assert_eq!(code, 0, "{}", err);
    let (code, status, err) = t.j(&["status"]);
    assert_eq!(code, 0, "{}", err);
    assert!(status.contains("from jj"), "{}", status);
}

#[test]
fn jj_sees_js_operations() {
    let Some(t) = setup() else { return };
    t.j(&["init"]);
    std::fs::write(t.dir.join("a.txt"), "x\n").unwrap();
    t.j(&["describe \"work\""]);
    t.j(&["new"]);
    let (code, ops, _) = t.jj(&["op", "log", "--no-pager"]);
    assert_eq!(code, 0);
    assert!(ops.contains("describe \"work\""), "{}", ops);
    assert!(ops.contains("new"), "{}", ops);
}

#[test]
fn jj_can_continue_from_js_state() {
    let Some(t) = setup() else { return };
    t.j(&["init"]);
    std::fs::write(t.dir.join("a.txt"), "x\n").unwrap();
    t.j(&["describe \"base\""]);
    // jj edits and snapshots
    std::fs::write(t.dir.join("b.txt"), "y\n").unwrap();
    let (code, _, _) = t.jj(&["describe", "-m", "jj continues"]);
    assert_eq!(code, 0);
    let (code, log, _) = t.jj(&["log", "--no-pager", "-T", "description"]);
    assert_eq!(code, 0);
    assert!(log.contains("jj continues"), "{}", log);
    // j reads it back
    let (code, status, _) = t.j(&["status"]);
    assert_eq!(code, 0);
    assert!(status.contains("jj continues"), "{}", status);
}

#[test]
fn jj_undo_interop_marker() {
    let Some(t) = setup() else { return };
    t.j(&["init"]);
    std::fs::write(t.dir.join("a.txt"), "x\n").unwrap();
    t.j(&["describe \"one\""]);
    // jj's own undo
    let (code, _, _) = t.jj(&["undo"]);
    assert_eq!(code, 0);
    // j's ops shows jj's undo
    let (code, ops, _) = t.j(&["ops"]);
    assert_eq!(code, 0);
    assert!(ops.contains("undo"), "{}", ops);
}

#[test]
fn j_and_jj_continue_each_others_undo_redo_chains() {
    let sequences: &[&[(&str, &str, &str)]] = &[
        &[
            ("jj", "undo", "two"), ("j", "redo", "three"),
            ("j", "undo", "two"), ("j", "undo", "one"),
            ("j", "redo", "two"), ("j", "redo", "three"),
        ],
        &[
            ("jj", "undo", "two"), ("jj", "undo", "one"),
            ("j", "undo", "zero"), ("j", "redo", "one"),
            ("j", "redo", "two"), ("j", "redo", "three"),
        ],
        &[
            ("jj", "undo", "two"), ("jj", "undo", "one"),
            ("jj", "redo", "two"), ("j", "redo", "three"),
            ("j", "undo", "two"), ("jj", "undo", "one"),
            ("jj", "redo", "two"), ("jj", "redo", "three"),
        ],
        &[
            ("j", "undo", "two"), ("j", "undo", "one"),
            ("jj", "redo", "two"), ("jj", "redo", "three"),
            ("jj", "undo", "two"), ("j", "undo", "one"),
            ("j", "redo", "two"), ("jj", "redo", "three"),
        ],
        &[
            ("jj", "undo", "two"), ("j", "redo", "three"),
            ("j", "undo", "two"), ("jj", "undo", "one"),
            ("j", "redo", "two"), ("jj", "redo", "three"),
        ],
    ];
    for sequence in sequences {
        let Some(t) = setup() else { return };
        for expr in ["init", "describe \"zero\"", "describe \"one\"", "describe \"two\"", "describe \"three\""] {
            let (code, _, err) = t.j(&[expr]);
            assert_eq!(code, 0, "{}: {}", expr, err);
        }
        for &(tool, command, expected) in *sequence {
            let (code, _, err) = if tool == "j" { t.j(&[command]) } else { t.jj(&[command]) };
            assert_eq!(code, 0, "{} {}: {}", tool, command, err);
            let (code, message, err) = t.j(&["\\r -> r.root.message"]);
            assert_eq!(code, 0, "{}", err);
            assert_eq!(message.trim(), expected, "after {} {} in {:?}", tool, command, sequence);
        }
        let (code, _, err) = t.j(&["redo"]);
        assert_eq!(code, 1, "{}", err);
        assert!(err.contains("nothing to redo"), "{}", err);
        let (code, ops, err) = t.j(&["ops"]);
        assert_eq!(code, 0, "{}", err);
        assert!(ops.lines().any(|line| line.ends_with("(undo)")), "{}", ops);
        assert!(ops.lines().any(|line| line.ends_with("(redo)")), "{}", ops);
    }
}

#[test]
fn modern_undo_redo_continues_legacy_j_markers() {
    use jj_lib::config::{ConfigLayer, ConfigSource, StackedConfig};
    use jj_lib::default_backend_factories::{default_backend_factories, default_working_copy_factories};
    use jj_lib::object_id::ObjectId;
    use jj_lib::repo::ReadonlyRepo;
    use std::sync::Arc;

    // Build operations with the exact descriptions older j versions wrote.
    // Only descriptions change in this history, so no checkout is needed.
    let restore = |repo: &Arc<ReadonlyRepo>, target: &jj_lib::operation::Operation, description: String| {
        let view = pollster::block_on(target.op_store().read_view(target.view_id())).unwrap();
        let mut tx = repo.start_transaction();
        tx.repo_mut().set_view(view);
        pollster::block_on(pollster::block_on(tx.write(description)).unwrap().publish()).unwrap()
    };
    for start_with_redo in [false, true] {
        let Some(t) = setup() else { return };
        for expr in ["init", "describe \"zero\"", "describe \"one\"", "describe \"two\"", "describe \"three\""] {
            let (code, _, err) = t.j(&[expr]);
            assert_eq!(code, 0, "{}", err);
        }
        let mut config = StackedConfig::with_defaults();
        config.add_layer(ConfigLayer::parse(ConfigSource::User, "[user]\nname = 'Test'\nemail = 't@t'\n").unwrap());
        let settings = jj_lib::settings::UserSettings::from_config(config).unwrap();
        let ws = jj_lib::workspace::Workspace::load(
            &settings, &t.dir, &default_backend_factories(), &default_working_copy_factories(),
        ).unwrap();
        let repo = pollster::block_on(ws.repo_loader().load_at_head()).unwrap();
        let three = repo.operation().clone();
        let two = pollster::block_on(three.parents()).unwrap().remove(0);
        let one = pollster::block_on(two.parents()).unwrap().remove(0);
        let repo = restore(&repo, &two, format!("undo: {}", three.id().hex()));
        let repo = restore(&repo, &one, format!("undo: {}", two.id().hex()));
        if start_with_redo {
            let undo_id = repo.operation().id().hex();
            restore(&repo, &two, format!("redo: {}", undo_id));
            let (code, _, err) = t.j(&["undo"]);
            assert_eq!(code, 0, "{}", err);
        }
        for expected in ["two", "three"] {
            let (code, _, err) = t.j(&["redo"]);
            assert_eq!(code, 0, "{}", err);
            let (code, message, err) = t.j(&["\\r -> r.root.message"]);
            assert_eq!(code, 0, "{}", err);
            assert_eq!(message.trim(), expected);
        }
    }
}

#[test]
fn jj_reads_conflicts_written_by_j() {
    let Some(t) = setup() else { return };
    let j_ok = |expr: &str| {
        let (code, _, err) = t.j(&[expr]);
        assert_eq!(code, 0, "{}: {}", expr, err);
    };
    j_ok("init");
    std::fs::write(t.dir.join("f.txt"), "base\n").unwrap();
    j_ok("describe \"base\"");
    // side 1
    j_ok("new");
    std::fs::write(t.dir.join("f.txt"), "one\n").unwrap();
    j_ok("describe \"one\"");
    // side 2 from the parent
    j_ok("new . prev");
    std::fs::write(t.dir.join("f.txt"), "two\n").unwrap();
    j_ok("describe \"two\"");
    // rebase two onto one: conflicts
    j_ok("rebase (matching (\\c -> c.message == \"one\") all)");
    // jj sees the conflict
    let (code, log, _) = t.jj(&["log", "--no-pager"]);
    assert_eq!(code, 0);
    assert!(log.contains("conflict"), "{}", log);
    // resolve via jj's working copy and j records it
    std::fs::write(t.dir.join("f.txt"), "resolved\n").unwrap();
    j_ok("id");
    let (code, log, _) = t.jj(&["log", "--no-pager"]);
    assert_eq!(code, 0);
    assert!(!log.contains("conflict"), "{}", log);
}
