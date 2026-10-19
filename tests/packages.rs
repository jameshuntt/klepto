//! Workspace members as cargo metadata states them. Fixture: tests/fixtures/ws/.

mod common;

use common::*;

#[test]
fn every_member_has_its_manifest_root_and_targets() {
    let k = ws();
    let app = k.packages.get("app").expect("app is a member");
    assert_eq!(app.cargo_name, "app");
    assert!(app.manifest_path.ends_with("ws/app/Cargo.toml"), "{}", app.manifest_path.display());
    assert!(app.root.ends_with("ws/app"));
    assert!(app.targets.iter().any(|t| t.kind == "lib" && t.src_path.ends_with("app/src/lib.rs")));
    assert!(k.packages.values().all(|p| p.manifest_path.parent() == Some(p.root.as_path())));
}

#[test]
fn folder_scans_have_no_packages() {
    assert!(app_folder().packages.is_empty());
}

#[test]
fn the_builder_has_the_packages_before_parsing() {
    let b = klepto::Klepto::new("app").scan_workspace_root(fixture("ws")).expect("metadata");
    assert!(b.packages().contains_key("app"));
    assert!(b.packages().contains_key("mydep"));
}

#[test]
fn a_bin_named_after_its_target_resolves_its_packages_dependencies() {
    let k = klepto::Klepto::new("bins").scan_workspace_root(fixture("bins")).expect("metadata").parse().expect("parse");
    assert_eq!(k.crate_roots.get("appd").map(String::as_str), Some("app"));
    let call = k.paths.iter().find(|p| p.path == "lib_a::hello").expect("the call in appd");
    assert_eq!(call.resolution, Some(klepto::Resolution::External("lib_a::hello".into())));
}

#[test]
fn an_integration_test_is_a_crate_root_that_uses_its_package() {
    // include_tests after the scan reads tests/ just the same
    let k = klepto::Klepto::new("bins").scan_workspace_root(fixture("bins")).expect("metadata").include_tests(true).parse().expect("parse");
    assert_eq!(k.crate_roots.get("hello").map(String::as_str), Some("lib_a"));
    assert!(k.items.iter().any(|d| d.id == "hello::hello_is_one"));
    assert!(k.graph.edges.iter().any(|e| e.from == "hello::hello_is_one" && e.to == "lib_a::hello"), "{:?}", k.graph.edges.iter().filter(|e| e.from.starts_with("hello")).collect::<Vec<_>>());
    let without = klepto::Klepto::new("bins").scan_workspace_root(fixture("bins")).expect("metadata").parse().expect("parse");
    assert!(!without.crate_roots.contains_key("hello"));
}
