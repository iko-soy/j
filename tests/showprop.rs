//! Property tests: `show` round-trips through the parser for generated values
//! of every kind (§10 golden tests).

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
