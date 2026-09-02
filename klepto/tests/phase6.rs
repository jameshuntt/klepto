//! Phase 6 acceptance tests: receipt, KG export, analysis frame.
//! Rules: PHASE6_RULES.md. Fixture: tests/fixtures/receipt/.
//! Run with `cargo test --test phase6`.

mod common;

use common::*;
use klepto::*;
use serde_json::Value;
use std::collections::BTreeSet;

fn receipt_ws() -> Klepto {
    Klepto::new("app")
        .scan_workspace_root(fixture("receipt"))
        .expect("cargo metadata on fixture receipt")
        .ignore_parse_errors(true)
        .parse()
        .expect("parse fixture receipt")
}

fn local(s: &str) -> Resolution { Resolution::Local(s.to_string()) }
fn feat(s: &str) -> CfgPredicate { CfgPredicate::Feature(s.to_string()) }

fn s(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_else(|| panic!("no string `{key}` in {v}")).to_string()
}

fn records<'a>(b: &'a [Value], kind: &str) -> Vec<&'a Value> {
    b.iter().filter(|r| r.get("kind").and_then(Value::as_str) == Some(kind)).collect()
}

fn edge<'a>(b: &'a [Value], kind: &str, from: &str, to: &str) -> Option<&'a Value> {
    records(b, kind).into_iter().find(|e| s(e, "from") == from && s(e, "to") == to)
}

fn node<'a>(b: &'a [Value], id: &str) -> &'a Value {
    b.iter().find(|r| r.get("id").and_then(Value::as_str) == Some(id)).unwrap_or_else(|| panic!("no record with id {id}"))
}

fn ids(v: &Value) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(&s(v, "expected")).unwrap_or_else(|e| panic!("expected is not a JSON array: {e} in {v}"))
}

const LIB: &str = "app/src/lib.rs";

// ---------------------------------------------------------------- R rules

#[test]
fn receipt_is_versioned_and_deterministic() {
    let a = Receipt::from_klepto(&receipt_ws());
    let b = Receipt::from_klepto(&receipt_ws());
    assert_eq!(a.version, RECEIPT_VERSION, "R1");
    assert_eq!(a.version, 1);
    assert_eq!(a.tool, "klepto");
    assert_eq!(a.tool_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(a.language, "rust");
    assert_eq!(a.crate_name, "app");
    assert_eq!(a.digest.len(), 64, "R2: blake3 hex");
    assert_eq!(a.to_json_compact(), b.to_json_compact(), "R2/R3: byte-identical across runs");
    assert_eq!(a.digest, b.digest);
    assert!(a.verify_digest());
}

#[test]
fn receipt_paths_are_relative_and_files_hashed() {
    let k = receipt_ws();
    let r = Receipt::from_klepto(&k);

    let lib = r.files.iter().find(|f| f.file == LIB).unwrap_or_else(|| panic!("R4: no {LIB} in {:?}", r.files));
    assert_eq!(lib.package, "app");
    assert_eq!(lib.module.as_deref(), Some("app"));
    let bytes = std::fs::read(fixture("receipt").join(LIB)).unwrap();
    assert_eq!(lib.blake3, blake3::hash(&bytes).to_hex().to_string());
    assert_eq!(lib.bytes, bytes.len() as u64);
    let files: Vec<&str> = r.files.iter().map(|f| f.file.as_str()).collect();
    assert!(files.contains(&"app/src/store.rs"));
    assert!(files.contains(&"app/src/orphan.rs"));
    assert!(files.contains(&"optx/src/lib.rs"));
    assert!(!files.iter().any(|f| f.contains("outside/")), "non-members are not scanned");
    let mut sorted = files.clone();
    sorted.sort();
    assert_eq!(files, sorted, "R3: sorted");

    // R3: every location relative
    for i in &r.items {
        assert!(!i.location.path.is_absolute(), "{:?}", i.location.path);
        let p = i.location.path.to_string_lossy();
        assert!(p.starts_with("app/") || p.starts_with("optx/"), "{p}");
    }
    for e in &r.edges {
        assert!(!e.location.path.is_absolute());
    }
    for f in &r.functions {
        assert!(!f.location.path.is_absolute());
    }
    for m in &r.modules {
        assert!(!m.file.is_absolute(), "{:?}", m.file);
    }
    let orphan = r.modules.iter().find(|m| m.file.to_string_lossy() == "app/src/orphan.rs").expect("orphan module");
    assert!(orphan.orphan);

    assert_eq!(r.skipped_files.len(), 1, "R4: {:?}", r.skipped_files);
    assert_eq!(r.skipped_files[0].file, "app/src/bad.rs");
    assert!(!r.skipped_files[0].error.is_empty());
}

#[test]
fn receipt_roundtrips_and_verifies_digest() {
    let r = Receipt::from_klepto(&receipt_ws());
    let back = Receipt::from_json_str(&r.to_json_string()).expect("R1: parse back");
    assert_eq!(back.digest, r.digest);
    assert!(back.verify_digest());
    assert_eq!(back.items.len(), r.items.len());
    let mut tampered = back.clone();
    tampered.items.pop();
    assert!(!tampered.verify_digest(), "R2: digest covers the body");
}

#[test]
fn receipt_carries_every_layer() {
    let k = receipt_ws();
    let r = Receipt::from_klepto(&k);

    assert_eq!(r.items.len(), k.items.len(), "R5 items");
    assert_eq!(r.edges.len(), k.item_graph().edges.len(), "R5 edges");
    assert_eq!(r.unresolved.len(), k.item_graph().unresolved.len());
    assert_eq!(r.functions.len(), k.functions.len());
    assert_eq!(r.types.len(), k.types.len());
    assert_eq!(r.impls.len(), k.impls.len());
    assert_eq!(r.imports.len(), k.imports.len());
    assert_eq!(r.exports.len(), k.exports.len());
    assert_eq!(r.calls.len(), k.calls.len());
    assert_eq!(r.attrs.len(), k.attrs.len());
    assert_eq!(r.modules.len(), k.modules.modules.len());

    let names: Vec<&str> = r.packages.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["app", "optx"]);
    assert_eq!(r.packages[0].deps, vec!["optx".to_string(), "outside".to_string()]);
    assert!(r.features["app"].features.contains_key("x"));
    assert!(r.modules.iter().any(|m| m.path == "app::store"));
    assert!(r.items.iter().any(|i| i.id == "app::store::Engine"));
    assert!(r.unresolved.iter().any(|u| u.from == "app::unknown" && u.raw == "frob"));
    assert!(r.calls.iter().any(|c| c.receiver_type == Some(local("app::store::Engine"))), "Phase 5 facts ride along");
    assert!(r.imports.iter().any(|i| i.full_path == "std::collections::HashMap"));
    assert!(r.exports.iter().any(|e| e.exported_as == "Motor"));
    assert!(r.attrs.iter().any(|a| a.path == "cfg"));

    // R5 dep uses, tagged and with cfg
    let optx: Vec<&DepUseRecord> = r.dep_uses.iter().filter(|d| d.site.dep == "optx").collect();
    assert_eq!(optx.len(), 2, "{:?}", r.dep_uses);
    assert!(optx.iter().all(|d| d.package == "app"));
    assert!(optx.iter().any(|d| d.cfg == feat("x") && d.site.scope == "app::only_x"));
    assert!(optx.iter().any(|d| d.cfg == CfgPredicate::True && d.site.scope == "app::oops"));
    assert!(r.dep_uses.iter().any(|d| d.site.dep == "outside"));
    assert!(r.dep_uses.iter().all(|d| !d.site.location.path.is_absolute()));

    // R3: canonical order
    let ids: Vec<&str> = r.items.iter().map(|i| i.id.as_str()).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted);
}

#[test]
fn stale_files_and_diff_against_an_older_receipt() {
    let new = Receipt::from_klepto(&receipt_ws());
    let mut old = new.clone();
    old.items.retain(|i| i.id != "app::never_called");
    old.edges.retain(|e| !(e.from == "app::run" && e.to == "app::store::Engine::tick"));
    for f in old.files.iter_mut().filter(|f| f.file == LIB) {
        f.blake3 = "0".repeat(64);
    }
    old.files.push(FileRecord { file: "app/src/gone.rs".into(), package: "app".into(), module: None, blake3: "1".repeat(64), bytes: 1 });
    old.files.sort_by(|a, b| a.file.cmp(&b.file));
    for f in old.functions.iter_mut().filter(|f| f.fq_name == "app::run") {
        f.signature = "fn run () -> u8".into();
    }
    let mut changed = old.items.iter().find(|i| i.id == "app::oops").unwrap().clone();
    changed.is_public = false;
    old.items.retain(|i| i.id != "app::oops");
    old.items.push(changed);

    assert_eq!(new.stale_files(&old), vec!["app/src/gone.rs".to_string(), LIB.to_string()], "R7");

    let d = new.diff(&old);
    assert_eq!(d.added_items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), vec!["app::never_called"], "R8");
    assert!(d.removed_items.is_empty());
    assert_eq!(d.changed_items.len(), 1);
    assert_eq!(d.changed_items[0].1.id, "app::oops");
    assert!(d.added_edges.iter().any(|e| e.from == "app::run" && e.to == "app::store::Engine::tick"));
    assert!(d.removed_edges.is_empty());
    assert_eq!(d.changed_signatures.len(), 1);
    assert_eq!(d.changed_signatures[0].0, "app::run");
    assert_eq!(d.changed_files, vec![LIB.to_string()]);
    assert_eq!(d.removed_files, vec!["app/src/gone.rs".to_string()]);
    assert!(d.added_files.is_empty());
}

#[test]
fn crate_root_extern_crate_alias_is_visible_in_every_module() {
    let k = receipt_ws();
    // `extern crate outside as far;` sits in app's root; store.rs uses `far::far()`
    let p = k
        .paths
        .iter()
        .find(|p| p.path == "far::far" && p.location.path.ends_with("store.rs"))
        .expect("R9 amendment: the aliased path occurrence in store.rs");
    assert_eq!(p.resolution, Some(Resolution::External("outside::far".into())), "{:?}", p.resolution);
    let r = Receipt::from_klepto(&k);
    assert!(
        r.dep_uses.iter().any(|d| d.site.dep == "outside" && d.site.scope == "app::store::from_store"),
        "the alias use is a use site of `outside`: {:?}",
        r.dep_uses.iter().map(|d| (&d.site.dep, &d.site.scope)).collect::<Vec<_>>()
    );
    assert!(!k.ungated_dep_uses().iter().any(|u| u.site.scope == "app::store::from_store"), "outside is not optional: nothing ungated");
}

// ---------------------------------------------------------------- K rules

#[test]
fn kg_records_follow_the_vocabulary_and_close() {
    let k = receipt_ws();
    let b = kg::export(&k);
    assert!(!b.knowledge.is_empty());
    let node_kinds: BTreeSet<&str> = kg::NODE_KINDS.iter().copied().collect();
    let edge_kinds: BTreeSet<&str> = kg::EDGE_KINDS.iter().copied().collect();

    let mut seen_ids = BTreeSet::new();
    let mut seen_kinds = BTreeSet::new();
    let mut node_ids = BTreeSet::new();
    for r in &b.knowledge {
        let kind = s(r, "kind");
        let id = s(r, "id");
        assert!(!s(r, "description").is_empty(), "K1: {id}");
        assert!(seen_ids.insert(id.clone()), "K1: duplicate id {id}");
        seen_kinds.insert(kind.clone());
        if node_kinds.contains(kind.as_str()) {
            node_ids.insert(id);
        } else {
            assert!(edge_kinds.contains(kind.as_str()), "K4: unknown kind {kind}");
        }
    }
    for r in &b.knowledge {
        let kind = s(r, "kind");
        if edge_kinds.contains(kind.as_str()) {
            assert!(node_ids.contains(&s(r, "from")), "K5: dangling from in {r}");
            assert!(node_ids.contains(&s(r, "to")), "K5: dangling to in {r}");
            assert!(r.get("occurrences").and_then(Value::as_u64).is_some_and(|n| n >= 1));
        }
    }
    for required in ["package", "module", "item", "feature", "crate", "constant", "contains", "references", "imports", "reexports", "implements", "impl_of", "depends_on", "enables", "gated_by"] {
        assert!(seen_kinds.contains(required), "K2/K4: no `{required}` record; have {seen_kinds:?}");
    }
    // K1: sorted by id, deterministic
    let ids: Vec<String> = b.knowledge.iter().map(|r| s(r, "id")).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted);
    let again = kg::export(&k);
    assert_eq!(b.knowledge, again.knowledge);
    assert_eq!(b.challenges, again.challenges);
}

#[test]
fn kg_node_ids_are_stable() {
    let b = kg::export(&receipt_ws()).knowledge;
    assert_eq!(s(node(&b, "pkg/app"), "kind"), "package");
    assert_eq!(s(node(&b, "pkg/optx"), "kind"), "package");
    assert_eq!(s(node(&b, "mod/app::store"), "package"), "app");
    assert_eq!(s(node(&b, "mod/app::store"), "file"), "app/src/store.rs", "K3");

    let engine = node(&b, "item/app::store::Engine");
    assert_eq!(s(engine, "item_kind"), "struct");
    assert_eq!(s(engine, "module"), "app::store");
    assert_eq!(s(engine, "file"), "app/src/store.rs");
    assert_eq!(engine.get("line").and_then(Value::as_u64), Some(1));
    assert_eq!(engine.get("public").and_then(Value::as_bool), Some(true));
    assert_eq!(s(node(&b, "item/app::store::Engine::tick"), "item_kind"), "method");
    assert_eq!(s(node(&b, "item/app::run"), "item_kind"), "fn");
    assert_eq!(s(node(&b, "item/app::only_x"), "cfg"), "feature = \"x\"");

    let x = node(&b, "feat/app/x");
    assert_eq!(x.get("declared").and_then(Value::as_bool), Some(true));
    assert_eq!(x.get("default").and_then(Value::as_bool), Some(false));
    assert_eq!(s(node(&b, "crate/outside"), "kind"), "crate");
    let far = node(&b, "item/outside::far");
    assert_eq!(s(far, "item_kind"), "external", "K6");
    assert_eq!(s(far, "package"), "outside");
    assert!(edge(&b, "contains", "crate/outside", "item/outside::far").is_some(), "K6");
    assert!(b.iter().any(|r| s(r, "id") == "item/app::<impl app::Runner for Engine>"));
    let c = node(&b, "const/app/items");
    assert_eq!(s(c, "kind"), "constant");
    assert!(c.get("value").and_then(Value::as_u64).is_some_and(|n| n > 5));
}

#[test]
fn kg_edges_cover_features_deps_and_impls() {
    let b = kg::export(&receipt_ws()).knowledge;
    let gated = edge(&b, "gated_by", "item/app::only_x", "feat/app/x").expect("K4 gated_by");
    assert_eq!(s(gated, "id"), "gated_by/app::only_x/app/x");
    assert!(s(gated, "description").contains("feature = \"x\""));
    assert!(edge(&b, "enables", "feat/app/x", "pkg/optx").is_some(), "K4 enables → scanned dep");
    let dep = edge(&b, "depends_on", "pkg/app", "pkg/optx").expect("K4 depends_on optional member");
    assert_eq!(dep.get("optional").and_then(Value::as_bool), Some(true));
    assert!(edge(&b, "depends_on", "pkg/app", "crate/outside").is_some());
    assert!(edge(&b, "implements", "item/app::<impl app::Runner for Engine>", "item/app::Runner").is_some());
    assert!(edge(&b, "impl_of", "item/app::<impl app::Runner for Engine>", "item/app::store::Engine").is_some());
    assert!(edge(&b, "reexports", "mod/app", "item/app::store::Engine").is_some());
    assert!(edge(&b, "imports", "mod/app", "item/std::collections::HashMap").is_some(), "K5: external target created on demand");
    let refs = edge(&b, "references", "item/app::run", "item/app::store::Engine::tick").expect("Phase 5 edge");
    assert_eq!(s(refs, "file"), LIB);
    assert_eq!(refs.get("line").and_then(Value::as_u64), Some(18));
    assert!(edge(&b, "references", "item/app::far_away", "item/outside::far").is_some());
    assert!(edge(&b, "contains", "pkg/app", "mod/app").is_some());
    assert!(edge(&b, "contains", "mod/app", "mod/app::store").is_some());
    assert!(edge(&b, "contains", "mod/app::store", "item/app::store::Engine").is_some());
}

#[test]
fn kg_challenges_are_digested_split_and_correct() {
    let b = kg::export(&receipt_ws());
    assert!(!b.challenges.is_empty());
    for c in &b.challenges {
        assert_eq!(s(c, "kind"), "challenge");
        let class = s(c, "class");
        assert!(kg::CHALLENGE_CLASSES.contains(&class.as_str()), "K7: {class}");
        let digest = s(c, "digest");
        assert_eq!(digest.len(), 64);
        assert!(digest.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert_eq!(s(c, "split"), kg::split_for(&digest));
        assert!(s(c, "id").starts_with(&format!("chal/{class}/")));
        assert!(s(c, "id").ends_with(&digest[0..8]));
        assert_eq!(s(c, "language"), "rust");
        assert_eq!(s(c, "frontier"), "available");
        assert!(!s(c, "prompt").is_empty());
        assert!(!s(c, "expected").is_empty());
        assert!(!s(c, "description").is_empty());
        assert_eq!(digest, kg::sha256_hex(&format!("klepto|{class}|{}|{}|{}", s(c, "package"), s(c, "prompt"), s(c, "expected"))));
    }
    let deps = b.challenges.iter().find(|c| s(c, "class") == "dependents_of" && s(c, "item") == "app::store::Engine::tick").expect("dependents_of tick");
    assert_eq!(ids(deps), vec!["app::run"]);
    let path = b.challenges.iter().find(|c| s(c, "class") == "resolve_path" && s(c, "path") == "std::collections::HashMap").expect("resolve_path");
    assert_eq!(s(path, "expected"), "std::collections::HashMap");
    assert_eq!(s(path, "module"), "app");
    let lost = b.challenges.iter().find(|c| s(c, "class") == "lost_without_feature" && s(c, "feature") == "x").expect("lost_without_feature x");
    let lost_ids = ids(lost);
    for expected in ["app::only_x", "app::oops", "app::calls_only_x"] {
        assert!(lost_ids.contains(&expected.to_string()), "{lost_ids:?}");
    }
    let recv = b.challenges.iter().find(|c| s(c, "class") == "receiver_type" && s(c, "method") == "tick").expect("receiver_type");
    assert_eq!(s(recv, "expected"), "app::store::Engine");
    assert_eq!(s(recv, "function"), "app::run");
    assert_eq!(recv.get("line").and_then(Value::as_u64), Some(18));
    let ids: Vec<String> = b.challenges.iter().map(|c| s(c, "id")).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted, "K8");
}

#[test]
fn kg_ndjson_roundtrips() {
    let b = kg::export(&receipt_ws());
    let text = kg::to_ndjson(&b.knowledge);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), b.knowledge.len());
    for (line, rec) in lines.iter().zip(&b.knowledge) {
        assert_eq!(&serde_json::from_str::<Value>(line).unwrap(), rec);
    }
    assert!(text.ends_with('\n'));
}

// ---------------------------------------------------------------- A rules

#[test]
fn builtin_rules_cover_phases_2_to_5() {
    let k = receipt_ws();
    let findings = RuleRunner::new(&k).with_all_rules().run();
    let codes: Vec<&str> = rules::all_codes().iter().map(|(c, _)| *c).collect();
    for c in ["KLEP001", "KLEP010", "KLEP011", "KLEP012", "KLEP013", "KLEP014", "KLEP015"] {
        assert!(codes.contains(&c), "A1: {c} in {codes:?}");
    }
    let by = |code: &str| findings.iter().filter(|f| f.code == code).collect::<Vec<_>>();

    let ungated = by("KLEP010");
    assert_eq!(ungated.len(), 1, "A2: {ungated:?}");
    assert_eq!(ungated[0].location.line, Some(27));
    assert_eq!(ungated[0].severity, Severity::Deny);
    assert_eq!(ungated[0].extra["dep"], "optx");
    assert_eq!(ungated[0].extra["scope"], "app::oops");

    let viol = by("KLEP011");
    assert_eq!(viol.len(), 1, "{viol:?}");
    assert_eq!(viol[0].extra["from"], "app::calls_only_x");
    assert_eq!(viol[0].extra["to"], "app::only_x");
    assert_eq!(viol[0].severity, Severity::Deny);

    let unres = by("KLEP012");
    assert!(unres.iter().any(|f| f.extra["from"] == "app::unknown" && f.extra["raw"] == "frob"), "{unres:?}");
    assert!(unres.iter().all(|f| f.severity == Severity::Info));

    let dead = by("KLEP013");
    assert_eq!(dead.iter().map(|f| f.extra["item"].as_str().unwrap()).collect::<Vec<_>>(), vec!["app::never_called"], "{dead:?}");
    assert_eq!(dead[0].location.line, Some(31));

    let orphan = by("KLEP014");
    assert_eq!(orphan.len(), 1);
    assert_eq!(orphan[0].extra["file"], "app/src/orphan.rs");
    assert_eq!(orphan[0].severity, Severity::Warn);

    let skipped = by("KLEP015");
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].extra["file"], "app/src/bad.rs");

    // A3: stable order
    let keys: Vec<(String, std::path::PathBuf, Option<u32>)> = findings.iter().map(|f| (f.code.clone(), f.location.path.clone(), f.location.line)).collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
}

#[test]
fn policy_parses_and_verdict_has_exit_code() {
    let k = receipt_ws();
    let policy = Policy::parse(
        r#"
[levels]
KLEP012 = "allow"
KLEP013 = "warn"
KLEP010 = "deny"

[max]
KLEP011 = 0
KLEP013 = 0
"#,
    )
    .expect("A4");
    let v = run_pipeline(&k, &policy);
    assert_eq!(v.exit_code, 1, "A5: {:?}", v.violations);
    let violated: BTreeSet<&str> = v.violations.iter().map(|x| x.code.as_str()).collect();
    assert!(violated.contains("KLEP010"), "{violated:?}");
    assert!(violated.contains("KLEP011"));
    assert!(violated.contains("KLEP013"), "max exceeded at warn level");
    assert_eq!(v.violations.iter().find(|x| x.code == "KLEP013").unwrap().limit, Some(0));
    assert_eq!(v.counts["KLEP010"], 1);
    assert!(!v.findings.iter().any(|f| f.code == "KLEP012"), "allowed findings are dropped");
    assert!(v.findings.iter().any(|f| f.code == "KLEP013"));
    assert!(v.findings.iter().any(|f| f.code == "KLEP014"), "default level from severity");

    let ok = run_pipeline(&k, &Policy::permissive());
    assert_eq!(ok.exit_code, 0);
    assert!(ok.findings.is_empty());

    assert!(matches!(Policy::parse("[levels]\nKLEP999 = \"deny\"\n"), Err(PolicyError::UnknownCode(c)) if c == "KLEP999"));
    assert!(Policy::parse("[levels]\nKLEP010 = \"maybe\"\n").is_err());
    assert!(Policy::parse("[nope]\nx = 1\n").is_err());
}

#[test]
fn verdict_summary_is_stable_text() {
    let k = receipt_ws();
    let policy = Policy::parse("[levels]\nKLEP012 = \"allow\"\n").unwrap();
    let v = run_pipeline(&k, &policy);
    let text = v.summary();
    assert!(text.contains("KLEP010 ungated-dep-use: 1 (deny)"), "A6: {text}");
    assert!(text.contains("KLEP011 cfg-edge-violation: 1 (deny)"));
    assert!(text.contains("KLEP014 orphan-file: 1 (warn)"));
    assert!(text.ends_with("exit 1"));
    assert_eq!(text, run_pipeline(&k, &policy).summary());
}
