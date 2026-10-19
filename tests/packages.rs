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
