//! Property tests: `show` round-trips through the parser for generated values
//! of every kind (§10 golden tests), and `diff`'s hunk headers agree with
//! their bodies (§4.9).

use j::config;
use j::domain::MemBackend;
use j::eval::Interp;
use j::parse::parse_expr;
use j::show::show;
use j::value::{value_eq, BlobVal, Env, Value};
use proptest::prelude::*;
use std::rc::Rc;

const CONFIG: &str = include_str!("../config.j");

fn make_interp() -> (Interp, config::Config) {
    let cfg = config::load_config(CONFIG).expect("config");
    let mut i = Interp::new(Rc::new(MemBackend::new()), cfg.shapes.clone(), Env::empty());
    config::eval_config(&mut i, &cfg).expect("eval");
    (i, cfg)
}

fn arb_value(depth: usize) -> BoxedStrategy<Value> {
    let leaf = prop_oneof![
        any::<i64>().prop_map(Value::int),
        "[a-zA-Z0-9 \n\t\"\\\\]{0,12}".prop_map(Value::text),
        any::<bool>().prop_map(Value::Bool),
        Just(BlobVal::text_blob("blob content")),
    ];
    leaf.prop_recursive(depth as u32, 24, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::list),
            prop::collection::vec(
                ("[a-hj-rt-z]{1}[a-z]{0,5}", inner).prop_filter("not a keyword", |(n, _)| {
                    !matches!(n.as_str(), "let" | "in" | "if" | "then" | "else" | "or" | "true" | "false")
                }),
                0..3,
            )
            .prop_map(|fields| Value::Record(Rc::new(fields.into_iter().collect()))),
        ]
    })
    .boxed()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn show_roundtrips_generated_values(v in arb_value(4)) {
        let (mut i, cfg) = make_interp();
        let s = show(&i, &v).unwrap();
        let outer = Rc::new(cfg.global_names.clone());
        let e = parse_expr(&s, outer)
            .unwrap_or_else(|err| panic!("show output does not parse: {:?}\nerror: {}", s, err.msg));
        let e = config::resolve_ids(&e, &i).unwrap();
        let env = i.global_env();
        let v2 = i.eval(&Rc::new(e), &env)
            .unwrap_or_else(|c| panic!("show output does not evaluate: {:?}\nerror: {}", s, c.msg));
        prop_assert!(value_eq(&v, &v2).unwrap_or(false), "roundtrip: {}", s);
    }
}

#[test]
fn show_record_field_order_stability() {
    let (i, _cfg) = make_interp();
    // records always render with ascending field names
    for _ in 0..10 {
        let v = Value::record(&[
            ("z", Value::int(1)),
            ("a", Value::int(2)),
            ("m", Value::int(3)),
        ]);
        assert_eq!(show(&i, &v).unwrap(), "{ a = 2, m = 3, z = 1 }");
    }
}

// ------------------------------------------------------------------
// unified diff (§4.9 `diff`)
// ------------------------------------------------------------------

/// Lines as code has them, repeated often enough to match across sides.
fn arb_code_line() -> impl Strategy<Value = String> {
    let lines = vec!["\n", "}\n", "    x\n", "fn f() {\n", "a\n", "b\n", "a\r\n", "\r\n"];
    prop::sample::select(lines).prop_map(String::from)
}

/// Two texts to diff: short strings of line ends and a few letters, two
/// runs of code-like lines, or a text and a few edits of it, which leaves
/// runs long enough between the edits to split the diff into hunks. Either
/// may lose its final newline.
fn arb_diff_pair() -> impl Strategy<Value = (String, String)> {
    let line = prop_oneof![(0u8..40).prop_map(|n| format!("line {}\n", n)), arb_code_line()];
    let edited = (
        prop::collection::vec(line.clone(), 0..60),
        prop::collection::vec((any::<prop::sample::Index>(), 0u8..3, line), 0..6),
    )
        .prop_map(|(old, edits)| {
            let mut new = old.clone();
            for (at, kind, l) in edits {
                let i = at.index(new.len() + 1);
                match kind {
                    0 => new.insert(i, l),
                    _ if i == new.len() => new.push(l),
                    1 => drop(new.remove(i)),
                    _ => new[i] = l,
                }
            }
            (old.concat(), new.concat())
        });
    let code = || prop::collection::vec(arb_code_line(), 0..15);
    let pairs = prop_oneof![
        ("[abc\n\r]{0,30}", "[abc\n\r]{0,30}"),
        (code(), code()).prop_map(|(a, b)| (a.concat(), b.concat())),
        edited,
    ];
    (pairs, any::<bool>(), any::<bool>()).prop_map(|((mut a, mut b), cut_a, cut_b)| {
        if cut_a && a.ends_with('\n') {
            a.pop();
        }
        if cut_b && b.ends_with('\n') {
            b.pop();
        }
        (a, b)
    })
}

/// `-s,c` or `+s,c` of a hunk header, `c` 1 when left out.
fn hunk_range(r: &str) -> (usize, usize) {
    match r[1..].split_once(',') {
        Some((s, c)) => (s.parse().unwrap(), c.parse().unwrap()),
        None => (r[1..].parse().unwrap(), 1),
    }
}

/// Reads `d`, the unified diff from `a` to `b`, a hunk at a time: each
/// header's counts are the lines its body shows of that side, its starts are
/// where those lines are (the line before, for an empty range), and applying
/// the hunks in order to `a`'s lines gives `b`'s.
fn check_unified_diff(a: &str, b: &str, d: &str) -> Result<(), TestCaseError> {
    let old: Vec<&str> = a.split_inclusive('\n').collect();
    let new: Vec<&str> = b.split_inclusive('\n').collect();
    prop_assert_eq!(d.is_empty(), a == b);
    let mut lines = d.split_inclusive('\n').peekable();
    // `b` as the hunks rebuild it, and the first line of `a` not yet in it
    let mut applied: Vec<String> = Vec::new();
    let mut next_old = 0;
    while let Some(header) = lines.next() {
        let ranges = header.strip_prefix("@@ ").and_then(|h| h.strip_suffix(" @@\n"));
        let Some((o, n)) = ranges.and_then(|r| r.split_once(' ')) else {
            return Err(TestCaseError::fail(format!("not a hunk header: {:?}", header)));
        };
        prop_assert!(o.starts_with('-') && n.starts_with('+'), "header {:?}", header);
        let ((os, oc), (ns, nc)) = (hunk_range(o), hunk_range(n));
        // the body, each line's sign and its text with its terminator, which
        // a following marker says it does not have
        let mut body: Vec<(char, String)> = Vec::new();
        while let Some(l) = lines.next_if(|l| !l.starts_with("@@")) {
            if l == "\\ No newline at end of file\n" {
                let last = body.last_mut().expect("a marker follows a line");
                prop_assert_eq!(last.1.pop(), Some('\n'));
                continue;
            }
            let sign = l.chars().next().unwrap();
            prop_assert!(matches!(sign, ' ' | '-' | '+'), "body line {:?}", l);
            body.push((sign, l[1..].to_string()));
        }
        let side = |skip: char| {
            body.iter().filter(|(s, _)| *s != skip).map(|(_, t)| t.as_str()).collect::<Vec<_>>()
        };
        let (olines, nlines) = (side('+'), side('-'));
        prop_assert_eq!((oc, nc), (olines.len(), nlines.len()), "counts of {:?} in\n{}", header, d);
        let from = if oc == 0 { os } else { os - 1 };
        prop_assert!(from >= next_old && from + oc <= old.len(), "old range {:?} in\n{}", header, d);
        prop_assert_eq!(&old[from..from + oc], &olines[..], "old lines of {:?} in\n{}", header, d);
        applied.extend(old[next_old..from].iter().map(|l| l.to_string()));
        let to = if nc == 0 { ns } else { ns - 1 };
        prop_assert_eq!(to, applied.len(), "new start of {:?} in\n{}", header, d);
        applied.extend(nlines.iter().map(|l| l.to_string()));
        next_old = from + oc;
    }
    applied.extend(old[next_old..].iter().map(|l| l.to_string()));
    prop_assert_eq!(applied, new);
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(3000))]

    #[test]
    fn unified_diff_hunks_match_their_headers((a, b) in arb_diff_pair()) {
        check_unified_diff(&a, &b, &j::show::unified_diff(&a, &b))?;
    }
}
