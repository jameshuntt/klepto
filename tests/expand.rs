//! Macro expansion. Fixture: tests/fixtures/expand/ (it has a Cargo.lock so the
//! `lazy_static` registry sources can be harvested offline).

mod common;

use common::*;
use klepto::*;
use std::collections::BTreeSet;

fn expand_ws() -> Klepto {
    Klepto::new("app")
        .scan_workspace_root(fixture("expand"))
        .expect("cargo metadata on fixture expand")
        .parse()
        .expect("parse fixture expand")
}

fn item<'a>(k: &'a Klepto, suffix: &str) -> &'a ItemDecl {
    k.items
        .iter()
        .find(|i| i.id == suffix || i.id.ends_with(&format!("::{suffix}")))
        .unwrap_or_else(|| panic!("item {suffix} missing; have {:?}", k.items.iter().map(|i| &i.id).collect::<Vec<_>>()))
}

// ---------------------------------------------------------------- Items from local macros

#[test]
fn items_from_local_macros_exist_and_are_marked() {
    let k = expand_ws();
    for m in ["Config::host", "Config::port"] {
        let i = item(&k, m);
        assert!(matches!(i.kind, NodeKind::Method), "{m}: {:?}", i.kind);
        assert_eq!(i.generated.as_deref(), Some("make_getters!"), "X6");
    }
    assert_eq!(item(&k, "app::Id").generated.as_deref(), Some("newtype!"));
    assert_eq!(item(&k, "Id::get").generated.as_deref(), Some("newtype!"), "X6: nested items inherit the generator");
    assert!(item(&k, "app::Config").generated.is_none(), "written items are not generated");

    let host = k.functions.iter().find(|f| f.fq_name.ends_with("Config::host")).expect("captured fn from the expansion");
    assert!(!host.has_docs, "the marker is not documentation");
    assert_eq!(host.generated.as_deref(), Some("make_getters!"));
    assert_eq!(host.return_ty.as_deref(), Some("String"), "the bound `$ty` fragment");
    let inv = k.macros_inv.iter().find(|m| m.name == "make_getters").expect("X8: the invocation is still an occurrence");
    assert_eq!(host.location.path, inv.location.path);
    assert_eq!(host.location.line, inv.location.line, "X7: generated items are located at the invocation");

    // `$crate` → `crate` in the defining package
    let config = k.functions.iter().find(|f| f.fq_name.ends_with("Id::config")).expect("Id::config");
    assert!(config.return_ty.as_deref().is_some_and(|t| t.contains("crate") && t.contains("Config")), "{:?}", config.return_ty);
}

// ---------------------------------------------------------------- Cfg_if

#[test]
fn cfg_if_branches_get_exact_predicates() {
    let k = expand_ws();
    let fast = k.modules.get("app::fast").expect("module declared inside cfg_if!");
    assert!(!fast.orphan);
    assert_eq!(fast.cfg, CfgPredicate::parse_str("feature = \"fast\""));

    let picks: Vec<&ItemDecl> = k.items.iter().filter(|i| i.name == "pick" && matches!(i.kind, NodeKind::Fn)).collect();
    assert_eq!(picks.len(), 2, "one `pick` per branch: {picks:?}");
    let have: BTreeSet<String> = picks.iter().map(|i| format!("{:?}", i.cfg)).collect();
    let want: BTreeSet<String> = ["feature = \"fast\"", "not(feature = \"fast\")"].iter().map(|s| format!("{:?}", CfgPredicate::parse_str(s))).collect();
    assert_eq!(have, want, "exact branch predicates");
    assert!(picks.iter().all(|i| i.generated.as_deref() == Some("cfg_if::cfg_if!")), "the generator is the invocation path as written: {:?}", picks.iter().map(|i| &i.generated).collect::<Vec<_>>());

    // `fast::go` is used only from the `fast`-gated `pick`: never a violation
    let v = k.cfg_edge_violations();
    assert!(v.iter().all(|x| !matches!(x.verdict, Implication::Sometimes { .. } | Implication::Never)), "{v:?}");
}

// ---------------------------------------------------------------- Occurrences survive

#[test]
fn expanded_invocations_stay_occurrences_and_expansions_are_real_code() {
    let k = expand_ws();
    let names: Vec<&str> = k.macros_inv.iter().map(|m| m.name.as_str()).collect();
    for n in ["make_getters", "newtype", "cfg_if", "call_it", "lazy_static", "mystery_macro"] {
        assert!(names.contains(&n), "{n}! missing in {names:?}");
    }
    assert_eq!(names.iter().filter(|n| **n == "make_getters").count(), 1, "one occurrence per invocation: {names:?}");
    assert_eq!(names.iter().filter(|n| **n == "newtype").count(), 2, "{names:?}");
    assert_eq!(names.iter().filter(|n| **n == "cfg_if").count(), 1, "the wrapper is internal: {names:?}");

    let inv = k.macros_inv.iter().find(|m| m.name == "call_it").expect("call_it! occurrence");
    assert_eq!(inv.enclosing_fn.as_deref(), Some("app::uses"));
    assert_eq!(inv.resolution, Some(Resolution::Local("app::call_it".into())), "R12 still resolves the name");

    // the expanded `c.host()` is a real, typed method call
    let host_id = item(&k, "Config::host").id.clone();
    let call = k
        .calls
        .iter()
        .find(|c| c.is_method && c.callee == "host" && c.location.path.ends_with("lib.rs"))
        .expect("c.host() produced by call_it!");
    assert_eq!(call.resolution, Some(Resolution::Local(host_id)), "{call:?}");
    assert_eq!(call.enclosing_fn.as_deref(), Some("app::uses"));
}

// ---------------------------------------------------------------- The ledger and dependency macros

#[test]
fn every_invocation_has_an_outcome() {
    let k = expand_ws();
    let by = |name: &str| -> Vec<Outcome> { k.expansions.iter().filter(|e| e.macro_path == name || e.macro_path.ends_with(&format!("::{name}"))).map(|e| e.outcome.clone()).collect() };
    assert_eq!(by("make_getters"), vec![Outcome::Expanded { produced: 2 }]);
    assert_eq!(by("newtype"), vec![Outcome::Expanded { produced: 2 }, Outcome::Unmatched], "X3: no rule matched → left in place");
    assert_eq!(by("cfg_if"), vec![Outcome::Expanded { produced: 2 }], "two branches");
    assert_eq!(by("call_it"), vec![Outcome::Expanded { produced: 1 }]);
    assert_eq!(by("mystery_macro"), vec![Outcome::ProcMacroOrUnknown]);
    assert!(by("vec").iter().all(|o| *o == Outcome::Builtin) && !by("vec").is_empty(), "std macros are recorded, not expanded: {:?}", by("vec"));
    assert!(k.expansions.iter().all(|e| e.location.path.ends_with("lib.rs")), "{:?}", k.expansions);

    // the unmatched / unknown invocations are still there as occurrences and nothing was invented
    assert!(k.items.iter().all(|i| i.name != "never"), "mystery_macro! is opaque: {:?}", k.items.iter().map(|i| &i.name).collect::<Vec<_>>());
    assert!(k.items.iter().all(|i| i.name != "nope"));

    // `lazy_static!` came from the registry sources
    assert!(k.dep_macro_note.is_none(), "{:?}", k.dep_macro_note);
    assert!(k.macros_known.iter().any(|(n, c)| n == "lazy_static" && c == "lazy_static"), "{:?}", k.macros_known);
    assert_eq!(by("lazy_static").first(), Some(&Outcome::Expanded { produced: 1 }), "{:?}", by("lazy_static"));
    assert!(by("__lazy_static_internal").iter().any(|o| matches!(o, Outcome::Expanded { .. })), "nested expansion: {:?}", by("__lazy_static_internal"));
    let table = item(&k, "app::TABLE");
    assert!(matches!(table.kind, NodeKind::Struct), "lazy_static makes a struct: {:?}", table.kind);
    assert_eq!(table.generated.as_deref(), Some("lazy_static::lazy_static!"), "the outermost invocation names the generator");
    assert!(k.impls.iter().any(|i| i.self_ty.raw == "TABLE"), "impls of the generated struct exist: {:?}", k.impls.iter().map(|i| &i.self_ty.raw).collect::<Vec<_>>());
}

// ---------------------------------------------------------------- opt-out

#[test]
fn dependency_macros_can_be_skipped() {
    let k = Klepto::new("app")
        .scan_workspace_root(fixture("expand"))
        .expect("cargo metadata on fixture expand")
        .skip_dependency_macros(true)
        .parse()
        .expect("parse fixture expand");
    assert!(!k.macros_known.iter().any(|(_, c)| c == "lazy_static"));
    let outcomes: Vec<&Outcome> = k.expansions.iter().filter(|e| e.macro_path == "lazy_static::lazy_static").map(|e| &e.outcome).collect();
    assert_eq!(outcomes, vec![&Outcome::ProcMacroOrUnknown], "not on disk for klepto → unknown, listed");
}

// ---------------------------------------------------------------- Macro scoping

#[test]
fn macro_names_follow_macro_use_scoping() {
    let k = Klepto::new("app")
        .scan_workspace_root(fixture("macroscope"))
        .expect("cargo metadata on fixture macroscope")
        .parse()
        .expect("parse fixture macroscope");
    let inv = |name: &str, in_fn: &str| -> Option<Resolution> {
        k.macros_inv
            .iter()
            .find(|m| m.name == name && m.enclosing_fn.as_deref() == Some(in_fn))
            .unwrap_or_else(|| panic!("no {name}! in {in_fn}: {:?}", k.macros_inv.iter().map(|m| (&m.name, &m.enclosing_fn)).collect::<Vec<_>>()))
            .resolution
            .clone()
    };
    assert_eq!(inv("lifted", "app::root"), Some(Resolution::Local("app::util::lifted".into())), "#[macro_use] lifts util's macros to the root");
    assert_eq!(inv("hidden", "app::inner::a"), Some(Resolution::Local("app::inner::hidden".into())));
    assert_eq!(inv("assert_eq", "app::assert_eq_user"), Some(Resolution::External("std::assert_eq".into())), "the test module's macro is not lifted");
    assert_eq!(inv("assert_eq", "app::tests::t"), Some(Resolution::Local("app::tests::assert_eq".into())));
    // the expander agrees: std's assert_eq! outside `tests`, the local rule inside
    let by_fn = |in_fn: &str| -> Vec<Outcome> {
        let line = k.macros_inv.iter().find(|m| m.name == "assert_eq" && m.enclosing_fn.as_deref() == Some(in_fn)).map(|m| m.location.line).unwrap();
        k.expansions.iter().filter(|e| e.macro_path == "assert_eq" && e.location.line == line).map(|e| e.outcome.clone()).collect()
    };
    assert_eq!(by_fn("app::assert_eq_user"), vec![Outcome::Builtin]);
    assert_eq!(by_fn("app::tests::t"), vec![Outcome::Expanded { produced: 0 }]);
}

// ---------------------------------------------------------------- Custom cfg keys

#[test]
fn custom_cfg_keys_make_verdicts_unknown() {
    let p = |s: &str| CfgPredicate::parse_str(s);
    assert!(matches!(p("all()").implies(&p("not(no_serde_derive)"), None), Implication::Unknown), "only a build script sets `no_serde_derive`");
    assert!(matches!(p("all()").implies(&p("not(test)"), None), Implication::Sometimes { .. }), "`test` is the toolchain's");
    assert!(matches!(p("docsrs").implies(&p("feature = \"a\""), None), Implication::Unknown));
    assert!(matches!(p("feature = \"a\"").implies(&p("any(feature = \"a\", no_x)"), None), Implication::Always));
    assert!(matches!(p("all()").implies(&p("no_x"), None), Implication::Sometimes { .. }), "a plain build has the key unset: definite");
}

// ---------------------------------------------------------------- The definition's cfg

#[test]
fn expansions_carry_the_definitions_cfg() {
    let k = expand_ws();
    let go = k.paths.iter().find(|p| p.enclosing_fn.as_deref() == Some("app::gated") && p.path == "fast::go").expect("fast::go from only_fast!");
    assert_eq!(go.cfg, CfgPredicate::parse_str("feature = \"fast\""), "the `full!`-style twin's cfg wraps what it produced");
    let v = k.cfg_edge_violations();
    assert!(v.iter().all(|x| x.from != "app::gated"), "gated code referencing gated code is no violation: {v:?}");
}
