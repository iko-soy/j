//! Newcomer vocabulary remains ordinary, pure config code.

use j::config::{self, Config};
use j::domain::{MemBackend, MetaInfo, ROOT_ID};
use j::eval::Interp;
use j::parse::parse_expr;
use j::value::{value_eq, BlobVal, Crash, Env, Value};
use std::rc::Rc;

const CONFIG: &str = include_str!("../config.j");

fn commit(id: &str, message: &str, content: Option<&str>) -> Value {
    let files = content.map(|text| Value::record(&[
        ("path", Value::list(vec![Value::text("work.txt")])),
        ("content", BlobVal::text_blob(text)),
    ])).into_iter().collect();
    Value::record(&[
        ("id", Value::Id(Rc::new(id.to_string()))),
        ("message", Value::text(message)),
        ("labels", Value::list(vec![])),
        ("files", Value::list(files)),
    ])
}

fn repo(content: &str, children: Vec<Value>) -> Value {
    Value::record(&[
        ("root", commit("kppppppp", "current work", Some(content))),
        ("children", Value::list(children)),
        ("context", Value::list(vec![Value::record(&[
            ("parent", commit(ROOT_ID, "", None)),
            ("left", Value::list(vec![])),
            ("right", Value::list(vec![])),
        ])])),
    ])
}

fn subtree(id: &str, message: &str) -> Value {
    Value::record(&[
        ("root", commit(id, message, Some("recorded"))),
        ("children", Value::list(vec![])),
    ])
}

fn interpreter(source: &str, loaded: &Value, given: &Value) -> (Interp, Config) {
    let cfg = config::load_config(source).unwrap();
    let mut backend = MemBackend::new();
    for id in [ROOT_ID, "kppppppp", "kqqqqqqq", "krrrrrrr"] {
        backend.metas.insert(id.to_string(), MetaInfo {
            hash: id.to_string(), author: "Test".into(), email: "test@example.com".into(), time: 1,
        });
    }
    let mut interp = Interp::new(Rc::new(backend), cfg.shapes.clone(), Env::empty());
    *interp.old_repo.borrow_mut() = Some(loaded.clone());
    *interp.given_repo.borrow_mut() = Some(given.clone());
    config::eval_config(&mut interp, &cfg).unwrap();
    (interp, cfg)
}

fn evaluate(interp: &mut Interp, cfg: &Config, source: &str, repo: Value) -> Result<Value, Crash> {
    let expression = parse_expr(source, Rc::new(cfg.global_names.clone())).unwrap();
    let expression = config::resolve_ids(&expression, interp).unwrap();
    let value = interp.eval(&Rc::new(expression), &interp.global_env())?;
    if matches!(value, Value::Fun(_)) { interp.apply(value, repo) } else { Ok(value) }
}

#[test]
fn save_keeps_the_snapshot_and_preview_validates_without_returning_a_repo() {
    let old = repo("recorded", vec![]);
    let given = repo("edited", vec![]);
    let (mut interp, cfg) = interpreter(CONFIG, &old, &given);
    let saved = evaluate(&mut interp, &cfg, "save", given.clone()).unwrap();
    assert!(value_eq(&saved, &given).unwrap());
    let preview = evaluate(&mut interp, &cfg, "preview (describe \"ready for review\")", given.clone()).unwrap();
    assert!(preview.as_text().unwrap().contains("ready for review"));
    assert_eq!(given.field("root").unwrap().field("message").unwrap().as_text().unwrap(), "current work");
    assert!(value_eq(interp.old_repo.borrow().as_ref().unwrap(), &old).unwrap());
    let error = evaluate(&mut interp, &cfg, "preview (describe \"change root\" . top)", given).err().unwrap();
    assert!(error.msg.contains("focus must be a mutable commit"), "{}", error.msg);
}

#[test]
fn next_at_a_tip_recommends_creating_a_child() {
    let given = repo("recorded", vec![]);
    let (mut interp, cfg) = interpreter(CONFIG, &given, &given);
    let error = evaluate(&mut interp, &cfg, "next", given.clone()).err().unwrap();
    assert!(error.msg.contains("no children") && error.msg.contains("j new"), "{}", error.msg);
    // tip still stops at a leaf; recovery text must not change navigation.
    let tip = evaluate(&mut interp, &cfg, "tip", given.clone()).unwrap();
    assert!(value_eq(&tip, &given).unwrap());
}

#[test]
fn ambiguous_navigation_lists_commit_messages_and_usable_ids() {
    let given = repo("recorded", vec![
        subtree("kqqqqqqq", "keyboard support\nprivate second line"),
        subtree("krrrrrrr", ""),
    ]);
    let (mut interp, cfg) = interpreter(CONFIG, &given, &given);
    let error = evaluate(&mut interp, &cfg, "next", given.clone()).err().unwrap();
    for expected in ["expected one revision, got 2", "@kqqqqqqq", "keyboard support", "@krrrrrrr", "(no description)", "j by @ID"] {
        assert!(error.msg.contains(expected), "missing {expected}: {}", error.msg);
    }
    assert!(!error.msg.contains("private second line"), "{}", error.msg);
    let selected = evaluate(&mut interp, &cfg, "by @kqqqqqqq", given).unwrap();
    assert!(matches!(selected.field("root").unwrap().field("id").unwrap(), Value::Id(id) if id.as_str() == "kqqqqqqq"));
}

#[test]
fn missing_remote_navigation_explains_fetch_without_changing_empty_revsets() {
    let given = repo("recorded", vec![]);
    let (mut interp, cfg) = interpreter(CONFIG, &given, &given);
    for expression in ["trunk", "%main"] {
        assert!(evaluate(&mut interp, &cfg, expression, given.clone()).unwrap().as_list().unwrap().is_empty());
    }
    for expression in ["goto trunk", "goto %main", "goto (const [])"] {
        let error = evaluate(&mut interp, &cfg, expression, given.clone()).err().unwrap();
        for expected in ["expected one revision, got 0", "j tree", "j remote URL", "j fetch"] {
            assert!(error.msg.contains(expected), "{expression}: missing {expected}: {}", error.msg);
        }
    }
}

#[test]
fn status_distinguishes_commit_changes_from_unrecorded_work() {
    let old = repo("recorded", vec![]);
    let given = repo("edited", vec![]);
    let (mut interp, cfg) = interpreter(CONFIG, &old, &given);
    let status = evaluate(&mut interp, &cfg, "status", given.clone()).unwrap();
    let paths = Value::list(vec![Value::list(vec![Value::text("work.txt")])]);
    assert!(value_eq(&status.field("changed").unwrap(), &paths).unwrap());
    assert!(value_eq(&status.field("unrecorded").unwrap(), &paths).unwrap());

    let (mut interp, cfg) = interpreter(CONFIG, &given, &given);
    let recorded = evaluate(&mut interp, &cfg, "status", given).unwrap();
    assert!(value_eq(&recorded.field("changed").unwrap(), &paths).unwrap());
    assert!(recorded.field("unrecorded").unwrap().as_list().unwrap().is_empty());
}

#[test]
fn older_custom_configs_can_omit_new_vocabulary() {
    let older = CONFIG
        .replace("unrecorded : Repo -> [Path]\n", "")
        .replace("save : Edit\nsave = id\n", "")
        .replace("preview : Edit -> Repo -> Text\npreview = \\edit -> tree . validate . edit\n", "")
        .replace(", unrecorded : [Path]", "")
        .replace("\n  , unrecorded = unrecorded repo }", " }");
    let given = repo("recorded", vec![]);
    let (mut interp, cfg) = interpreter(&older, &given, &given);
    for name in ["save", "preview", "unrecorded"] { assert!(interp.globals.lookup(name).is_none(), "{name} unexpectedly available"); }
    let status = evaluate(&mut interp, &cfg, "status", given).unwrap();
    assert!(status.field("unrecorded").is_err());
    assert!(matches!(status, Value::Record(fields) if fields.len() == 5));
}
