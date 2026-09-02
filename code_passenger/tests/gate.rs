//! Consumer-side acceptance tests: gate, apply, oracle, feature commands,
//! receipt, lint, patterns. Rules: GATE_RULES.md. Fixture: tests/fixtures/gate/.
//! Run with `cargo test --test gate` (the oracle tests run real cargo, offline).

mod common;

use code_passenger::commands::{features, gate as gate_cmd, lint, patterns, receipt};
use code_passenger::gate::{apply, scratch_copy};
use code_passenger::oracle::CargoOracle;
use common::*;
use klepto::{CfgPredicate, ConvergeStatus, GateKind, ManifestEdit, Oracle, Receipt};
use std::path::Path;

const APP_LIB: &str = "app/src/lib.rs";

fn feat(s: &str) -> CfgPredicate { CfgPredicate::Feature(s.to_string()) }

fn opts(root: &Path) -> gate_cmd::GateOptions {
    gate_cmd::GateOptions {
        spec: root.join("klepto.toml"),
        package: Some("app".into()),
        write: false,
        check: false,
        max_rounds: 3,
        no_cargo: true,
    }
}

// ---------------------------------------------------------------- G / A rules

#[test]
fn plan_for_fixture() {
    let root = fixture("gate");
    let (_k, planned) = gate_cmd::plan(&root, Some("app"), &root.join("klepto.toml")).expect("G2");
    assert_eq!(planned.package, "app");
    assert!(planned.plan.manifest.contains(&ManifestEdit::MakeOptional { dep: "beta".into() }), "{:?}", planned.plan.manifest);
    assert!(planned.plan.manifest.contains(&ManifestEdit::SetFeature { name: "b".into(), enables: vec!["dep:beta".into()] }));
    let g = planned.plan.gate_for("app::uses_beta").expect("uses_beta gated");
    assert_eq!(g.kind, GateKind::Item);
    assert_eq!(g.predicate, feat("b"));
    assert_eq!(g.location.line, Some(6), "the carrier starts at its doc comment (an attribute)");
    let u = planned.plan.gate_for("app::unix_beta").expect("unix_beta gated");
    assert_eq!(u.existing_cfg, CfgPredicate::Key("unix".into()));
    assert_eq!(u.predicate, feat("b"), "the planner's predicate is what to add; the consumer merges");
    assert_eq!(code_passenger::gate::merged_attribute(&u.existing_cfg, &u.predicate), "#[cfg(all(unix, feature = \"b\"))]");
    assert!(planned.plan.gate_for("app::calls_uses_beta").is_some(), "cascade");
    assert!(planned.plan.gate_for("app::plain").is_none());
    assert!(planned.plan.unknown.is_empty());
    assert!(planned.desired.dep("beta").unwrap().optional);
    assert!(!planned.current.dep("beta").unwrap().optional);
}

#[test]
fn apply_produces_expected_edits_without_touching_the_tree() {
    let root = fixture("gate");
    let before_lib = read(&root.join(APP_LIB));
    let before_manifest = read(&root.join("app/Cargo.toml"));
    let (_k, planned) = gate_cmd::plan(&root, Some("app"), &root.join("klepto.toml")).unwrap();
    let edits = apply(&root, &planned.plan).expect("A3");
    assert!(!edits.is_empty());

    let lib = edits.files.iter().find(|f| f.path.ends_with(APP_LIB)).expect("lib.rs edited");
    assert!(lib.after.contains("#[cfg(feature = \"b\")]\npub fn uses_beta()"), "A1 insert: {}", lib.after);
    assert!(lib.after.contains("#[cfg(feature = \"b\")]\npub fn calls_uses_beta()"), "cascade gate: {}", lib.after);
    assert!(!lib.after.contains("#[cfg(feature = \"b\")]\npub fn plain()"));

    let manifest = edits.manifest.as_ref().expect("A2 manifest edit");
    assert!(manifest.path.ends_with("app/Cargo.toml"));
    assert!(manifest.after.contains("optional = true"), "{}", manifest.after);
    assert!(manifest.after.contains("b = [\"dep:beta\"]"), "{}", manifest.after);
    assert!(manifest.after.contains("path = \"../beta\""), "formatting kept: {}", manifest.after);

    let diff = edits.diff();
    assert!(diff.contains("+#[cfg(feature = \"b\")]"), "{diff}");
    assert!(diff.contains("+optional = true") || diff.contains("optional = true"), "{diff}");

    assert_eq!(read(&root.join(APP_LIB)), before_lib, "A3: apply is pure");
    assert_eq!(read(&root.join("app/Cargo.toml")), before_manifest);
}

#[test]
fn apply_merges_an_existing_cfg_instead_of_stacking() {
    let root = fixture("gate");
    let (_k, planned) = gate_cmd::plan(&root, Some("app"), &root.join("klepto.toml")).unwrap();
    let edits = apply(&root, &planned.plan).unwrap();
    let lib = &edits.files.iter().find(|f| f.path.ends_with(APP_LIB)).unwrap().after;
    assert!(lib.contains("#[cfg(all(unix, feature = \"b\"))]\npub fn unix_beta()"), "A1 replace: {lib}");
    assert!(!lib.contains("#[cfg(unix)]"), "the old attribute is gone: {lib}");
    let stacked = lib.lines().zip(lib.lines().skip(1)).filter(|(a, b)| a.starts_with("#[cfg(") && b.starts_with("#[cfg(")).count();
    assert_eq!(stacked, 0, "never two cfg lines in a row");
}

#[test]
fn scratch_copy_excludes_target_git_and_passenger() {
    let copy = fresh_copy("gate");
    std::fs::create_dir_all(copy.path().join("target/debug")).unwrap();
    std::fs::write(copy.path().join("target/debug/junk"), "x").unwrap();
    std::fs::create_dir_all(copy.path().join(".git")).unwrap();
    std::fs::write(copy.path().join(".git/HEAD"), "ref").unwrap();
    std::fs::create_dir_all(copy.path().join(".passenger")).unwrap();
    std::fs::write(copy.path().join(".passenger/config.toml"), "").unwrap();
    std::fs::create_dir_all(copy.path().join("app/target")).unwrap();
    std::fs::write(copy.path().join("app/target/deep"), "x").unwrap();

    let scratch = scratch_copy(copy.path()).expect("A4");
    let s = scratch.path();
    assert!(s.join(APP_LIB).exists());
    assert!(s.join("Cargo.toml").exists());
    assert!(s.join("klepto.toml").exists());
    assert!(s.join("beta/src/lib.rs").exists());
    assert!(!s.join("target").exists());
    assert!(!s.join(".git").exists());
    assert!(!s.join(".passenger").exists());
    assert!(!s.join("app/target").exists());
    assert_eq!(read(&s.join(APP_LIB)), read(&copy.path().join(APP_LIB)));
}

// ---------------------------------------------------------------- O rules (real cargo)

#[test]
fn cargo_oracle_proves_the_plan_and_leaves_the_tree_alone() {
    let root = fixture("gate");
    let before = read(&root.join(APP_LIB));
    let mut o = opts(&root);
    o.no_cargo = false;
    let out = gate_cmd::run(&root, &o).expect("G4");
    let conv = out.convergence.as_ref().expect("oracle ran");
    assert_eq!(conv.status, ConvergeStatus::Converged, "rounds: {}", conv.rounds.len());
    assert!(!conv.rounds.is_empty());
    assert!(conv.rounds.iter().all(|r| r.jobs.len() >= 2), "V1: default and no-default jobs");
    assert!(conv.human.is_empty());
    assert!(!out.written);
    assert_eq!(out.exit_code, 0);
    assert_eq!(read(&root.join(APP_LIB)), before, "O3");
    assert!(!out.edits.is_empty());
}

#[test]
fn oracle_check_runs_a_job_on_a_scratch_copy() {
    let root = fixture("gate");
    let (_k, planned) = gate_cmd::plan(&root, Some("app"), &root.join("klepto.toml")).unwrap();
    let jobs = klepto::check_matrix(&planned.desired);
    let mut oracle = CargoOracle::new(&root);
    let no_default = jobs.iter().find(|j| j.no_default).expect("a --no-default-features job");
    let ok = oracle.check(&root, no_default, &planned.plan);
    assert!(ok.ok, "O2: gated plan compiles without beta: {:?}", ok.diagnostics);
    // the manifest edits alone (beta optional, no gates): the ungated code must fail without beta
    let ungated = klepto::GatePlan { package: "app".into(), manifest: planned.plan.manifest.clone(), ..Default::default() };
    let bad = oracle.check(&root, no_default, &ungated);
    assert!(!bad.ok, "an ungated tree must fail without beta: {:?}", bad.diagnostics);
    assert!(bad.diagnostics.iter().any(|d| d.mention.as_deref() == Some("beta")), "{:?}", bad.diagnostics);
    assert!(oracle.scratch_dir().is_some_and(|d| d.join(APP_LIB).exists()), "O1: one scratch copy");
}

// ---------------------------------------------------------------- G6 / G7 / X rules

#[test]
fn check_write_and_idempotence() {
    let copy = fresh_copy("gate");
    let root = copy.path();

    let mut check = opts(root);
    check.check = true;
    let out = gate_cmd::run(root, &check).unwrap();
    assert_eq!(out.exit_code, 1, "G6: work to do");
    assert!(!out.written);

    let mut write = opts(root);
    write.write = true;
    let out = gate_cmd::run(root, &write).unwrap();
    assert!(out.written, "G5");
    assert_eq!(out.exit_code, 0);
    let lib = read(&root.join(APP_LIB));
    assert!(lib.contains("#[cfg(feature = \"b\")]\npub fn uses_beta()"));
    assert!(read(&root.join("app/Cargo.toml")).contains("optional = true"));

    let again = gate_cmd::run(root, &check).unwrap();
    assert!(again.plan.is_empty(), "G7: {:?}", again.plan);
    assert_eq!(again.exit_code, 0);
    let text = gate_cmd::render(&again);
    assert!(text.contains("nothing to do"), "{text}");
}

// ---------------------------------------------------------------- F rules

#[test]
fn feature_map_lists_packages_features_and_deps() {
    let root = fixture("gate");
    let map = features::feature_map(&root, None).expect("F1");
    let names: Vec<&str> = map.packages.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["app", "beta"]);
    let app = &map.packages[0];
    assert!(app.features.iter().any(|f| f.name == "default"));
    let beta = app.deps.iter().find(|d| d.name == "beta").expect("dep beta");
    assert!(!beta.optional);
    assert_eq!(beta.sites, 2);
    let text = features::render_map(&map);
    assert!(text.starts_with("CODE PASSENGER :: FEATURE MAP\n"), "{text}");
    assert!(text.contains("beta"));
}

#[test]
fn feature_sites_groups_by_file_with_cfg_and_scope() {
    let root = fixture("gate");
    let sites = features::feature_sites(&root, None, "beta").expect("F2");
    assert_eq!(sites.kind, "dep");
    assert_eq!(sites.deps, vec!["beta".to_string()]);
    assert_eq!(sites.total, 2);
    assert_eq!(sites.files.len(), 1);
    assert_eq!(sites.files[0].file, APP_LIB);
    let lines: Vec<Option<u32>> = sites.files[0].sites.iter().map(|s| s.line).collect();
    assert_eq!(lines, vec![Some(7), Some(11)]);
    assert_eq!(sites.files[0].sites[0].scope, "app::uses_beta");
    assert_eq!(sites.files[0].sites[0].cfg, "true");
    assert_eq!(sites.files[0].sites[1].cfg, "unix");
    let text = features::render_sites(&sites);
    assert!(text.starts_with("CODE PASSENGER :: FEATURE SITES\n"));
    assert!(text.contains("total_sites: 2"));

    // a feature name expands to the deps it enables — none before gating
    let by_feature = features::feature_sites(&root, None, "default").unwrap();
    assert_eq!(by_feature.kind, "feature");
    assert_eq!(by_feature.total, 0);
}

#[test]
fn feature_probe_reviews_against_the_spec_without_writing() {
    let root = fixture("gate");
    let spec = root.join("klepto.toml");
    let probe = features::feature_probe(&root, None, "beta", Some(&spec)).expect("F3");
    assert_eq!(probe.ungated.len(), 2, "{:?}", probe.ungated);
    assert!(probe.ungated.iter().any(|u| u.scope == "app::uses_beta" && u.verdict == "sometimes"));
    assert_eq!(probe.proposed.len(), 3, "{:?}", probe.proposed);
    assert!(probe.proposed.iter().any(|g| g.item == "app::unix_beta" && g.attribute == "#[cfg(all(unix, feature = \"b\"))]"));
    assert_eq!(probe.lost, vec!["app::calls_uses_beta".to_string(), "app::unix_beta".into(), "app::uses_beta".into()]);
    assert!(!probe.writes);
    let text = features::render_probe(&probe);
    assert!(text.starts_with("CODE PASSENGER :: FEATURE PROBE\n"));
    assert!(text.ends_with("writes: false\n"), "{text}");

    let current = features::feature_probe(&root, None, "beta", None).unwrap();
    assert!(current.ungated.is_empty(), "current model: beta is not optional");
    assert!(current.proposed.is_empty());
}

// ---------------------------------------------------------------- R rules

#[test]
fn receipt_and_kg_are_written() {
    let root = fixture("gate");
    let out = tempfile::tempdir().unwrap();
    let written = receipt::write(&root, &out.path().join("receipt.json"), Some(out.path())).expect("R1");
    assert_eq!(written.receipt, out.path().join("receipt.json"));
    let r = Receipt::from_json_str(&read(&written.receipt)).unwrap();
    assert!(r.verify_digest());
    assert!(r.items.iter().any(|i| i.id == "app::uses_beta"));
    let knowledge = read(&out.path().join("knowledge.ndjson"));
    assert!(knowledge.lines().count() > 10);
    assert!(out.path().join("challenges.ndjson").exists());
}

#[test]
fn lint_applies_a_policy_and_reports_an_exit_code() {
    let root = fixture("gate");
    let ok = lint::run(&root, None).expect("R2 default policy");
    assert_eq!(ok.exit_code, 0, "{}", ok.summary());

    let policy = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(policy.path(), "[levels]\nKLEP013 = \"deny\"\nKLEP001 = \"allow\"\n").unwrap();
    let v = lint::run(&root, Some(policy.path())).unwrap();
    assert_eq!(v.exit_code, 1);
    assert_eq!(v.counts.get("KLEP013"), Some(&1), "{}", v.summary());
    assert!(v.findings.iter().any(|f| f.extra["item"] == "app::util::unused_helper"));
}

#[test]
fn patterns_lists_candidates() {
    let root = fixture("gate");
    let report = patterns::run(&root, &klepto::MiningOptions::default()).expect("R3");
    let c = report.candidates.iter().find(|c| c.members.contains(&"app::util::twice".to_string())).expect("candidate");
    assert_eq!(c.kind, klepto::CandidateKind::ExtractFn);
    assert_eq!(c.members, vec!["app::util::double".to_string(), "app::util::twice".to_string()]);
    let text = patterns::render(&report, 5);
    assert!(text.contains("extract_fn") || text.contains("ExtractFn"), "{text}");
    assert!(text.contains("app::util::twice"));
}

// ---------------------------------------------------------------- E rules

#[test]
fn existing_scan_still_works_on_the_fixture_package() {
    let root = fixture("gate").join("app");
    let ctx = code_passenger::engine::RunContext {
        root: root.clone(),
        src_rel: "src".into(),
        manifest_path: root.join("Cargo.toml"),
        lang: "rust".into(),
    };
    let out = code_passenger::engine::run_scan(&ctx).expect("E1");
    assert_eq!(out.manifest.crate_name, "app");
    assert!(!out.reports.is_empty());
    assert!(out.reports.iter().any(|r| r.external_use_sites.iter().any(|u| u.dep == "beta")));
}

// ---------------------------------------------------------------- gate2: use gates, impl methods, multi-line attributes

#[test]
fn use_gates_impl_methods_and_multiline_attributes() {
    let root = fixture("gate2");
    let (_k, planned) = gate_cmd::plan(&root, Some("app"), &root.join("klepto.toml")).expect("plan gate2");
    assert!(planned.plan.unknown.is_empty(), "{:?}", planned.plan.unknown);
    assert!(planned.plan.gates.iter().any(|g| g.kind == GateKind::Use), "A1: the module-level `use beta::beep` is a carrier: {:?}", planned.plan.gates);
    let edits = apply(&root, &planned.plan).unwrap();
    let lib = &edits.files.iter().find(|f| f.path.ends_with("app/src/lib.rs")).unwrap().after;
    assert!(lib.contains("#[cfg(feature = \"b\")]\nuse beta::beep;"), "use gate: {lib}");
    assert!(lib.contains("#[cfg(feature = \"b\")]\npub fn via_import()"), "fn using the import: {lib}");
    assert!(lib.contains("    allow(dead_code)\n)]\n/// A struct holding"), "the multi-line attribute is kept intact: {lib}");
    assert!(lib.contains("\n#[cfg(feature = \"b\")]\npub struct Holder"), "after the attribute and the docs, right above the struct: {lib}");
    assert!(lib.contains("    #[cfg(feature = \"b\")]\n    pub fn tick(&self)"), "impl method keeps its indentation: {lib}");
    assert!(!lib.contains("#[cfg(feature = \"b\")]\n    pub fn plain"), "untouched method: {lib}");
    assert!(lib.contains("/// A struct holding"), "doc comments survive");

    let mut o = opts(&root);
    o.no_cargo = false;
    let out = gate_cmd::run(&root, &o).expect("converge gate2");
    assert_eq!(out.convergence.as_ref().unwrap().status, ConvergeStatus::Converged, "{}", gate_cmd::render(&out));
    assert_eq!(out.exit_code, 0);
}

// ---------------------------------------------------------------- O1: a member of a parent workspace

#[test]
fn oracle_copies_the_parent_workspace_for_a_member_package() {
    let root = fixture("parent_ws").join("inner/app");
    let ws = code_passenger::gate::workspace_root(&root).expect("locate-project");
    assert!(ws.ends_with("parent_ws"), "{}", ws.display());
    assert_eq!(code_passenger::gate::workspace_root(&fixture("gate")).unwrap(), fixture("gate"), "a workspace root is its own root");

    let mut o = opts(&root);
    o.no_cargo = false;
    let out = gate_cmd::run(&root, &o).expect("converge inside a parent workspace");
    let conv = out.convergence.as_ref().expect("oracle ran");
    assert_eq!(conv.status, ConvergeStatus::Converged, "{}", gate_cmd::render(&out));
    assert!(!out.written);
    assert_eq!(read(&root.join("src/lib.rs")), read(&fixture("gate").join(APP_LIB)), "O3: untouched");
}

#[test]
fn feature_probe_accepts_dashed_feature_names() {
    let root = fixture("gate");
    let probe = features::feature_probe(&root, None, "extra-io", None).expect("a dashed feature name is a feature");
    assert_eq!(probe.kind, "feature", "{probe:?}");
    assert!(probe.deps.is_empty());
}
