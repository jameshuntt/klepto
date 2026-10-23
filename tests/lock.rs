//! `CargoWorkspace`: one `cargo metadata` read as cargo wrote it (editions, rust-versions, requirements, features,
//! the resolve), and `FeatureModel::from_declared` giving the same model for a manifest cargo never read.

use klepto::{version_meets, CargoWorkspace, DepKind, FeatureEnable, FeatureModel};
use std::path::PathBuf;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lockshape")
}

#[test]
fn the_resolve_carries_each_packages_edition_rust_version_and_edges() {
    let ws = CargoWorkspace::read(&fixture().join("Cargo.toml"), true).unwrap();
    let names: Vec<&str> = ws.member_packages().iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["app", "tool"]);
    let dups = ws.by_name("dup");
    assert_eq!(dups.iter().map(|p| p.version.as_str()).collect::<Vec<_>>(), ["0.1.0", "0.2.0"]);
    assert_eq!(dups.iter().map(|p| p.rust_version.as_deref()).collect::<Vec<_>>(), [Some("1.80.0"), Some("1.85.0")]);
    let app = ws.by_name("app")[0];
    assert_eq!((app.edition.as_str(), app.rust_version.as_deref()), ("2021", Some("1.60.0")));
    assert_eq!(ws.by_name("tool")[0].edition, "2024");
    assert_eq!(ws.by_name("tool")[0].rust_version, None);
    // the rename is the source name; the package is cargo's
    let tool = ws.by_name("tool")[0];
    let d = &tool.dependencies[0];
    assert_eq!((d.dep.name.as_str(), d.dep.package.as_str(), d.req.as_str()), ("dupe", "dup", "*"));
    assert!(d.path.as_ref().unwrap().ends_with("dup-0.2"));
    // app's default build turns on dup's `quick` through `fast`: cargo's unified features
    let resolve = ws.resolve.as_ref().unwrap();
    assert_eq!(resolve[&dups[0].id].features, ["quick"]);
    // the closure and the reverse dependents, read off the resolve
    let closure = ws.closure(&app.id, &[DepKind::Normal, DepKind::Build, DepKind::Dev]);
    assert!(closure.contains(&dups[0].id) && !closure.contains(&dups[1].id));
    let (direct, all) = ws.dependents(&dups[1].id);
    assert_eq!(direct.iter().map(|i| ws.packages[i].name.as_str()).collect::<Vec<_>>(), ["tool"]);
    assert_eq!(all.len(), 1, "app turns tool on only through `extra`, which the default resolve leaves off");
}

#[test]
fn no_deps_reads_the_members_without_a_resolve() {
    let ws = CargoWorkspace::read(&fixture().join("Cargo.toml"), false).unwrap();
    assert!(ws.resolve.is_none());
    assert_eq!(ws.member_packages().len(), 2);
}

#[test]
fn a_model_from_the_manifest_is_the_model_cargo_metadata_gives() {
    let ws = CargoWorkspace::read(&fixture().join("Cargo.toml"), true).unwrap();
    let app = ws.by_name("app")[0];
    let declared = FeatureModel::from_declared(
        "app",
        [("default".to_string(), vec!["fast".to_string()]), ("fast".to_string(), vec!["dup/quick".to_string()]), ("extra".to_string(), vec!["dep:tool".to_string()])],
        app.features.dependencies.clone(),
    );
    assert_eq!(declared.features, app.features.features);
    assert_eq!(declared.active_features(&[], true).into_iter().collect::<Vec<_>>(), ["default", "fast"]);
    assert_eq!(declared.deps_enabled_by("extra").into_iter().collect::<Vec<_>>(), ["tool"]);
    assert_eq!(FeatureEnable::parse("a-b?/x"), FeatureEnable::DepFeature { dep: "a_b".into(), feature: "x".into(), weak: true });
    assert_eq!(FeatureEnable::parse("dep:z"), FeatureEnable::Dep("z".into()));
}

#[test]
fn a_requirement_is_met_or_not_by_semvers_answer() {
    assert_eq!(version_meets("^0.4", "0.4.3"), Some(true));
    assert_eq!(version_meets("^0.3", "0.4.3"), Some(false));
    assert_eq!(version_meets("*", "9.0.0"), Some(true));
    assert_eq!(version_meets("nope", "1.0.0"), None);
}
