//! Phase 3 acceptance tests: the item graph.
//! Rules: PHASE3_RULES.md. Fixture: tests/fixtures/graph/.

mod common;

use common::*;
use klepto::*;
use std::collections::BTreeSet;

fn graph_ws() -> Klepto {
    Klepto::new("app")
        .scan_workspace_root(fixture("graph"))
        .expect("cargo metadata on fixture graph")
        .parse()
        .expect("parse fixture graph")
}

fn graph_folder() -> Klepto {
    Klepto::new("app")
        .scan_in_folder(fixture("graph").join("app").join("src"))
        .parse()
        .expect("parse fixture graph/app/src")
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

fn strs(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

fn feat(s: &str) -> CfgPredicate { CfgPredicate::Feature(s.to_string()) }

fn assert_edge(g: &ItemGraph, from: &str, to: &str, kind: EdgeKind) {
    assert!(
        g.has_edge(from, to, kind),
        "missing {kind:?} edge {from} -> {to}; edges from {from}: {:?}",
        g.edges_from(from).iter().map(|e| (&e.to, e.kind)).collect::<Vec<_>>()
    );
}

const RUN: &str = "app::<Config as app::Runner>::run";
const IMPL_RUNNER: &str = "app::<impl app::Runner for Config>";
const IMPL_CONFIG: &str = "app::<impl Config>";

// ---------------------------------------------------------------- N rules

#[test]
fn nodes_cover_every_item_kind() {
    let k = graph_ws();
    let g = k.item_graph();
    let kind = |id: &str| g.node(id).unwrap_or_else(|| panic!("no node {id}; have {:?}", g.nodes.keys().collect::<Vec<_>>())).kind;

    assert_eq!(kind("app"), NodeKind::Module);
    assert_eq!(kind("app::store"), NodeKind::Module);
    assert_eq!(kind("app::api"), NodeKind::Module);
    assert_eq!(kind("app::Config"), NodeKind::Struct);
    assert_eq!(kind("app::Runner"), NodeKind::Trait);
    assert_eq!(kind("app::Runner::run"), NodeKind::Method);
    assert_eq!(kind("app::LIMIT"), NodeKind::Const);
    assert_eq!(kind("app::NAME"), NodeKind::Static);
    assert_eq!(kind("app::Alias"), NodeKind::TypeAlias);
    assert_eq!(kind("app::shout"), NodeKind::Macro);
    assert_eq!(kind("app::top"), NodeKind::Fn);
    assert_eq!(kind("app::store::Engine"), NodeKind::Struct);
    assert_eq!(kind("app::store::Engine::new"), NodeKind::Method);
    assert_eq!(kind("app::store::MAX"), NodeKind::Const);
    assert_eq!(kind(IMPL_CONFIG), NodeKind::Impl);
    assert_eq!(kind(IMPL_RUNNER), NodeKind::Impl);
    assert_eq!(kind("app::store::<impl Engine>"), NodeKind::Impl);
    assert_eq!(kind(RUN), NodeKind::Method);
    assert_eq!(kind("app::Config::engine"), NodeKind::Method);

    // scanned workspace members are local nodes even when referenced from app (N3)
    assert_eq!(kind("dep_x::x"), NodeKind::Fn);
    assert_eq!(g.node("dep_x::x").unwrap().package.as_deref(), Some("dep_x"));
    assert_eq!(kind("dep_x::x2"), NodeKind::Fn);
    assert_eq!(kind("opt_y::y"), NodeKind::Fn);
    assert_eq!(kind("dep_x::Gx"), NodeKind::Struct);
    assert_eq!(kind("dep_x"), NodeKind::Module);

    // a crate that is not scanned is external, created on demand
    assert_eq!(kind("std::collections::HashMap"), NodeKind::External, "N3");
    assert_eq!(kind("std::collections::HashMap::new"), NodeKind::External);
}

#[test]
fn nodes_carry_package_location_cfg_and_visibility() {
    let k = graph_ws();
    let g = k.item_graph();

    let gated = g.node("app::gated_fn").unwrap();
    assert_eq!(gated.package.as_deref(), Some("app"));
    assert_eq!(gated.module, "app");
    assert_eq!(gated.name, "gated_fn");
    assert!(gated.location.as_ref().unwrap().path.ends_with("app/src/lib.rs"));
    assert_eq!(gated.cfg, feat("y"), "N2 cfg");
    assert!(gated.is_public);

    let chain_b = g.node("app::chain_b").unwrap();
    assert!(!chain_b.is_public);
    assert_eq!(chain_b.cfg, CfgPredicate::True);

    let engine_new = g.node("app::store::Engine::new").unwrap();
    assert_eq!(engine_new.module, "app::store");
    assert_eq!(engine_new.name, "new");

    let ext = g.node("std::collections::HashMap").unwrap();
    assert_eq!(ext.package, None, "N3");
    assert_eq!(ext.module, "std");
    assert_eq!(ext.name, "HashMap");
    assert!(ext.location.is_none());
    assert_eq!(ext.cfg, CfgPredicate::True);
    assert!(ext.is_public);

    let m = g.node("app::store").unwrap();
    assert!(m.is_public);
    assert_eq!(m.cfg, CfgPredicate::True);
}

#[test]
fn external_roots_group_external_nodes_by_crate() {
    let k = graph_ws();
    let roots = k.item_graph().external_roots();
    assert!(
        roots["std"].is_superset(&set(&["std::collections::HashMap", "std::collections::HashMap::new"])),
        "{roots:?}"
    );
    assert!(!roots.contains_key("dep_x"), "scanned members are local, not external: {roots:?}");
    assert!(!roots.contains_key("opt_y"));
}

// ---------------------------------------------------------------- D rules

#[test]
fn reference_edges_from_bodies_signatures_fields_and_impls() {
    let k = graph_ws();
    let g = k.item_graph();
    use EdgeKind::Reference;

    assert_edge(g, "app::top", "app::store::helper", Reference);
    assert_edge(g, "app::Config", "app::store::Engine", Reference); // field type
    assert_edge(g, "app::Config::engine", "app::store::Engine", Reference); // return type
    assert_edge(g, IMPL_RUNNER, "app::Runner", Reference); // D8
    assert_edge(g, IMPL_RUNNER, "app::Config", Reference);
    assert_edge(g, "app::store::<impl Engine>", "app::store::Engine", Reference);
    assert_edge(g, RUN, "app::store::Engine::new", Reference);
    assert_edge(g, "app::LIMIT", "app::store::MAX", Reference);
    assert_edge(g, "app::Alias", "app::store::Engine", Reference);
    assert_edge(g, "app::uses_macro", "app::shout", Reference); // macro
    assert_edge(g, "app::uses_dep_x", "dep_x::x", Reference); // external
    assert_edge(g, "app::api::serve", "app::store::Engine::new", Reference); // via alias
    assert_edge(g, "app::api::serve", "app::top", Reference); // super::
    assert_edge(g, "app::store::helper", "app::store::Engine::new", Reference);
    assert_edge(g, "app::chain_c", "dep_x::x2", Reference);
    assert_edge(g, "app::gated_fn", "opt_y::y", Reference);

    // an edge carries the referencing site's cfg
    let e = g.edges_from("app::gated_fn").into_iter().find(|e| e.to == "opt_y::y").unwrap();
    assert_eq!(e.cfg, feat("y"));
    assert!(e.location.line.is_some());
}

#[test]
fn import_reexport_and_contains_edges() {
    let k = graph_ws();
    let g = k.item_graph();

    assert_edge(g, "app", "app::api::serve", EdgeKind::ReExport); // D2
    assert_edge(g, "app::api", "app::store::Engine", EdgeKind::Import);
    assert_edge(g, "app::api", "dep_x", EdgeKind::Import); // external glob → crate root node

    assert_edge(g, "app", "app::store", EdgeKind::Contains); // D3
    assert_edge(g, "app", "app::Config", EdgeKind::Contains);
    assert_edge(g, "app", IMPL_CONFIG, EdgeKind::Contains);
    assert_edge(g, IMPL_CONFIG, "app::Config::engine", EdgeKind::Contains);
    assert_edge(g, IMPL_RUNNER, RUN, EdgeKind::Contains);
    assert_edge(g, "app::Runner", "app::Runner::run", EdgeKind::Contains);
    assert_edge(g, "app::store", "app::store::MAX", EdgeKind::Contains);

    // imports are not dependencies-of-record for dependents (Q2) but are for Q1
    assert!(g.dependencies_of("app::api").contains("app::store::Engine"));
    assert!(!g.dependents_of("app::store::Engine").contains("app::api"));
}

#[test]
fn resolve_id_accepts_one_associated_segment() {
    let k = graph_ws();
    let g = k.item_graph();
    assert_eq!(g.resolve_id("app::store::Engine::new"), Some("app::store::Engine::new"));
    assert_eq!(g.resolve_id("app::store::Engine::nonexistent"), Some("app::store::Engine"), "D4: associated item → the type");
    assert_eq!(g.resolve_id("app::store::Engine::a::b"), None);
    assert_eq!(g.resolve_id("app::nope"), None, "D4: never falls back to a module");
    assert_eq!(g.resolve_id("app::store"), Some("app::store"));
    assert_eq!(g.resolve_id("std::collections::HashMap::nested"), None, "D4: no prefix logic on externals");
    assert_eq!(g.resolve_id("dep_x::x::nested"), Some("dep_x::x"), "a scanned member's fn is a local node");
}

#[test]
fn complementary_twin_definitions_are_always_present() {
    let k = Klepto::new("app")
        .scan_workspace_root(fixture("twins"))
        .expect("cargo metadata on fixture twins")
        .parse()
        .expect("parse fixture twins");
    let g = k.item_graph();
    assert!(g.duplicates.contains(&"app::pos".to_string()), "N6: still listed as a duplicate");
    assert_eq!(g.node("app::pos").unwrap().cfg, CfgPredicate::True, "N6 amendment: the twins cover every configuration");
    assert!(g.node("app::partial").unwrap().cfg.is_unknown(), "unix/windows do not provably cover everything: undecidable, never the first twin's cfg");
    let v = k.cfg_edge_violations();
    let pairs: Vec<(&str, &str)> = v.iter().map(|x| (x.from.as_str(), x.to.as_str())).collect();
    assert!(!pairs.contains(&("app::caller", "app::pos")), "no false positive: {pairs:?}");
    assert!(pairs.contains(&("app::bad_caller", "app::only_x")), "the real one stays: {pairs:?}");
    let partial = v.iter().find(|x| x.from == "app::calls_partial" && x.to == "app::partial").expect("partial twins are listed");
    assert_eq!(partial.verdict, Implication::Unknown, "but as undecidable, not as a definite violation");

    // M6 amendment: a module declared twice under complementary cfgs
    let twin = k.modules.get("app::twin").expect("module app::twin");
    assert_eq!(twin.cfg, CfgPredicate::True, "M6 amendment: the twin module is always present: {:?}", twin.cfg);
    assert_eq!(g.node("app::twin::t").unwrap().cfg, CfgPredicate::True, "and so are its items");
    assert!(!pairs.contains(&("app::calls_twin", "app::twin::t")), "no false positive on the module twin: {pairs:?}");
}

#[test]
fn self_edges_dropped_and_unresolved_listed() {
    let k = graph_ws();
    let g = k.item_graph();

    assert!(!g.has_edge("app::recursive", "app::recursive", EdgeKind::Reference), "D6");

    // D5 as amended by Phase 5 G1: `self.engine.tick()` in run has a typed
    // receiver (self → Config → field engine → Engine), so it is an edge now
    // and no longer listed as unresolved.
    assert!(g.has_edge(RUN, "app::store::Engine::tick", EdgeKind::Reference), "{:?}", g.edges_from(RUN));
    assert!(!g.unresolved.iter().any(|u| u.from == RUN && u.raw == "tick"), "{:?}", g.unresolved);

    // D5: the glob-only name in via_glob
    let via = g.unresolved.iter().find(|u| u.from == "app::api::via_glob").expect("via_glob unresolved");
    match &via.resolution {
        Resolution::Unresolved { candidates, .. } => assert_eq!(candidates, &strs(&["dep_x::Gx::new"])),
        other => panic!("{other:?}"),
    }
    assert!(g.dependencies_of("app::api::via_glob").is_empty());
}

#[test]
fn enclosing_item_is_recorded_on_occurrences() {
    let k = graph_ws();
    let find = |raw: &str, item: &str| {
        k.paths
            .iter()
            .find(|p| p.path == raw && p.enclosing_item.as_deref() == Some(item))
            .unwrap_or_else(|| panic!("no path {raw} in {item}; have {:?}", k.paths.iter().map(|p| (&p.path, &p.enclosing_item)).collect::<Vec<_>>()))
    };
    let field = find("store::Engine", "app::Config"); // N5 struct field
    assert_eq!(field.enclosing_fn, None);
    find("store::MAX", "app::LIMIT"); // const initialiser
    find("store::Engine", "app::Alias"); // type alias
    find("Runner", IMPL_RUNNER); // impl header
    let in_fn = find("store::Engine::new", RUN);
    assert_eq!(in_fn.enclosing_fn.as_deref(), Some(RUN), "inside a fn, enclosing_item == enclosing_fn");

    // module-level: a `use` has no enclosing item
    let imp = k.imports.iter().find(|i| i.full_path == "crate::store::Engine").unwrap();
    assert_eq!(imp.enclosing_item, None);
}

// ---------------------------------------------------------------- Q rules

#[test]
fn dependents_and_dependencies() {
    let k = graph_ws();
    let g = k.item_graph();

    let deps_of_engine = g.dependents_of("app::store::Engine");
    for expected in ["app::Config", "app::Config::engine", "app::Alias", "app::store::<impl Engine>"] {
        assert!(deps_of_engine.contains(expected), "Q2: {deps_of_engine:?}");
    }
    assert_eq!(g.dependencies_of("app::api::serve"), set(&["app::store::Engine::new", "app::top"]), "Q1");
    assert!(g.dependencies_of("app::island").is_empty());
    assert!(g.dependents_of("app::island").is_empty());
}

#[test]
fn transitive_closures() {
    let k = graph_ws();
    let g = k.item_graph();
    assert_eq!(g.transitive_dependents(["dep_x::x2"]), set(&["app::chain_a", "app::chain_b", "app::chain_c"]), "Q3");
    assert_eq!(g.transitive_dependencies("app::chain_a"), set(&["app::chain_b", "app::chain_c", "dep_x::x2"]), "Q4");
    let users_of_new = g.transitive_dependents(["app::store::Engine::new"]);
    for expected in ["app::store::helper", "app::top", "app::api::serve", RUN, "app"] {
        assert!(users_of_new.contains(expected), "re-exports count: {users_of_new:?}");
    }
}

#[test]
fn affected_by_dep() {
    let k = graph_ws();
    assert_eq!(
        k.affected_by_dep("dep_x"),
        Affected { gated: vec![], uses_lost_dep: strs(&["app::chain_c", "app::uses_dep_x"]), transitive: strs(&["app::chain_a", "app::chain_b"]) },
        "Q5"
    );
    assert_eq!(
        k.affected_by_dep("opt_y"),
        Affected {
            gated: vec![],
            uses_lost_dep: strs(&["app::gated_fn", "app::sneaky", "app::store::store_gated"]),
            transitive: strs(&["app::ok_caller", "app::store::store_uses_gated", "app::uses_gated"]),
        }
    );
    assert_eq!(k.affected_by_dep("nope"), Affected::default());
}

#[test]
fn affected_by_feature() {
    let k = graph_ws();
    assert_eq!(
        k.affected_by_feature("app", "y"),
        Affected {
            gated: strs(&["app::gated_fn", "app::ok_caller", "app::store::store_gated"]),
            uses_lost_dep: strs(&["app::sneaky"]),
            transitive: strs(&["app::store::store_uses_gated", "app::uses_gated"]),
        },
        "Q6"
    );
    assert_eq!(k.affected_by_feature("app", "nope"), Affected::default(), "unknown feature: nothing is lost");
    assert_eq!(k.affected_by_feature("nope", "y"), Affected::default());
}

#[test]
fn cfg_edge_violations_find_ungated_references_to_gated_items() {
    let k = graph_ws();
    let m = k.feature_model("app").unwrap();
    let violations = k.cfg_edge_violations();
    let pairs: BTreeSet<(String, String)> = violations.iter().map(|v| (v.from.clone(), v.to.clone())).collect();
    let expected: BTreeSet<(String, String)> = [
        ("app::uses_gated", "app::gated_fn"),
        ("app::store::store_uses_gated", "app::store::store_gated"),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    assert_eq!(pairs, expected, "Q7: {violations:?}");

    let v = violations.iter().find(|v| v.from == "app::uses_gated").unwrap();
    assert_eq!(v.package, "app");
    assert_eq!(v.edge_cfg, CfgPredicate::True);
    assert_eq!(v.to_cfg, feat("y"));
    assert_eq!(v.verdict, Implication::Sometimes { counterexample: m.default_env() });
    assert!(v.location.line.is_some());
}

// ---------------------------------------------------------------- no-metadata mode

#[test]
fn folder_scan_builds_the_graph_without_a_model() {
    let k = graph_folder();
    let g = k.item_graph();
    assert!(g.node("app::store::Engine").is_some());
    assert_edge(g, "app::chain_c", "dep_x::x2", EdgeKind::Reference);
    assert_eq!(
        k.affected_by_dep("dep_x"),
        Affected { gated: vec![], uses_lost_dep: strs(&["app::chain_c", "app::uses_dep_x"]), transitive: strs(&["app::chain_a", "app::chain_b"]) }
    );
    assert_eq!(k.affected_by_feature("app", "y"), Affected::default(), "no model → nothing claimed");
    assert!(k.cfg_edge_violations().is_empty());
}
