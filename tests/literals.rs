//! String literals with the call they are an argument of, paths named in
//! macro tokens, and glob cycles that stay unresolved instead of exploding.
//! Fixtures: tests/fixtures/literals/, tests/fixtures/globcycle/.

mod common;

use common::*;
use klepto::*;

fn literals() -> Klepto {
    Klepto::new("app").scan_in_folder(fixture("literals").join("src")).parse().expect("parse fixture literals")
}

fn lit<'a>(k: &'a Klepto, value: &str) -> &'a LiteralOccurrence {
    k.literals.iter().find(|l| l.value == value).unwrap_or_else(|| panic!("no literal {value:?}; got {:?}", k.literals.iter().map(|l| &l.value).collect::<Vec<_>>()))
}

#[test]
fn a_literal_knows_its_call_and_the_calls_resolution() {
    let k = literals();
    let key = lit(&k, "APP_KEY");
    let arg = key.arg_of.as_ref().expect("APP_KEY is an argument");
    assert_eq!(arg.callee, "std::env::var");
    assert!(!arg.is_method);
    assert_eq!(arg.resolution, Some(Resolution::External("std::env::var".into())));
    assert_eq!(key.enclosing_fn.as_deref(), Some("app::read"));
    assert_eq!(key.location.line, Some(8));

    let default = lit(&k, "/run/app.sock");
    let arg = default.arg_of.as_ref().expect("the default is an argument");
    assert_eq!(arg.callee, "env_or");
    assert_eq!(arg.resolution, Some(Resolution::Local("app::env_or".into())));
}

#[test]
fn a_const_initialiser_is_its_enclosing_item() {
    let k = literals();
    let c = lit(&k, "APP_SOCKET");
    assert_eq!(c.enclosing_item.as_deref(), Some("app::ENV_SOCKET"));
    assert!(c.arg_of.is_none());
    assert!(c.enclosing_fn.is_none());
}

#[test]
fn a_path_argument_knows_its_call() {
    let k = literals();
    let p = k.paths.iter().find(|p| p.path == "ENV_SOCKET").expect("ENV_SOCKET as a path");
    assert_eq!(p.resolution, Some(Resolution::Local("app::ENV_SOCKET".into())));
    let arg = p.arg_of.as_ref().expect("passed to env_or");
    assert_eq!(arg.resolution, Some(Resolution::Local("app::env_or".into())));
}

#[test]
fn macro_tokens_give_literals_and_named_paths() {
    let k = literals();
    let printed = lit(&k, "APP_PRINTED");
    assert_eq!(printed.in_macro.as_deref(), Some("println"));
    let m = k.macros_inv.iter().find(|m| m.name == "println").expect("println!");
    assert!(m.token_paths.contains(&"a".to_string()), "{:?}", m.token_paths);
    // an array element is in no call
    assert!(lit(&k, "APP_NAMED").arg_of.is_none());
}

#[test]
fn doc_comments_are_not_literals() {
    let k = literals();
    assert!(k.literals.iter().all(|l| !l.value.contains("APP_IN_DOC")));
}

#[test]
fn glob_cycles_stay_unresolved_and_finish() {
    let t = std::time::Instant::now();
    let k = Klepto::new("app").scan_in_folder(fixture("globcycle").join("src")).parse().expect("parse fixture globcycle");
    assert!(t.elapsed().as_secs() < 30, "glob resolution took {:?}", t.elapsed());
    assert!(k.paths.iter().all(|p| p.path != "Zed" || !matches!(p.resolution, Some(Resolution::Local(_)))));
}

#[test]
fn a_local_bound_from_locals_stays_small() {
    let t = std::time::Instant::now();
    let k = Klepto::new("app").scan_in_folder(fixture("doubling").join("src")).parse().expect("parse fixture doubling");
    assert!(t.elapsed().as_secs() < 30, "method typing took {:?}", t.elapsed());
    // the first calls are typed; past the cap the receiver is unknown, never guessed
    let f_calls: Vec<_> = k.calls.iter().filter(|c| c.is_method && c.callee == "f").collect();
    assert_eq!(f_calls.len(), 40);
    assert!(f_calls.iter().any(|c| matches!(&c.resolution, Some(Resolution::Local(m)) if m == "app::S::f")));
    assert!(f_calls.iter().all(|c| matches!(&c.resolution, Some(Resolution::Local(_)) | Some(Resolution::Unresolved { .. }))));
}
