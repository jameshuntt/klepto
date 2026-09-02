//! Phase 7 acceptance tests: pattern mining.
//! Rules: PHASE7_RULES.md. Fixture: tests/fixtures/patterns/.
//! Run with `cargo test --test phase7`.

mod common;

use common::*;
use klepto::*;
use serde_json::Value;

fn patterns_ws() -> Klepto {
    Klepto::new("app")
        .scan_workspace_root(fixture("patterns"))
        .expect("cargo metadata on fixture patterns")
        .parse()
        .expect("parse fixture patterns")
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn cluster<'a>(r: &'a PatternReport, level: ShapeLevel, member: &str) -> &'a Cluster {
    r.clusters
        .iter()
        .find(|c| c.level == level && c.members.iter().any(|m| m == member))
        .unwrap_or_else(|| panic!("no {level:?} cluster containing {member}; have {:?}", r.clusters))
}

fn candidate<'a>(r: &'a PatternReport, member: &str) -> &'a Candidate {
    r.candidates
        .iter()
        .find(|c| c.members.iter().any(|m| m == member))
        .unwrap_or_else(|| panic!("no candidate containing {member}; have {:?}", r.candidates))
}

fn s(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_else(|| panic!("no string `{key}` in {v}")).to_string()
}

const ENGINE_DESCRIBE: &str = "app::Engine::describe";
const MOTOR_DESCRIBE: &str = "app::Motor::describe";

// ---------------------------------------------------------------- S rules

#[test]
fn shapes_have_five_levels_and_hashes() {
    let k = patterns_ws();
    let r = k.patterns();
    let sh = r.shape("app::sum_a").expect("S1: sum_a is shaped");
    assert_eq!(sh.shapes.len(), 5);
    assert_eq!(sh.hashes.len(), 5);
    for level in SHAPE_LEVELS {
        assert_eq!(sh.hashes[&level].len(), 64, "S5: blake3 hex at {level:?}");
        assert!(!sh.shapes[&level].is_empty());
    }
    assert!(sh.tokens >= 12, "{}", sh.tokens);
    // S2: tokens joined by single spaces, `::` split, delimiters present
    let exact = &sh.shapes[&ShapeLevel::Exact];
    assert!(exact.contains("for x in xs {"), "{exact}");
    assert!(exact.contains("( xs : & [ u32 ] ) -> u32"), "S1: the signature is part of the shape: {exact}");
    assert!(!exact.contains("  "));
    // S3: renamed
    let renamed = &sh.shapes[&ShapeLevel::Renamed];
    assert!(renamed.contains("for $v2 in $v0 {"), "{renamed}");
    assert!(renamed.contains("let mut $v1 = 0"), "{renamed}");
    assert_eq!(sh.slots[&ShapeLevel::Renamed], vec![
        Slot { placeholder: "$v0".into(), filler: "xs".into() },
        Slot { placeholder: "$v1".into(), filler: "t".into() },
        Slot { placeholder: "$v2".into(), filler: "x".into() },
    ], "S4");
    // S3: literals
    let literals = &sh.shapes[&ShapeLevel::Literals];
    assert!(literals.contains("let mut $v1 = $l0"), "{literals}");
    assert_eq!(sh.slots[&ShapeLevel::Literals], vec![Slot { placeholder: "$l0".into(), filler: "0".into() }]);
    // S3: types — primitives are types too
    let types = &sh.shapes[&ShapeLevel::Types];
    assert!(types.contains("( $v0 : & [ $t0 ] ) -> $t0"), "{types}");
    assert_eq!(sh.slots[&ShapeLevel::Types], vec![Slot { placeholder: "$t0".into(), filler: "u32".into() }]);
    // S3: callees — sum_a calls nothing, so Callees == Types
    assert_eq!(sh.shapes[&ShapeLevel::Callees], sh.shapes[&ShapeLevel::Types]);
    assert!(sh.slots[&ShapeLevel::Callees].is_empty());
    // a method's `self` survives every level
    let d = r.shape(ENGINE_DESCRIBE).expect("describe is shaped");
    assert!(d.shapes[&ShapeLevel::Callees].contains("self . name"), "{}", d.shapes[&ShapeLevel::Callees]);
}

#[test]
fn exact_and_renamed_clusters() {
    let r = patterns_ws().patterns();
    let exact = cluster(&r, ShapeLevel::Exact, "app::sum_a");
    assert_eq!(exact.members, strs(&["app::sum_a", "app::sum_b"]), "C1");
    assert_eq!(exact.exemplar, "app::sum_a");
    assert!(exact.tightest, "C2");
    assert!(exact.params.is_empty(), "C4");

    let renamed = cluster(&r, ShapeLevel::Renamed, "app::sum_c");
    assert_eq!(renamed.members, strs(&["app::sum_a", "app::sum_b", "app::sum_c"]));
    assert!(renamed.tightest, "C2: no stricter level joins all three");
    assert!(renamed.params.is_empty());

    // the looser levels also contain the trio, but are not tightest
    for level in [ShapeLevel::Literals, ShapeLevel::Types, ShapeLevel::Callees] {
        let c = cluster(&r, level, "app::sum_c");
        assert_eq!(c.members, strs(&["app::sum_a", "app::sum_b", "app::sum_c"]));
        assert!(!c.tightest, "{level:?}");
    }
    // C3 order
    let keys: Vec<(ShapeLevel, std::cmp::Reverse<usize>, String)> = r.clusters.iter().map(|c| (c.level, std::cmp::Reverse(c.members.len()), c.hash.clone())).collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
    assert_eq!(r.clusters_of("app::sum_a").len(), 5);
}

// ---------------------------------------------------------------- P rules

#[test]
fn literal_clusters_yield_parameters() {
    let r = patterns_ws().patterns();
    let c = cluster(&r, ShapeLevel::Literals, "app::scale_a");
    assert_eq!(c.members, strs(&["app::scale_a", "app::scale_b"]));
    assert!(c.tightest);
    assert_eq!(
        c.params,
        vec![
            Param { kind: ParamKind::Literal, placeholder: "$l0".into(), fillers: strs(&["2", "3"]) },
            Param { kind: ParamKind::Literal, placeholder: "$l1".into(), fillers: strs(&["1", "7"]) },
        ],
        "P1"
    );
    assert!(!r.clusters.iter().any(|x| x.level == ShapeLevel::Renamed && x.members.contains(&"app::scale_a".to_string())));
    let cand = candidate(&r, "app::scale_a");
    assert_eq!(cand.kind, CandidateKind::Parameterize, "P2");
    assert_eq!(cand.level, ShapeLevel::Literals);
    assert_eq!(cand.params.len(), 2);
    assert!(!cand.note.is_empty());
}

#[test]
fn type_clusters_yield_generic_candidates_with_required_methods() {
    let r = patterns_ws().patterns();
    let c = cluster(&r, ShapeLevel::Types, "app::check_engine");
    assert_eq!(c.members, strs(&["app::check_engine", "app::check_motor"]));
    assert!(c.tightest);
    assert_eq!(c.params.len(), 1, "P1: only the differing type slot is a parameter: {:?}", c.params);
    assert_eq!(c.params[0].kind, ParamKind::Type);
    assert_eq!(c.params[0].fillers, strs(&["Engine", "Motor"]));
    let cand = candidate(&r, "app::check_engine");
    assert_eq!(cand.kind, CandidateKind::GenericFn);
    assert_eq!(cand.required_methods.get(&c.params[0].placeholder), Some(&strs(&["ready"])), "P3: {:?}", cand.required_methods);
}

#[test]
fn same_named_methods_on_different_types_yield_trait_candidate() {
    let r = patterns_ws().patterns();
    let c = cluster(&r, ShapeLevel::Exact, ENGINE_DESCRIBE);
    assert_eq!(c.members, strs(&[ENGINE_DESCRIBE, MOTOR_DESCRIBE]), "C6");
    let cand = candidate(&r, ENGINE_DESCRIBE);
    assert_eq!(cand.kind, CandidateKind::Trait, "P2 precedence 1");
    assert_eq!(cand.method.as_deref(), Some("describe"));
    assert_eq!(cand.types, strs(&["app::Engine", "app::Motor"]));
}

#[test]
fn callee_clusters_yield_macro_candidates() {
    let r = patterns_ws().patterns();
    let c = cluster(&r, ShapeLevel::Callees, "app::apply_a");
    assert_eq!(c.members, strs(&["app::apply_a", "app::apply_b"]));
    assert!(c.tightest);
    let callee: Vec<&Param> = c.params.iter().filter(|p| p.kind == ParamKind::Callee).collect();
    assert_eq!(callee.len(), 1, "{:?}", c.params);
    assert_eq!(callee[0].fillers, strs(&["double", "triple"]));
    assert!(c.params.iter().all(|p| p.kind == ParamKind::Callee), "types and literals agree: {:?}", c.params);
    let cand = candidate(&r, "app::apply_a");
    assert_eq!(cand.kind, CandidateKind::MacroRules);
    // P4 weight and order
    assert_eq!(cand.weight, 2 * r.shape("app::apply_a").unwrap().tokens);
    let weights: Vec<usize> = r.candidates.iter().map(|c| c.weight).collect();
    let mut sorted = weights.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(weights, sorted);
}

#[test]
fn tiny_bodies_are_skipped_unless_asked() {
    let k = patterns_ws();
    let r = k.patterns();
    assert!(r.shape("app::double").is_none(), "S5");
    assert!(!r.clusters.iter().any(|c| c.members.contains(&"app::double".to_string())));
    assert!(r.metrics.contains_key("app::double"), "M3: metrics regardless");

    let loose = patterns::mine(&k, &MiningOptions { min_tokens: 1, min_members: 2 });
    let c = cluster(&loose, ShapeLevel::Literals, "app::double");
    assert_eq!(c.members, strs(&["app::double", "app::triple"]));
    assert_eq!(c.params[0].fillers, strs(&["2", "3"]));
    assert!(!loose.clusters.iter().any(|c| c.members.contains(&"app::Engine::stop".to_string())), "S5: empty bodies never cluster");
}

// ---------------------------------------------------------------- I rules

#[test]
fn shared_method_sets_are_maximal() {
    let r = patterns_ws().patterns();
    assert_eq!(
        r.shared_method_sets,
        vec![
            SharedMethodSet { methods: strs(&["new", "ready"]), types: strs(&["app::Engine", "app::Motor", "app::Pump"]) },
            SharedMethodSet { methods: strs(&["describe", "new", "ready", "stop"]), types: strs(&["app::Engine", "app::Motor"]) },
        ],
        "I1"
    );
}

#[test]
fn repeated_trait_impls() {
    let r = patterns_ws().patterns();
    assert_eq!(
        r.repeated_impls,
        vec![RepeatedImpl {
            trait_id: "app::Named".into(),
            types: strs(&["app::Engine", "app::Motor"]),
            impls: strs(&["app::<impl app::Named for Engine>", "app::<impl app::Named for Motor>"]),
            level: ShapeLevel::Exact,
        }],
        "I2"
    );
}

// ---------------------------------------------------------------- M rules

#[test]
fn metrics() {
    let r = patterns_ws().patterns();
    let m = &r.metrics["app::complex"];
    assert_eq!(m.branches, 4, "M1: for + if + && + (2 arms - 1)");
    assert_eq!(m.calls, 0);
    assert_eq!(m.max_depth, 3);
    assert_eq!(m.lines, 14);
    assert!(m.tokens > 20);
    assert_eq!(m.fan_in, 0);

    let a = &r.metrics["app::apply_a"];
    assert_eq!(a.calls, 4, "iter, map, collect, double");
    assert_eq!(a.max_depth, 0);
    assert!(a.fan_out >= 1, "M2: double");
    assert_eq!(r.metrics["app::double"].fan_in, 1);
    assert_eq!(r.metrics["app::double"].lines, 1);
    assert_eq!(r.metrics.len(), patterns_ws().functions.len(), "M3");
}

// ---------------------------------------------------------------- determinism, KG, rules

#[test]
fn report_is_deterministic() {
    let k = patterns_ws();
    let a = k.patterns().to_json_string();
    let b = patterns_ws().patterns().to_json_string();
    assert_eq!(a, b, "C5");
    assert!(a.contains("\"options\""));
}

#[test]
fn kg_exports_patterns_realizes_and_same_shape_challenges() {
    let k = patterns_ws();
    let b = kg::export(&k);
    let patterns: Vec<&Value> = b.knowledge.iter().filter(|r| s(r, "kind") == "pattern").collect();
    assert!(!patterns.is_empty(), "K10");
    let trio = patterns
        .iter()
        .find(|p| s(p, "level") == "renamed" && p["support"] == 3)
        .unwrap_or_else(|| panic!("no renamed pattern of support 3 in {patterns:?}"));
    assert!(s(trio, "id").starts_with("pattern/renamed/"));
    assert_eq!(s(trio, "id").len(), "pattern/renamed/".len() + 16);
    assert_eq!(s(trio, "exemplar"), "app::sum_a");
    assert!(s(trio, "signature").contains("$v0"));
    assert_eq!(s(trio, "hash").len(), 64);
    let realizes = b
        .knowledge
        .iter()
        .find(|r| s(r, "kind") == "realizes" && s(r, "from") == "item/app::sum_c" && s(r, "to") == s(trio, "id"));
    assert!(realizes.is_some(), "K10 realizes edge");
    assert!(kg::NODE_KINDS.contains(&"pattern") && kg::EDGE_KINDS.contains(&"realizes"));
    // only tightest clusters become patterns: scale_a's literals cluster yes, its types cluster no
    assert!(patterns.iter().any(|p| s(p, "level") == "literals" && s(p, "exemplar") == "app::scale_a"));
    assert!(!patterns.iter().any(|p| s(p, "level") == "types" && s(p, "exemplar") == "app::scale_a"));

    let same = b
        .challenges
        .iter()
        .find(|c| s(c, "class") == "same_shape" && s(c, "item") == "app::sum_a" && s(c, "level") == "renamed")
        .expect("same_shape challenge");
    let expected: Vec<String> = serde_json::from_str(&s(same, "expected")).unwrap();
    assert_eq!(expected, strs(&["app::sum_b", "app::sum_c"]));
    assert_eq!(s(same, "split"), kg::split_for(&s(same, "digest")));
    assert!(kg::CHALLENGE_CLASSES.contains(&"same_shape"));
}

#[test]
fn rules_report_duplicates_and_repeated_impls() {
    let k = patterns_ws();
    let findings = RuleRunner::new(&k).with_all_rules().run();
    let dups: Vec<_> = findings.iter().filter(|f| f.code == "KLEP016").collect();
    assert_eq!(dups.len(), 3, "R1: exact sums, renamed sums, exact describes: {dups:?}");
    let renamed = dups.iter().find(|f| f.extra["level"] == "renamed").expect("renamed duplicate");
    assert_eq!(renamed.extra["members"], serde_json::json!(["app::sum_a", "app::sum_b", "app::sum_c"]));
    assert_eq!(renamed.location.line, Some(33), "at the exemplar");
    assert!(dups.iter().all(|f| f.severity == Severity::Info));

    let rep: Vec<_> = findings.iter().filter(|f| f.code == "KLEP017").collect();
    assert_eq!(rep.len(), 1, "R2");
    assert_eq!(rep[0].extra["trait"], "app::Named");
    assert_eq!(rep[0].extra["types"], serde_json::json!(["app::Engine", "app::Motor"]));
    let codes: Vec<&str> = rules::all_codes().iter().map(|(c, _)| *c).collect();
    assert!(codes.contains(&"KLEP016") && codes.contains(&"KLEP017"));
}
