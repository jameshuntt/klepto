//! Phase 8 acceptance tests: receiver typing (rules T1–T6 in PHASE8_RULES.md).
//! Fixture: tests/fixtures/typing/.

mod common;

use common::*;
use klepto::*;

fn typing_ws() -> Klepto {
    Klepto::new("app")
        .scan_workspace_root(fixture("typing"))
        .expect("cargo metadata on fixture typing")
        .parse()
        .expect("parse fixture typing")
}

/// The method call `callee` inside the fn whose id ends with `in_fn`.
fn call<'a>(k: &'a Klepto, in_fn: &str, callee: &str) -> Vec<&'a CallOccurrence> {
    k.calls
        .iter()
        .filter(|c| c.is_method && c.callee == callee && c.enclosing_fn.as_deref().is_some_and(|f| f.ends_with(in_fn)))
        .collect()
}

fn local_ending(res: &Option<Resolution>, suffix: &str) -> bool {
    matches!(res, Some(Resolution::Local(id)) if id.ends_with(suffix))
}

fn assert_local(k: &Klepto, in_fn: &str, callee: &str, method: &str, receiver: &str) {
    let cs = call(k, in_fn, callee);
    assert!(!cs.is_empty(), "no `{callee}` call in {in_fn}");
    for c in cs {
        assert!(local_ending(&c.resolution, method), "{in_fn}: `{callee}` → {:?}, wanted …{method}", c.resolution);
        assert!(local_ending(&c.receiver_type, receiver), "{in_fn}: receiver of `{callee}` is {:?}, wanted …{receiver}", c.receiver_type);
    }
}

fn assert_external(k: &Klepto, in_fn: &str, callee: &str, id: &str) {
    let cs = call(k, in_fn, callee);
    assert!(!cs.is_empty(), "no `{callee}` call in {in_fn}");
    for c in cs {
        assert_eq!(c.resolution, Some(Resolution::External(id.to_string())), "{in_fn}: `{callee}`");
    }
}

#[test]
fn closure_parameters_are_items_of_the_receiver() {
    let k = typing_ws();
    assert_local(&k, "Store::heaviest", "weight", "Item::weight", "app::Item");
    assert_local(&k, "Store::labels", "label", "Item::label", "app::Item");
    assert_local(&k, "Store::first_label", "label", "Item::label", "app::Item");
}

#[test]
fn patterns_bind_parts_of_the_scrutinee() {
    let k = typing_ws();
    assert_local(&k, "Store::total", "weight", "Item::weight", "app::Item");
    assert_local(&k, "Store::named", "weight", "Item::weight", "app::Item");
    assert_external(&k, "Store::named", "len", "std::String::len");
    assert_local(&k, "Store::tuple", "weight", "Item::weight", "app::Item");
}

#[test]
fn try_index_cells_and_entries_unwrap() {
    let k = typing_ws();
    assert_local(&k, "Store::try_first", "weight", "Item::weight", "app::Item");
    assert_local(&k, "Store::nth", "label", "Item::label", "app::Item");
    assert_external(&k, "Store::cached", "len", "std::Vec::len");
    assert_local(&k, "Store::lookup_default", "weight", "Item::weight", "app::Item");
}

#[test]
fn std_shapes_flow_through_return_types_and_closure_results() {
    let k = typing_ws();
    assert_external(&k, "Store::mapped", "len", "std::String::len");
    assert_external(&k, "Store::all_labels_len", "len", "std::String::len");
    let iter_call = call(&k, "Store::labels", "map");
    assert_eq!(iter_call.len(), 1);
    assert_eq!(iter_call[0].receiver_type, Some(Resolution::External("std::Iterator".into())));
    assert_eq!(iter_call[0].resolution, Some(Resolution::External("std::Iterator::map".into())));
}

#[test]
fn trait_object_receivers_use_the_trait() {
    let k = typing_ws();
    for f in ["Store::via_dyn", "Store::via_impl"] {
        let cs = call(&k, f, "describe");
        assert_eq!(cs.len(), 1, "{f}");
        assert!(local_ending(&cs[0].resolution, "Describe::describe"), "{f}: {:?}", cs[0].resolution);
        assert!(local_ending(&cs[0].receiver_type, "app::Describe"), "{f}: {:?}", cs[0].receiver_type);
    }
}

#[test]
fn unknown_stays_unknown() {
    let k = typing_ws();
    // `let y: Item = ….into()` is typed by its annotation, not by `into`
    assert_local(&k, "Store::untyped", "weight", "Item::weight", "app::Item");
    let into = call(&k, "Store::untyped", "into");
    assert_eq!(into.len(), 1);
    assert!(into[0].receiver_type.as_ref().is_some_and(|r| matches!(r, Resolution::Local(id) if id.ends_with("app::Item"))), "{:?}", into[0].receiver_type);
    assert!(matches!(&into[0].resolution, Some(Resolution::Unresolved { reason: UnresolvedReason::MethodCall, .. })), "T6: `into` is not a method klepto knows: {:?}", into[0].resolution);
}

#[test]
fn locals_never_name_items() {
    let k = typing_ws();
    let in_shadow: Vec<&PathOccurrence> = k.paths.iter().filter(|p| p.enclosing_fn.as_deref() == Some("app::shadow") && p.path == "meta").collect();
    assert!(in_shadow.is_empty(), "the local `meta` is not a reference to `app::meta`: {in_shadow:?}");
    assert!(!k.item_graph().has_edge("app::shadow", "app::meta", EdgeKind::Reference));
    let real = k.paths.iter().find(|p| p.enclosing_fn.as_deref() == Some("app::uses_meta") && p.path == "meta::x").expect("meta::x");
    assert_eq!(real.resolution, Some(Resolution::Local("app::meta::x".into())));
    // an upper-case pattern identifier is a const: still a reference
    let lim = k.paths.iter().find(|p| p.enclosing_fn.as_deref() == Some("app::limit") && p.path == "crate::LIMIT").expect("crate::LIMIT in a pattern is a reference");
    assert_eq!(lim.resolution, Some(Resolution::Local("app::LIMIT".into())));
}

#[test]
fn namespaces_pick_the_right_binding() {
    let k = typing_ws();
    let res = |in_fn: &str, path: &str, ns: Namespace| -> Vec<Resolution> {
        k.paths.iter().filter(|p| p.enclosing_fn.as_deref() == Some(in_fn) && p.path == path && p.ns == ns).filter_map(|p| p.resolution.clone()).collect()
    };
    let ty = res("app::take", "Token", Namespace::Type);
    assert!(!ty.is_empty() && ty.iter().all(|r| *r == Resolution::Local("app::types::Token".into())), "type position → the struct: {ty:?}");
    let call = res("app::make", "Token", Namespace::Value);
    assert!(!call.is_empty() && call.iter().all(|r| *r == Resolution::Local("app::Token".into())), "call → the fn: {call:?}");
    let lit = res("app::Token", "Token", Namespace::Type);
    assert!(lit.iter().all(|r| *r == Resolution::Local("app::types::Token".into())), "struct literal and return type → the struct: {lit:?}");
    // R16: through a module's re-export
    let via = res("app::by_path", "crate::ident::Marker", Namespace::Type);
    assert_eq!(via, vec![Resolution::Local("app::types::Token".into())], "type position follows the re-export");
    let call = res("app::by_path", "ident::Marker", Namespace::Value);
    assert_eq!(call, vec![Resolution::Local("app::ident::Marker".into())], "value position takes the fn");
}

// ---------------------------------------------------------------- R14: cfg-twin bindings

#[test]
fn cfg_twin_bindings_resolve_to_the_build_at_hand() {
    let k = Klepto::new("app")
        .scan_workspace_root(fixture("usetwins"))
        .expect("cargo metadata on fixture usetwins")
        .parse()
        .expect("parse fixture usetwins");
    let path = |in_fn: &str, p: &str| -> &PathOccurrence {
        k.paths.iter().find(|x| x.enclosing_fn.as_deref() == Some(in_fn) && x.path == p).unwrap_or_else(|| panic!("no `{p}` in {in_fn}: {:?}", k.paths.iter().map(|x| (&x.path, &x.enclosing_fn)).collect::<Vec<_>>()))
    };
    let go = path("app::go", "handle");
    assert_eq!(go.resolution, Some(Resolution::Local("app::real::handle".into())), "the default build's binding");
    assert!(go.covered, "the twins cover every build: exempt from cfg checking");
    let t = path("app::tests::t", "super::handle");
    assert_eq!(t.resolution, Some(Resolution::Local("app::mocks::handle".into())), "under `test` the mock is the one the reference's cfg implies");
    let run = path("app::run", "log::trace");
    assert_eq!(run.resolution, Some(Resolution::Local("app::log::trace".into())), "`logging` is not a default feature: the module");
    assert!(run.covered);
    let v = k.cfg_edge_violations();
    assert!(v.iter().all(|x| x.from != "app::go" && x.from != "app::run"), "no violation through covered twins: {v:?}");
    assert!(!k.item_graph().has_edge("app::go", "app::mocks::handle", EdgeKind::Reference));
}

#[test]
fn feature_implications_survive_wide_feature_lists() {
    let k = Klepto::new("app")
        .scan_workspace_root(fixture("usetwins"))
        .expect("cargo metadata on fixture usetwins")
        .parse()
        .expect("parse fixture usetwins");
    let model = k.features.get("app").expect("feature model");
    assert!(model.features.len() > 16, "the fixture declares more features than the closed enumeration cap: {}", model.features.len());
    let v = k.cfg_edge_violations();
    assert!(v.iter().all(|x| x.from != "app::wrap"), "wrap_help implies help: {v:?}");
    let a = CfgPredicate::parse_str("feature = \"wrap_help\"");
    let b = CfgPredicate::parse_str("all(feature = \"help\", feature = \"wrap_help\")");
    assert!(matches!(model.closed_implies(&a, &b), Implication::Always));
}

#[test]
fn module_twins_cover_their_descendants() {
    let k = Klepto::new("app")
        .scan_workspace_root(fixture("usetwins"))
        .expect("cargo metadata on fixture usetwins")
        .parse()
        .expect("parse fixture usetwins");
    assert_eq!(k.modules.get("app::rt").expect("rt").cfg, CfgPredicate::True, "M6: the twins cover every build");
    assert_eq!(k.modules.get("app::rt::park").expect("rt::park").cfg, CfgPredicate::True, "M6 amendment: declared under the first twin, present under both");
    let new = k.items.iter().find(|i| i.id == "app::rt::park::new").expect("park::new");
    assert_eq!(new.cfg, CfgPredicate::True);
    let v = k.cfg_edge_violations();
    assert!(v.iter().all(|x| x.from != "app::use_park"), "{v:?}");
}
