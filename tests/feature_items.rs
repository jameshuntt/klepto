//! A feature's items across crates. Fixture: tests/fixtures/featitems/
//! (p:f = ["dep:a", "a/x", "b?/y"], a:x = ["c/z"], a:default = ["d"],
//! p:g = ["f", "dep:b"], p:h = ["b/y"], p:i = ["e/w"] with e's implicit feature).

mod common;

use common::*;
use klepto::*;

fn ws() -> Klepto {
    Klepto::new("p")
        .scan_workspace_root(fixture("featitems"))
        .expect("cargo metadata on fixture featitems")
        .parse()
        .expect("parse fixture featitems")
}

fn strs(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

/// (node, items, edge that brought it) per entry, in closure order.
fn rows(m: &FeatureItemMap) -> Vec<(String, Vec<String>, String)> {
    m.entries
        .iter()
        .map(|e| (e.key.to_string(), e.items.clone(), e.enabled_by.as_ref().map(|x| x.to_string()).unwrap_or_default()))
        .collect()
}

fn row(key: &str, items: &[&str], by: &str) -> (String, Vec<String>, String) {
    (key.to_string(), strs(items), by.to_string())
}

/// affected_by_feature stays one crate's answer: p:f's own gated items,
/// nothing of a or c (the premise, and its value is unchanged).
#[test]
fn affected_by_feature_stays_in_its_crate() {
    let k = ws();
    assert_eq!(
        k.affected_by_feature("p", "f"),
        Affected { gated: strs(&["p::p_f", "p::p_g"]), uses_lost_dep: vec![], transitive: vec![] }
    );
}

/// Failed before feature_items: a::x_fn (a:x, through p:f → a/x) and
/// c::z_fn (c:z, through a:x → c/z) come in with p:f.
#[test]
fn dependency_items_come_in_with_the_feature() {
    let k = ws();
    let m = k.feature_items("p", "f");
    assert_eq!(
        rows(&m),
        vec![
            row("p:f", &["p::p_f", "p::p_g"], ""),
            row("a", &["a", "a::a_always", "a::a_without_x"], "p:f → dep:a"),
            row("a:default", &[], "p:f → dep:a (default features)"),
            row("a:x", &["a::x_fn", "a::x_mod", "a::x_mod::XThing"], "p:f → a/x"),
            row("a:d", &["a::d_fn"], "a:default → d"),
            row("c:z", &["c::z_fn"], "a:x → c/z"),
        ]
    );
    assert!(m.items_of("a").contains("a::x_fn") && m.items_of("c").contains("c::z_fn"));
    assert!(m.entries.iter().all(|e| e.scanned));
    // b is optional and nothing in p:f turns it on: b?/y waits
    assert_eq!(m.weak_pending.iter().map(|e| e.to_string()).collect::<Vec<_>>(), strs(&["p:f → b?/y"]));
    assert!(m.weak_pending[0].weak);
    assert!(m.get("b", Some("y")).is_none());
    assert_eq!(m.by_crate().keys().copied().collect::<Vec<_>>(), vec!["a", "c", "p"]);
}

/// p:g turns b on, so f's b?/y brings b:y; p:h's strong b/y turns b on itself.
#[test]
fn weak_entry_brings_its_feature_once_the_dependency_is_on() {
    let k = ws();
    let g = k.feature_items("p", "g");
    let y = g.get("b", Some("y")).expect("b:y with p:g");
    assert_eq!(y.items, strs(&["b::y_fn"]));
    let edge = y.enabled_by.as_ref().expect("an edge");
    assert_eq!(edge.to_string(), "p:f → b?/y");
    assert!(edge.weak);
    assert_eq!(edge.entry, FeatureEnable::DepFeature { dep: "b".into(), feature: "y".into(), weak: true });
    assert_eq!(g.get("b", None).and_then(|e| e.enabled_by.as_ref()).map(|e| e.to_string()).as_deref(), Some("p:g → dep:b"));
    assert!(g.weak_pending.is_empty());
    assert!(g.items_of("c").contains("c::z_fn"), "p:g → f carries f's whole closure");

    let h = k.feature_items("p", "h");
    assert_eq!(
        rows(&h),
        vec![row("p:h", &[], ""), row("b", &["b", "b::b_always"], "p:h → b/y"), row("b:y", &["b::y_fn"], "p:h → b/y")]
    );
    assert!(!h.entries[2].enabled_by.as_ref().is_some_and(|e| e.weak));
}

/// e/w on an optional e that no entry names with dep: also turns on p's
/// implicit feature e, as active_features does.
#[test]
fn implicit_feature_of_the_dependency_comes_in() {
    let k = ws();
    assert_eq!(
        rows(&k.feature_items("p", "i")),
        vec![
            row("p:i", &[], ""),
            row("p:e", &["p::p_with_e"], "p:i → e/w"),
            row("e", &["e", "e::e_always"], "p:i → e/w"),
            row("e:w", &["e::w_fn"], "p:i → e/w"),
        ]
    );
}

#[test]
fn feature_items_of_an_unknown_package_is_empty() {
    let k = ws();
    let m = k.feature_items("nope", "f");
    assert!(m.entries.is_empty() && m.weak_pending.is_empty());
}

fn chains(k: &Klepto, package: &str, item: &str) -> Vec<String> {
    k.features_bringing(package, item)
        .iter()
        .map(|c| {
            let edges: Vec<String> = c.edges.iter().map(|e| e.to_string()).collect();
            format!("{}:{} [{}] gate {}{}", c.package, c.feature, edges.join(", "), c.gate, if c.weak() { " weak" } else { "" })
        })
        .collect()
}

#[test]
fn features_bringing_an_item_of_a_dependency() {
    let k = ws();
    assert_eq!(
        chains(&k, "c", "c::z_fn"),
        strs(&[
            "a:x [a:x → c/z] gate c:z",
            "c:z [] gate c:z",
            "p:f [p:f → a/x, a:x → c/z] gate c:z",
            "p:g [p:g → f, p:f → a/x, a:x → c/z] gate c:z",
        ])
    );
    assert_eq!(
        chains(&k, "b", "b::y_fn"),
        strs(&["b:y [] gate b:y", "p:f [p:f → b?/y] gate b:y weak", "p:g [p:g → f, p:f → b?/y] gate b:y weak", "p:h [p:h → b/y] gate b:y"])
    );
    // no feature of a gates it: the crate turned on is enough
    assert_eq!(chains(&k, "a", "a::a_always"), strs(&["p:f [p:f → dep:a] gate a", "p:g [p:g → f, p:f → dep:a] gate a"]));
    assert!(k.features_bringing("a", "c::z_fn").is_empty(), "not an item of a");
    assert!(k.features_bringing("c", "c::nope").is_empty());
}

/// The two directions agree: every chain without a weak entry is a closure
/// that holds the item, for every item of the fixture.
#[test]
fn features_bringing_agrees_with_feature_items() {
    let k = ws();
    let mut checked = 0;
    for n in k.graph.nodes.values() {
        let Some(pkg) = &n.package else { continue };
        for c in k.features_bringing(pkg, &n.id).iter().filter(|c| !c.weak()) {
            let m = k.feature_items(&c.package, &c.feature);
            assert!(m.items_of(pkg).contains(n.id.as_str()), "{}:{} should bring {}", c.package, c.feature, n.id);
            checked += 1;
        }
    }
    assert!(checked >= 20, "{checked}");
}
