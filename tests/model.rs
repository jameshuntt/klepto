//! The parsed model's serde form: a workspace parsed once, written and read back, answers as the parse did, every
//! record the same; the syntax trees are not in it (syn's trees have no serde form, and klepto's are the expanded
//! ones). Fixtures: tests/fixtures/expand (macros expanded, a registry source harvested) and tests/fixtures/bins (a
//! binary named after its target, a crate root of its own).

mod common;

use common::*;
use klepto::*;

fn parsed(name: &str) -> Klepto {
    Klepto::new("app").scan_workspace_root(fixture(name)).expect("cargo metadata").parse().expect("parse")
}

#[test]
fn a_model_read_back_is_the_model_that_was_written() {
    for name in ["expand", "bins"] {
        let k = parsed(name);
        let text = serde_json::to_string(&k).expect("a model has a serde form");
        let back: Klepto = serde_json::from_str(&text).expect("and reads back");
        // every record, the module tree, the graph, the feature models, the packages, the sources: the same JSON
        assert_eq!(serde_json::to_value(&back).unwrap(), serde_json::to_value(&k).unwrap(), "{name}");
        assert!(back.files.iter().all(|f| f.ast.items.is_empty() && f.ast.attrs.is_empty()), "read back, no file holds a tree");
        assert!(k.files.iter().any(|f| !f.ast.items.is_empty()));
        // and it answers the same
        assert_eq!(back.crate_roots, k.crate_roots);
        assert_eq!(back.items.len(), k.items.len());
        for f in &k.functions {
            let loc = f.location.clone();
            assert_eq!(back.index.enclosing(&loc).map(|x| x.fq_name.clone()), k.index.enclosing(&loc).map(|x| x.fq_name.clone()));
        }
        for i in &k.items {
            assert_eq!(back.graph.resolve_id(&i.id).is_some(), k.graph.resolve_id(&i.id).is_some(), "{}", i.id);
        }
    }
}

#[test]
fn the_model_version_names_the_klepto_that_wrote_it() {
    assert_eq!(Klepto::MODEL_VERSION, concat!("klepto-model-", env!("CARGO_PKG_VERSION")));
}
