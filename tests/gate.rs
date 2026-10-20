//! Spec, gate plan, check matrix, diagnostics and the converge loop. Fixtures: tests/fixtures/plan/ and tests/fixtures/oracle/.

mod common;

use common::*;
use klepto::*;
use std::collections::BTreeSet;
use std::path::PathBuf;

fn plan_ws() -> Klepto {
    Klepto::new("app")
        .scan_workspace_root(fixture("plan"))
        .expect("cargo metadata on fixture plan")
        .parse()
        .expect("parse fixture plan")
}

fn spec() -> FeatureSpec {
    FeatureSpec::from_path(&fixture("plan").join("klepto.toml")).expect("klepto.toml")
}

fn desired(k: &Klepto) -> FeatureModel {
    k.feature_model("app").unwrap().apply_spec(spec().package("app").unwrap())
}

fn feat(s: &str) -> CfgPredicate { CfgPredicate::Feature(s.to_string()) }
fn key(s: &str) -> CfgPredicate { CfgPredicate::Key(s.to_string()) }
fn all(ps: Vec<CfgPredicate>) -> CfgPredicate { CfgPredicate::All(ps) }
fn any(ps: Vec<CfgPredicate>) -> CfgPredicate { CfgPredicate::Any(ps) }
fn not(p: CfgPredicate) -> CfgPredicate { CfgPredicate::Not(Box::new(p)) }

fn strs(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

fn gate<'a>(p: &'a GatePlan, item: &str) -> &'a GateEdit {
    p.gate_for(item).unwrap_or_else(|| panic!("no gate for {item}; gates: {:?}", p.gates.iter().map(|g| (&g.item, g.kind)).collect::<Vec<_>>()))
}

fn oracle_fixture() -> String {
    std::fs::read_to_string(fixture("oracle").join("no_default.jsonl")).unwrap()
}

// ---------------------------------------------------------------- S rules

#[test]
fn spec_file_parses() {
    let s = spec();
    let app = s.package("app").expect("app section");
    assert_eq!(app.features["a"], strs(&["dep:alpha", "dep:beta"]), "S1");
    assert!(app.optional.is_empty());
    assert_eq!(app.default, None);

    let parsed = FeatureSpec::parse("[my-crate]\noptional = [\"x\"]\ndefault = [\"f\"]\n[my-crate.features]\nf = [\"dep:x\"]\n").unwrap();
    let pkg = parsed.package("my_crate").expect("hyphen normalised");
    assert_eq!(pkg.optional, ["x".to_string()].into_iter().collect());
    assert_eq!(pkg.default, Some(strs(&["f"])));

    assert!(matches!(FeatureSpec::parse("[app\n"), Err(SpecError::Toml(_))));
    assert!(matches!(FeatureSpec::parse("[app]\nbogus = 1\n"), Err(SpecError::Toml(_))), "unknown keys are errors");
}

#[test]
fn apply_spec_rebuilds_the_desired_model() {
    let k = plan_ws();
    let current = k.feature_model("app").unwrap();
    assert!(current.features.contains_key("beta"), "cargo's implicit feature exists today");
    assert_eq!(current.features_enabling("beta"), ["beta".to_string()].into_iter().collect::<BTreeSet<_>>());

    let d = desired(&k);
    assert_eq!(d.features["a"].enables, vec![FeatureEnable::Dep("alpha".into()), FeatureEnable::Dep("beta".into())], "S2");
    assert!(!d.features.contains_key("beta"), "S4: `dep:beta` removes the implicit feature");
    assert!(!d.dep("beta").unwrap().implicit_feature);
    assert_eq!(d.features_enabling("beta"), ["a".to_string()].into_iter().collect::<BTreeSet<_>>());
    assert_eq!(d.dep_predicate("beta").simplify(), feat("a"));
    assert_eq!(d.dep_predicate("devd"), key("test"));
    assert!(d.features.contains_key("default"), "untouched features are kept");

    // Optional + default
    let extra = PackageSpec {
        features: [("newf".to_string(), strs(&["dep:devd"]))].into_iter().collect(),
        optional: ["devd".to_string()].into_iter().collect(),
        default: Some(strs(&["newf"])),
    };
    let d2 = current.apply_spec(&extra);
    assert!(d2.dep("devd").unwrap().optional);
    assert_eq!(d2.features["default"].enables, vec![FeatureEnable::Feature("newf".into())]);
    assert_eq!(
        d2.features_enabling("devd"),
        ["default".to_string(), "newf".to_string()].into_iter().collect::<BTreeSet<_>>(),
        "transitive: default → newf → dep:devd"
    );
}

#[test]
fn manifest_edits_diff() {
    let k = plan_ws();
    let current = k.feature_model("app").unwrap();
    let d = desired(&k);
    assert_eq!(
        manifest_edits(current, &d),
        vec![ManifestEdit::SetFeature { name: "a".into(), enables: strs(&["dep:alpha", "dep:beta"]) }],
        "S5: beta was already optional, default unchanged"
    );

    let extra = PackageSpec {
        features: [("newf".to_string(), strs(&["dep:devd", "a"]))].into_iter().collect(),
        optional: ["devd".to_string()].into_iter().collect(),
        default: Some(strs(&["newf"])),
    };
    let d2 = current.apply_spec(&extra);
    assert_eq!(
        manifest_edits(current, &d2),
        vec![
            ManifestEdit::MakeOptional { dep: "devd".into() },
            ManifestEdit::SetFeature { name: "newf".into(), enables: strs(&["dep:devd", "a"]) },
            ManifestEdit::SetDefault { features: strs(&["newf"]) },
        ],
        "S5 ordering"
    );
    assert!(manifest_edits(current, current).is_empty());
}

#[test]
fn a_feature_named_like_the_dependency_it_turns_on_is_written() {
    // `devd = ["dep:devd"]`: the spec declares it, so it is no implicit feature and the manifest gets it
    let k = plan_ws();
    let current = k.feature_model("app").unwrap();
    let named = PackageSpec {
        features: [("devd".to_string(), strs(&["dep:devd"]))].into_iter().collect(),
        optional: ["devd".to_string()].into_iter().collect(),
        default: None,
    };
    let d = current.apply_spec(&named);
    assert!(!d.features["devd"].implicit, "declared by the spec");
    assert!(!d.dep("devd").unwrap().implicit_feature);
    assert_eq!(
        manifest_edits(current, &d),
        vec![ManifestEdit::MakeOptional { dep: "devd".into() }, ManifestEdit::SetFeature { name: "devd".into(), enables: strs(&["dep:devd"]) }]
    );
    assert_eq!(d.features_enabling("devd"), ["devd".to_string()].into_iter().collect::<BTreeSet<_>>());
}

#[test]
fn a_dashed_dependencys_implicit_feature_keeps_the_manifests_name() {
    // cargo metadata lists `shop-marks = ["dep:shop-marks"]` for an optional `shop-marks` no feature names: it is
    // cargo's implicit feature (not a declared one), under the dependency's name as the manifest writes it
    let dep = |optional: bool| DepInfo { name: "shop_marks".into(), package: "shop-marks".into(), optional, kind: DepKind::Normal, target: None, default_features: true, features: Vec::new(), implicit_feature: false };
    let mut read = FeatureModel::default();
    read.package = "shop".into();
    read.features.insert("shop-marks".into(), FeatureInfo { enables: vec![FeatureEnable::Dep("shop_marks".into())], implicit: false });
    read.dependencies = vec![dep(true)];
    let m = read.apply_spec(&PackageSpec::default());
    assert!(m.features["shop-marks"].implicit, "{:?}", m.features);
    assert!(!m.features.contains_key("shop_marks"));
    assert!(m.dep("shop_marks").unwrap().implicit_feature);
    // required again: the implicit feature goes with it
    let mut required = m.clone();
    required.dependencies = vec![dep(false)];
    assert!(required.apply_spec(&PackageSpec::default()).features.is_empty());
    // declared by a spec under that name: written, not implicit
    let named = PackageSpec { features: [("shop-marks".to_string(), strs(&["dep:shop-marks"]))].into_iter().collect(), ..Default::default() };
    let d = m.apply_spec(&named);
    assert!(!d.features["shop-marks"].implicit && !d.dep("shop_marks").unwrap().implicit_feature);
}

// ---------------------------------------------------------------- C rules

#[test]
fn predicate_rendering_and_simplify() {
    assert_eq!(feat("a").render(), r#"feature = "a""#, "C7");
    assert_eq!(all(vec![feat("a"), not(feat("b"))]).render(), r#"all(feature = "a", not(feature = "b"))"#);
    assert_eq!(any(vec![key("unix"), CfgPredicate::KeyValue("target_os".into(), "linux".into())]).render(), r#"any(unix, target_os = "linux")"#);
    assert_eq!(any(vec![feat("a")]).render(), r#"feature = "a""#, "single-element Any");
    assert_eq!(CfgPredicate::True.render(), "all()");

    assert_eq!(any(vec![feat("a")]).simplify(), feat("a"), "C8");
    assert_eq!(all(vec![CfgPredicate::True, feat("a"), all(vec![feat("b"), feat("a")])]).simplify(), all(vec![feat("a"), feat("b")]));
    assert_eq!(any(vec![feat("a"), CfgPredicate::True]).simplify(), CfgPredicate::True);
    assert_eq!(all(vec![]).simplify(), CfgPredicate::True);

    let g = GateEdit {
        item: "x".into(),
        kind: GateKind::Item,
        location: FileLocation { path: PathBuf::new(), line: None, column: None },
        existing_cfg: CfgPredicate::True,
        predicate: all(vec![feat("a"), key("test")]),
        reasons: vec![],
    };
    assert_eq!(g.attribute_text(), r#"#[cfg(all(feature = "a", test))]"#, "P9");
}

// ---------------------------------------------------------------- P rules

#[test]
fn plan_proposes_gates_for_ungated_uses() {
    let k = plan_ws();
    let d = desired(&k);
    let p = k.plan_gates("app", &d);
    assert_eq!(p.package, "app");
    assert_eq!(p.manifest, vec![ManifestEdit::SetFeature { name: "a".into(), enables: strs(&["dep:alpha", "dep:beta"]) }], "P2");

    let open = gate(&p, "app::open");
    assert_eq!(open.kind, GateKind::Item);
    assert_eq!(open.predicate, feat("a"), "P3/P4");
    assert_eq!(open.existing_cfg, CfgPredicate::True);
    assert_eq!(open.reasons, vec![GateReason::UngatedDep { dep: "alpha".into() }]);
    assert_eq!(open.location.line, Some(9));

    assert_eq!(gate(&p, "app::inner::deep").predicate, feat("a"));
    assert_eq!(gate(&p, "app::Holder").reasons, vec![GateReason::UngatedDep { dep: "beta".into() }], "a struct is a carrier");
    assert_eq!(gate(&p, "app::Holder::get").predicate, feat("a"), "a method is a carrier");
    assert_eq!(gate(&p, "app::private_user").predicate, feat("a"));

    // the module-level `use beta::early` is gated as a Use
    let use_gate = p.gates.iter().find(|g| g.kind == GateKind::Use).expect("Use gate");
    assert_eq!(use_gate.item, "app");
    assert_eq!(use_gate.location.line, Some(7));
    assert_eq!(use_gate.predicate, feat("a"));
    assert_eq!(use_gate.reasons, vec![GateReason::UngatedDep { dep: "beta".into() }]);

    for untouched in ["app::fine", "app::gated_target", "app::ok_dev", "app::weird"] {
        assert!(p.gate_for(untouched).is_none(), "{untouched} must not be gated: {:?}", p.gate_for(untouched));
    }
}

#[test]
fn plan_merges_requirements_on_one_carrier() {
    let k = plan_ws();
    let p = k.plan_gates("app", &desired(&k));
    let both = gate(&p, "app::both");
    assert_eq!(both.predicate, all(vec![feat("a"), key("test")]), "P7");
    assert!(both.reasons.contains(&GateReason::UngatedDep { dep: "alpha".into() }));
    assert!(both.reasons.contains(&GateReason::UngatedDep { dep: "devd".into() }));
    assert_eq!(both.attribute_text(), r#"#[cfg(all(feature = "a", test))]"#);
    assert_eq!(p.gates.iter().filter(|g| g.item == "app::both").count(), 1, "one edit per carrier");
}

#[test]
fn plan_cascades_through_dependents() {
    let k = plan_ws();
    let p = k.plan_gates("app", &desired(&k));
    assert_eq!(gate(&p, "app::calls_open").predicate, feat("a"), "P5");
    assert_eq!(gate(&p, "app::calls_open").reasons, vec![GateReason::Cascade { from: "app::open".into() }]);
    assert_eq!(gate(&p, "app::calls_calls_open").reasons, vec![GateReason::Cascade { from: "app::calls_open".into() }], "second level");
    assert_eq!(gate(&p, "app::uses_holder").reasons, vec![GateReason::Cascade { from: "app::Holder".into() }]);
    assert_eq!(gate(&p, "app::<impl Holder>").reasons, vec![GateReason::Cascade { from: "app::Holder".into() }], "the impl block references Holder");
}

#[test]
fn plan_folds_existing_violations() {
    let k = plan_ws();
    let p = k.plan_gates("app", &desired(&k));
    let g = gate(&p, "app::calls_gated");
    assert_eq!(g.predicate, feat("a"), "P6");
    assert_eq!(g.reasons, vec![GateReason::ExistingViolation { to: "app::gated_target".into() }]);
}

#[test]
fn plan_reports_unknown_and_public_reach() {
    let k = plan_ws();
    let p = k.plan_gates("app", &desired(&k));
    assert!(p.unknown.iter().any(|u| u.item == "app::weird"), "P10: {:?}", p.unknown);
    assert_eq!(
        p.reaches_public,
        strs(&[
            "app::Holder",
            "app::Holder::get",
            "app::both",
            "app::calls_calls_open",
            "app::calls_gated",
            "app::calls_open",
            "app::inner::deep",
            "app::open",
            "app::uses_holder",
        ]),
        "P9b: private_user, the impl block and the use are not public"
    );
}

#[test]
fn plan_gates_are_ordered_by_location() {
    let k = plan_ws();
    let p = k.plan_gates("app", &desired(&k));
    assert!(p.gates.len() >= 10, "{}", p.gates.len());
    let keys: Vec<(PathBuf, Option<u32>, Option<u32>)> =
        p.gates.iter().map(|g| (g.location.path.clone(), g.location.line, g.location.column)).collect();
    for w in keys.windows(2) {
        assert!(w[0] <= w[1], "P8: {:?} before {:?}", w[0], w[1]);
    }
}

#[test]
fn forced_gates_are_included() {
    let k = plan_ws();
    let forced = vec![ForcedGate {
        item: "app::ok_dev".into(),
        predicate: feat("a"),
        reason: GateReason::Forced { code: "E0433".into(), mention: "alpha".into() },
    }];
    let p = k.plan_gates_with("app", &desired(&k), &forced);
    let g = gate(&p, "app::ok_dev");
    assert_eq!(g.predicate, feat("a"), "P11");
    assert_eq!(g.reasons, vec![GateReason::Forced { code: "E0433".into(), mention: "alpha".into() }]);
    assert_eq!(g.location.line, Some(16));
}

// ---------------------------------------------------------------- V rules

#[test]
fn check_matrix_from_desired_model() {
    let k = plan_ws();
    let jobs = check_matrix(&desired(&k));
    let set: BTreeSet<(Vec<String>, bool)> = jobs.iter().map(|j| (j.features.clone(), j.no_default)).collect();
    let expected: BTreeSet<(Vec<String>, bool)> =
        [(vec![], true), (vec![], false), (strs(&["a"]), true), (strs(&["a"]), false)].into_iter().collect();
    assert_eq!(set, expected, "V2: subsets of {{default, a}} deduplicated by closure");
    assert_eq!(jobs.len(), 4);
    assert!(jobs.iter().all(|j| j.package == "app"));

    let j = CheckJob { package: "app".into(), features: strs(&["a"]), no_default: true };
    assert_eq!(j.cargo_args(), strs(&["check", "-p", "app", "--message-format=json", "--no-default-features", "--features", "a"]), "V3");
    assert_eq!(j.label(), "app[no-default+a]");
    assert_eq!(CheckJob { package: "app".into(), features: vec![], no_default: false }.label(), "app[default]");
}

// ---------------------------------------------------------------- X rules

#[test]
fn parse_cargo_json_keeps_errors_with_primary_span() {
    let diags = parse_cargo_json(&oracle_fixture());
    assert_eq!(diags.len(), 1, "X1: warnings and non-message lines are skipped: {diags:?}");
    let d = &diags[0];
    assert_eq!(d.code.as_deref(), Some("E0433"));
    assert_eq!(d.level, "error");
    assert_eq!(d.mention.as_deref(), Some("alpha"), "X2");
    assert_eq!(d.file.as_deref(), Some(std::path::Path::new("app/src/lib.rs")));
    assert_eq!(d.line, Some(16));
    assert_eq!(d.column, Some(18));
    assert!(parse_cargo_json("not json\n{\"reason\":\"build-finished\",\"success\":true}\n").is_empty());
}

#[test]
fn map_diagnostic_to_item_and_predicate() {
    let k = plan_ws();
    let d = desired(&k);
    let diag = parse_cargo_json(&oracle_fixture()).remove(0);
    let m = k.map_diagnostic(&fixture("plan"), "app", &d, &diag);
    assert_eq!(m.item.as_deref(), Some("app::ok_dev"), "X3");
    assert_eq!(m.predicate, Some(feat("a")));

    let local = Diagnostic { mention: Some("gated_target".into()), ..diag.clone() };
    assert_eq!(k.map_diagnostic(&fixture("plan"), "app", &d, &local).predicate, Some(feat("a")), "a gated local item");

    let unknown = Diagnostic { mention: Some("mystery".into()), ..diag.clone() };
    let m2 = k.map_diagnostic(&fixture("plan"), "app", &d, &unknown);
    assert_eq!(m2.item.as_deref(), Some("app::ok_dev"));
    assert_eq!(m2.predicate, None);
}

// ---------------------------------------------------------------- L rules

#[test]
fn converge_learns_from_the_oracle() {
    let k = plan_ws();
    let d = desired(&k);
    let recorded = oracle_fixture();
    let mut round = 0usize;
    let mut seen_jobs = Vec::new();
    let mut oracle = FnOracle(|job: &CheckJob, plan: &GatePlan| {
        if job.no_default && job.features.is_empty() {
            round += 1;
        }
        seen_jobs.push(job.label());
        // round 1: the no-default build blames ok_dev; the plan must not already gate it
        if round == 1 && job.no_default && job.features.is_empty() {
            assert!(plan.gate_for("app::ok_dev").is_none());
            return OracleResult::failed(parse_cargo_json(&recorded));
        }
        OracleResult::passed()
    });
    let c = k.converge(&fixture("plan"), "app", &d, &mut oracle, 5);
    assert_eq!(c.status, ConvergeStatus::Converged, "L2");
    assert_eq!(c.rounds.len(), 2);
    assert_eq!(c.forced.len(), 1);
    assert_eq!(c.forced[0].item, "app::ok_dev");
    let g = gate(&c.plan, "app::ok_dev");
    assert_eq!(g.predicate, feat("a"));
    assert_eq!(g.reasons, vec![GateReason::Forced { code: "E0433".into(), mention: "alpha".into() }]);
    assert!(c.human.is_empty());
    assert_eq!(c.rounds[0].missed.len(), 1);
    assert!(c.rounds[1].results.iter().all(|r| r.ok));
    assert_eq!(seen_jobs.len(), 8, "4 jobs × 2 rounds");
}

#[test]
fn converge_gives_up_on_unmappable_errors() {
    let k = plan_ws();
    let d = desired(&k);
    let mut oracle = FnOracle(|_job: &CheckJob, _plan: &GatePlan| {
        OracleResult::failed(vec![Diagnostic {
            code: Some("E0425".into()),
            level: "error".into(),
            message: "cannot find value `mystery` in this scope".into(),
            mention: Some("mystery".into()),
            file: Some(PathBuf::from("app/src/lib.rs")),
            line: Some(16),
            column: Some(18),
        }])
    });
    let c = k.converge(&fixture("plan"), "app", &d, &mut oracle, 5);
    assert_eq!(c.status, ConvergeStatus::GaveUp, "L2: nothing actionable");
    assert_eq!(c.rounds.len(), 1);
    assert!(c.forced.is_empty());
    assert!(!c.human.is_empty());
    assert_eq!(c.human[0].item.as_deref(), Some("app::ok_dev"));
    assert_eq!(c.human[0].predicate, None);
}

#[test]
fn converge_respects_max_rounds() {
    let k = plan_ws();
    let d = desired(&k);
    let recorded = oracle_fixture();
    // an oracle that keeps failing the same way even after the gate is forced
    let mut oracle = FnOracle(|job: &CheckJob, _plan: &GatePlan| {
        if job.no_default && job.features.is_empty() {
            OracleResult::failed(parse_cargo_json(&recorded))
        } else {
            OracleResult::passed()
        }
    });
    let c = k.converge(&fixture("plan"), "app", &d, &mut oracle, 3);
    assert_eq!(c.status, ConvergeStatus::GaveUp);
    assert!(c.rounds.len() <= 3);
    assert_eq!(c.forced.len(), 1, "the same forced gate is not added twice");
}

#[test]
fn a_use_under_cfg_attr_of_the_feature_is_not_lost_without_it() {
    let k = Klepto::new("app").scan_workspace_root(fixture("cfgattr")).expect("cargo metadata on fixture cfgattr").parse().expect("parse");
    let a = k.affected_by_feature("app", "x");
    assert_eq!(a.gated, ["app::gone"]);
    assert_eq!(a.uses_lost_dep, ["app::uses"], "Kept's derive exists only with x on: Kept stays");
}
