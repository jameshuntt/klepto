//! cfg predicates and the Cargo feature model. Fixture: tests/fixtures/features/.

mod common;

use common::*;
use klepto::*;
use std::collections::BTreeSet;

fn features_ws() -> Klepto {
    Klepto::new("app")
        .scan_workspace_root(fixture("features"))
        .expect("cargo metadata on fixture features")
        .parse()
        .expect("parse fixture features")
}

fn features_folder() -> Klepto {
    Klepto::new("app")
        .scan_in_folder(fixture("features").join("app").join("src"))
        .parse()
        .expect("parse fixture features/app/src")
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

fn feat(s: &str) -> CfgPredicate { CfgPredicate::Feature(s.to_string()) }
fn key(s: &str) -> CfgPredicate { CfgPredicate::Key(s.to_string()) }
fn kv(k: &str, v: &str) -> CfgPredicate { CfgPredicate::KeyValue(k.to_string(), v.to_string()) }
fn all(ps: Vec<CfgPredicate>) -> CfgPredicate { CfgPredicate::All(ps) }
fn any(ps: Vec<CfgPredicate>) -> CfgPredicate { CfgPredicate::Any(ps) }
fn not(p: CfgPredicate) -> CfgPredicate { CfgPredicate::Not(Box::new(p)) }

fn fn_cfg<'a>(k: &'a Klepto, fq: &str) -> &'a CfgPredicate {
    &k.functions
        .iter()
        .find(|f| f.fq_name == fq)
        .unwrap_or_else(|| panic!("no fn {fq}; have {:?}", k.functions.iter().map(|f| &f.fq_name).collect::<Vec<_>>()))
        .cfg
}

fn model(k: &Klepto) -> &FeatureModel {
    k.feature_model("app").expect("feature model for app")
}

// ---------------------------------------------------------------- F rules

#[test]
fn feature_model_reads_cargo_features() {
    let k = features_ws();
    let m = model(&k);
    assert_eq!(m.package, "app");

    let names: BTreeSet<&str> = m.features.keys().map(String::as_str).collect();
    for expected in ["default", "fast", "net", "full", "weak", "opt_a"] {
        assert!(names.contains(expected), "missing feature {expected} in {names:?}");
    }
    assert!(!names.contains("opt_b"), "F4: `dep:opt_b` suppresses the implicit feature: {names:?}");

    assert_eq!(m.features["default"].enables, vec![FeatureEnable::Feature("fast".into())]);
    assert_eq!(m.features["fast"].enables, vec![]);
    assert_eq!(
        m.features["net"].enables,
        vec![FeatureEnable::Dep("opt_b".into()), FeatureEnable::Feature("opt_a".into())]
    );
    assert_eq!(
        m.features["full"].enables,
        vec![
            FeatureEnable::Feature("net".into()),
            FeatureEnable::DepFeature { dep: "core_dep".into(), feature: "extra".into(), weak: false },
        ]
    );
    assert_eq!(
        m.features["weak"].enables,
        vec![FeatureEnable::DepFeature { dep: "opt_b".into(), feature: "extra".into(), weak: true }]
    );
    assert!(m.features["opt_a"].implicit, "F4 implicit feature");
    assert_eq!(m.features["opt_a"].enables, vec![FeatureEnable::Dep("opt_a".into())]);
    assert!(!m.features["net"].implicit);

    // other members get models too
    assert!(k.feature_model("core_dep").unwrap().features.contains_key("extra"));
    assert!(k.feature_model("nope").is_none());
}

#[test]
fn dependencies_carry_kind_optional_target_and_implicit_feature() {
    let k = features_ws();
    let m = model(&k);
    let dep = |n: &str| m.dep(n).unwrap_or_else(|| panic!("no dep {n}; have {:?}", m.dependencies));

    assert!(!dep("core_dep").optional);
    assert_eq!(dep("core_dep").kind, DepKind::Normal);
    assert_eq!(dep("core_dep").target, None);
    assert!(!dep("core_dep").implicit_feature);
    assert_eq!(dep("core_dep").package, "core_dep");
    assert!(dep("core_dep").default_features);
    assert!(dep("core_dep").features.is_empty());

    assert!(dep("opt_a").optional);
    assert!(dep("opt_a").implicit_feature, "F4");
    assert!(dep("opt_b").optional);
    assert!(!dep("opt_b").implicit_feature, "F4: referenced with dep:");

    assert_eq!(dep("devonly").kind, DepKind::Dev, "F3");
    assert_eq!(dep("unixonly").target, Some(key("unix")), "F3 target");
    assert_eq!(dep("unixonly").kind, DepKind::Normal);
    assert!(!dep("unixonly").optional);

    assert!(m.dep("nonexistent").is_none());
}

// ----------------------------------------------------------------
#[test]
fn feature_closure_and_enabling() {
    let k = features_ws();
    let m = model(&k);

    assert_eq!(m.active_features(&["full"], true), set(&["default", "fast", "full", "net", "opt_a"]), "Q1");
    assert_eq!(m.active_features(&[], true), set(&["default", "fast"]));
    assert_eq!(m.active_features(&[], false), set(&[]));
    assert_eq!(m.active_features(&["weak"], false), set(&["weak"]), "weak enables nothing else");
    assert_eq!(m.active_features(&["net"], false), set(&["net", "opt_a"]));

    assert_eq!(m.deps_enabled_by("net"), set(&["opt_a", "opt_b"]), "Q2");
    assert_eq!(m.deps_enabled_by("full"), set(&["opt_a", "opt_b"]));
    assert_eq!(m.deps_enabled_by("weak"), set(&[]), "weak never enables a dependency");
    assert_eq!(m.deps_enabled_by("fast"), set(&[]));
    assert_eq!(m.deps_enabled_by("opt_a"), set(&["opt_a"]), "implicit feature");

    assert_eq!(m.features_enabling("opt_b"), set(&["full", "net"]), "Q3");
    assert_eq!(m.features_enabling("opt_a"), set(&["full", "net", "opt_a"]));
    assert_eq!(m.features_enabling("core_dep"), set(&[]), "non-optional");

    assert!(m.dep_active("opt_b", &m.active_features(&["net"], false)));
    assert!(!m.dep_active("opt_b", &m.active_features(&[], true)));
    assert!(m.dep_active("core_dep", &set(&[])));

    assert_eq!(m.default_env(), CfgEnv::with_features(["default", "fast"]));
}

#[test]
fn dep_predicates() {
    let k = features_ws();
    let m = model(&k);
    assert_eq!(m.dep_predicate("core_dep"), CfgPredicate::True, "Q4");
    assert_eq!(m.dep_predicate("opt_b"), any(vec![feat("full"), feat("net")]), "sorted feature names");
    assert_eq!(m.dep_predicate("opt_a"), any(vec![feat("full"), feat("net"), feat("opt_a")]));
    assert_eq!(m.dep_predicate("unixonly"), key("unix"));
    assert_eq!(m.dep_predicate("devonly"), key("test"));
    assert_eq!(m.dep_predicate("nope"), CfgPredicate::Unknown("nope".into()));
}

// ---------------------------------------------------------------- C rules

#[test]
fn cfg_predicate_parsing() {
    assert_eq!(CfgPredicate::parse_str(r#"feature = "net""#), feat("net"), "C1");
    assert_eq!(
        CfgPredicate::parse_str(r#"all(feature = "net", not(feature = "fast"))"#),
        all(vec![feat("net"), not(feat("fast"))]),
        "C3"
    );
    assert_eq!(CfgPredicate::parse_str("unix"), key("unix"), "C2");
    assert_eq!(CfgPredicate::parse_str(r#"target_os = "linux""#), kv("target_os", "linux"), "C2");
    assert_eq!(CfgPredicate::parse_str(r#"any(unix, target_os = "linux",)"#), any(vec![key("unix"), kv("target_os", "linux")]), "trailing comma");
    assert_eq!(CfgPredicate::parse_str("all()"), CfgPredicate::True, "C3");
    assert_eq!(CfgPredicate::parse_str("cfg(unix)"), key("unix"), "C4 cargo target string");
    assert_eq!(CfgPredicate::parse_str("x86_64-unknown-linux-gnu"), kv("target", "x86_64-unknown-linux-gnu"), "C4 triple");
    assert!(matches!(CfgPredicate::parse_str("bogus("), CfgPredicate::Unknown(_)), "C5");
    assert!(matches!(CfgPredicate::parse_str(r#"version("1.70")"#), CfgPredicate::Unknown(_)), "C5");

    let p = all(vec![feat("net"), not(feat("fast")), key("unix"), kv("target_os", "linux")]);
    assert_eq!(p.features(), set(&["fast", "net"]));
    assert_eq!(p.keys(), set(&["target_os=linux", "unix"]));
    assert_eq!(CfgPredicate::True.and(feat("a")), feat("a"), "C6");
    assert_eq!(feat("a").and(all(vec![feat("b"), feat("c")])), all(vec![feat("a"), feat("b"), feat("c")]), "C6 flattening");
}

// ---------------------------------------------------------------- E rules

#[test]
fn items_and_modules_carry_effective_cfg() {
    let k = features_ws();
    assert_eq!(fn_cfg(&k, "app::always::a"), &CfgPredicate::True);
    assert_eq!(fn_cfg(&k, "app::net_slow"), &all(vec![feat("net"), not(feat("fast"))]), "E1");
    assert_eq!(fn_cfg(&k, "app::either"), &any(vec![feat("net"), feat("full")]));
    assert_eq!(fn_cfg(&k, "app::linux_fn"), &kv("target_os", "linux"));
    assert!(matches!(fn_cfg(&k, "app::weird"), CfgPredicate::Unknown(_)), "C5 on an item");

    // The whole of netmod.rs is under `net`
    assert_eq!(k.modules.get("app::netmod").unwrap().cfg, feat("net"));
    assert_eq!(fn_cfg(&k, "app::netmod::n"), &feat("net"));
    assert_eq!(fn_cfg(&k, "app::netmod::n_fast"), &all(vec![feat("net"), feat("fast")]), "E2 + own cfg");
    assert_eq!(k.modules.get("app::tests").unwrap().cfg, key("test"));
    assert_eq!(fn_cfg(&k, "app::tests::t"), &key("test"));

    // On impl items and impl blocks
    assert_eq!(fn_cfg(&k, "app::Cfgd::net_method"), &feat("net"));
    assert_eq!(fn_cfg(&k, "app::Cfgd::in_gated_impl"), &feat("net"));
}

#[test]
fn statements_cfg_attr_and_use_items_carry_cfg() {
    let k = features_ws();

    // On a `use` item
    let bb = k.imports.iter().find(|i| i.full_path == "opt_b::b").expect("use opt_b::b as bb");
    assert_eq!(bb.cfg, feat("net"));

    // Statement-level cfg in stmt_level
    let call = k
        .paths
        .iter()
        .find(|p| p.path == "opt_b::b" && p.enclosing_fn.as_deref() == Some("app::stmt_level"))
        .expect("opt_b::b in stmt_level");
    assert_eq!(call.cfg, feat("net"), "E3");

    // Cfg! is not a gate
    let cfg_macro = k
        .macros_inv
        .iter()
        .find(|m| m.name == "cfg" && m.enclosing_fn.as_deref() == Some("app::stmt_level"))
        .expect("cfg! in stmt_level");
    assert_eq!(cfg_macro.cfg, CfgPredicate::True, "E6");

    // The derive produced by cfg_attr
    let derive = k
        .attrs
        .iter()
        .find(|a| a.path == "derive" && a.arg_paths.iter().any(|p| p == "opt_b::Derive"))
        .expect("derive(opt_b::Derive) from cfg_attr");
    assert_eq!(derive.cfg, feat("net"), "E5");

    // The cfg attribute itself is recorded with its parsed predicate
    assert!(
        k.attrs.iter().any(|a| a.path == "cfg" && a.predicate == Some(all(vec![feat("net"), not(feat("fast"))]))),
        "E7: {:?}",
        k.attrs.iter().filter(|a| a.path == "cfg").map(|a| &a.predicate).collect::<Vec<_>>()
    );
}

#[test]
fn dep_use_sites_carry_cfg() {
    let k = features_ws();
    let sites = k.dep_use_sites_with_cfg(&set(&["opt_b"]));
    let cfg_of = |scope: &str| {
        sites
            .iter()
            .find(|(s, _)| s.scope == scope && matches!(s.kind, UseSiteKind::Path))
            .map(|(_, c)| c.clone())
            .unwrap_or_else(|| panic!("no opt_b path site in {scope}; have {sites:?}"))
    };
    assert_eq!(cfg_of("app::net_slow"), all(vec![feat("net"), not(feat("fast"))]), "Q8");
    assert_eq!(cfg_of("app::netmod::n"), feat("net"));
    assert_eq!(cfg_of("app::stmt_level"), feat("net"));
    assert_eq!(cfg_of("app::fast_only"), feat("fast"));

    let a_sites = k.dep_use_sites_with_cfg(&set(&["opt_a"]));
    assert!(a_sites.iter().any(|(s, c)| s.scope == "app::oops" && *c == CfgPredicate::True), "{a_sites:?}");
}

// ----------------------------------------------------------------
#[test]
fn implication_check() {
    let default_env = CfgEnv::with_features(["default", "fast"]);
    let opt_b = any(vec![feat("full"), feat("net")]);

    assert_eq!(feat("net").implies(&opt_b, None), Implication::Always, "Q5");
    assert_eq!(all(vec![feat("net"), not(feat("fast"))]).implies(&opt_b, None), Implication::Always);
    assert_eq!(any(vec![feat("net"), feat("full")]).implies(&opt_b, None), Implication::Always);
    assert_eq!(
        feat("fast").implies(&opt_b, Some(&default_env)),
        Implication::Sometimes { counterexample: default_env.clone() },
        "preferred counterexample first"
    );
    assert_eq!(
        CfgPredicate::True.implies(&feat("net"), Some(&default_env)),
        Implication::Sometimes { counterexample: default_env.clone() }
    );
    assert_eq!(key("test").implies(&key("test"), None), Implication::Always);
    assert_eq!(feat("net").implies(&not(feat("net")), None), Implication::Never);
    assert_eq!(CfgPredicate::Unknown("x".into()).implies(&feat("net"), None), Implication::Unknown);
    assert_eq!(feat("net").implies(&CfgPredicate::Unknown("x".into()), None), Implication::Unknown);

    // enumeration counterexample when no preferred env is given: all-false first
    match feat("a").implies(&feat("b"), None) {
        Implication::Sometimes { counterexample } => assert_eq!(counterexample, CfgEnv::with_features(["a"])),
        other => panic!("{other:?}"),
    }
}

// ---------------------------------------------------------------- The payoff

#[test]
fn ungated_dep_uses_finds_the_real_breaks() {
    let k = features_ws();
    let m = model(&k);
    let ungated = k.ungated_dep_uses();

    let found: BTreeSet<(String, String)> = ungated.iter().map(|u| (u.site.scope.clone(), u.site.dep.clone())).collect();
    let expected: BTreeSet<(String, String)> = [
        ("app::oops", "opt_a"),
        ("app::fast_only", "opt_b"),
        ("app::uses_dev", "devonly"),
    ]
    .iter()
    .map(|(s, d)| (s.to_string(), d.to_string()))
    .collect();
    assert_eq!(found, expected, "Q6: {ungated:?}");

    let by_scope = |scope: &str| ungated.iter().find(|u| u.site.scope == scope).unwrap();

    let oops = by_scope("app::oops");
    assert_eq!(oops.package, "app");
    assert_eq!(oops.site_cfg, CfgPredicate::True);
    assert_eq!(oops.dep_predicate, m.dep_predicate("opt_a"));
    assert_eq!(oops.verdict, Implication::Sometimes { counterexample: m.default_env() }, "breaks the default build");

    let fast_only = by_scope("app::fast_only");
    assert_eq!(fast_only.site_cfg, feat("fast"));
    assert_eq!(fast_only.verdict, Implication::Sometimes { counterexample: m.default_env() }, "fast is default, net is not");

    let uses_dev = by_scope("app::uses_dev");
    assert_eq!(uses_dev.dep_predicate, key("test"));
    assert!(matches!(uses_dev.verdict, Implication::Sometimes { .. }));
}

#[test]
fn items_requiring_feature() {
    let k = features_ws();
    assert_eq!(
        k.items_requiring_feature("app", "net"),
        vec![
            "app::Cfgd::in_gated_impl",
            "app::Cfgd::net_method",
            "app::either",
            "app::net_slow",
            "app::netmod::n",
            "app::netmod::n_fast",
        ],
        "Q7 (closed assignments: full ⇒ net, so `either` requires net)"
    );
    assert_eq!(k.items_requiring_feature("app", "fast"), vec!["app::fast_only", "app::netmod::n_fast"]);
    assert_eq!(k.items_requiring_feature("app", "full"), Vec::<String>::new(), "nothing needs full specifically");
    assert_eq!(k.items_requiring_feature("nope", "net"), Vec::<String>::new());
}

// ---------------------------------------------------------------- no-metadata mode

#[test]
fn folder_scan_has_no_model_but_keeps_cfg() {
    let k = features_folder();
    assert!(k.feature_model("app").is_none());
    assert!(k.ungated_dep_uses().is_empty(), "no model → no judgement");
    assert_eq!(fn_cfg(&k, "app::net_slow"), &all(vec![feat("net"), not(feat("fast"))]), "E rules need no cargo");
    assert_eq!(fn_cfg(&k, "app::netmod::n"), &feat("net"));
}

#[test]
fn unknown_cfg_is_never_treated_as_absent() {
    let k = features_ws();
    let weird = fn_cfg(&k, "app::weird");
    assert!(weird.is_unknown());
    assert_eq!(weird.implies(&feat("net"), None), Implication::Unknown);
    assert_eq!(weird.eval(&CfgEnv::default()), None);
    assert!(!k.items_requiring_feature("app", "net").contains(&"app::weird".to_string()), "never claim for Unknown");
}
