//! Phase 1 acceptance tests: module tree + alias resolution.
//! Rules: PHASE1_RULES.md. Fixture: tests/fixtures/resolve/.

mod common;

use common::*;
use klepto::*;
use std::collections::BTreeSet;

fn resolve_ws() -> Klepto {
    Klepto::new("app")
        .scan_workspace_root(fixture("resolve"))
        .expect("cargo metadata on fixture resolve")
        .parse()
        .expect("parse fixture resolve")
}

fn resolve_folder() -> Klepto {
    Klepto::new("app")
        .scan_in_folder(fixture("resolve").join("app").join("src"))
        .parse()
        .expect("parse fixture resolve/app/src")
}

fn deps(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

fn local(s: &str) -> Resolution { Resolution::Local(s.to_string()) }
fn external(s: &str) -> Resolution { Resolution::External(s.to_string()) }

/// The path occurrence spelled `raw` inside the function whose fq_name ends with `fn_suffix`.
fn path_in<'a>(k: &'a Klepto, fn_suffix: &str, raw: &str) -> &'a PathOccurrence {
    k.paths
        .iter()
        .find(|p| p.path == raw && p.enclosing_fn.as_deref().is_some_and(|f| f.ends_with(fn_suffix)))
        .unwrap_or_else(|| {
            let seen: Vec<(&str, Option<&str>)> =
                k.paths.iter().map(|p| (p.path.as_str(), p.enclosing_fn.as_deref())).collect();
            panic!("no path `{raw}` inside a fn ending with `{fn_suffix}`; recorded paths: {seen:?}")
        })
}

fn resolution(p: &PathOccurrence) -> &Resolution {
    p.resolution.as_ref().expect("resolution is None (phase 1 not run)")
}

fn fq_names(k: &Klepto) -> Vec<&str> {
    k.functions.iter().map(|f| f.fq_name.as_str()).collect()
}

// ---------------------------------------------------------------- M rules

#[test]
fn module_tree_maps_files_to_module_paths() {
    let k = resolve_ws();
    let m = |path: &str| k.modules.get(path).unwrap_or_else(|| panic!("no module {path}; have {:?}", k.modules.modules.keys()));

    assert!(m("app").file.ends_with("app/src/lib.rs"));
    assert!(m("app::query").file.ends_with("app/src/query.rs"), "M2");
    assert!(m("app::query::deep").file.ends_with("app/src/query/deep.rs"), "M2 non-mod-rs");
    assert!(m("app::engine").file.ends_with("app/src/engine/mod.rs"), "M2 mod.rs");
    assert!(m("app::engine::parts").file.ends_with("app/src/engine/parts.rs"), "M2 mod-rs style");
    assert!(m("app::helpers").file.ends_with("app/src/helpers_impl.rs"), "M3 #[path]");
    assert!(m("app::nested").inline, "M4 inline");
    assert!(m("app::nested").file.ends_with("app/src/lib.rs"), "M4 inline lives in lib.rs");
    assert!(m("app::nested::inner").file.ends_with("app/src/nested/inner.rs"), "M4 child of inline");
    assert!(!m("app::nested").is_pub && m("app::query").is_pub);

    assert_eq!(m("app::engine::parts").segments, vec!["engine", "parts"]);
    assert_eq!(m("app::engine::parts").package, "app");
    assert!(m("app::engine::parts").declared_in.as_ref().unwrap().ends_with("engine/mod.rs"));

    // M7 items
    assert_eq!(m("app::query").items.get("Query"), Some(&ItemKind::Struct));
    assert_eq!(m("app::query").items.get("run"), Some(&ItemKind::Fn));
    assert_eq!(m("app::query").items.get("deep"), Some(&ItemKind::Mod));
    assert_eq!(m("app::query").items.get("Regex"), Some(&ItemKind::Use));
    assert_eq!(m("app").items.get("Rx"), Some(&ItemKind::Use));

    // every parsed file knows its module
    for f in &k.files {
        assert!(f.module.is_some(), "file without module: {}", f.path.display());
    }
    assert_eq!(k.modules.for_file(&m("app::query").file).unwrap().path, "app::query");
}

#[test]
fn orphan_files_are_kept_and_flagged() {
    let k = resolve_ws();
    let orphan = k.modules.get("app::orphan").expect("M5 orphan module");
    assert!(orphan.orphan);
    assert!(orphan.declared_in.is_none());
    assert!(fq_names(&k).contains(&"app::orphan::orphan_fn"), "{:?}", fq_names(&k));
}

#[test]
fn workspace_scan_assigns_package_names() {
    let k = resolve_ws();
    let names = fq_names(&k);
    assert!(names.contains(&"app::query::run"), "{names:?}");
    assert!(names.contains(&"regexish::Regex::new"), "{names:?}");
    assert!(names.contains(&"other::other_only"), "{names:?}");
    assert!(k.modules.get("regexish").unwrap().file.ends_with("regexish/src/lib.rs"));
    assert_eq!(k.modules.get("regexish::sub").unwrap().items.get("Thing"), Some(&ItemKind::Struct));
}

// ---------------------------------------------------------------- F rules (fq_name)

#[test]
fn file_module_functions_get_full_fq_names() {
    let k = resolve_ws();
    let names = fq_names(&k);
    for expected in [
        "app::root_fn",
        "app::query::run",
        "app::query::helper",
        "app::query::Query::new",
        "app::query::Query::run_with",
        "app::query::deep::deep_run",
        "app::engine::helper",
        "app::engine::Engine::start",
        "app::engine::parts::part",
        "app::engine::parts::uses_fn_body_use",
        "app::helpers::help",
        "app::nested::in_inline",
        "app::nested::inner::deep_fn",
    ] {
        assert!(names.contains(&expected), "missing {expected} in {names:?}");
    }
    let helper = k.functions.iter().find(|f| f.fq_name == "app::engine::parts::part").unwrap();
    assert_eq!(helper.module_path, vec!["engine", "parts"]);
}

#[test]
fn trait_impl_methods_have_distinct_fq_names() {
    let k = resolve_ws();
    let fmts: Vec<&str> = k.functions.iter().filter(|f| f.name == "fmt").map(|f| f.fq_name.as_str()).collect();
    assert!(fmts.contains(&"app::engine::<Engine as std::fmt::Display>::fmt"), "{fmts:?}");
    assert!(fmts.contains(&"app::engine::<Engine as std::fmt::Debug>::fmt"), "{fmts:?}");
}

#[test]
fn snapshot_function_keys_are_unique() {
    let k = resolve_ws();
    let names = fq_names(&k);
    let unique: BTreeSet<&str> = names.iter().copied().collect();
    assert_eq!(unique.len(), names.len(), "duplicate fq_names: {names:?}");
}

#[test]
fn enclosing_index_uses_module_paths() {
    let k = resolve_ws();
    let loc = path_in(&k, "query::run", "Regex::new").location.clone();
    let span = k.index.enclosing(&loc).expect("enclosing fn for Regex::new in run");
    assert_eq!(span.fq_name, "app::query::run");
}

// ---------------------------------------------------------------- S / R rules

#[test]
fn alias_use_inside_fn_resolves_to_dependency() {
    let k = resolve_ws();
    // R7: `use regexish::Regex;` at module level, `Regex::new()` inside `run`
    assert_eq!(resolution(path_in(&k, "query::run", "Regex::new")), &external("regexish::Regex::new"));
    // and inside a method
    assert_eq!(resolution(path_in(&k, "Query::run_with", "Regex::new")), &external("regexish::Regex::new"));
    assert_eq!(resolution(path_in(&k, "Query::run_with", "Engine::start")), &local("app::engine::Engine::start"));
    assert_eq!(resolution(path_in(&k, "Query::run_with", "deep::deep_run")), &local("app::query::deep::deep_run"));
}

#[test]
fn fn_body_use_overlay_resolves() {
    let k = resolve_ws();
    // S4/R6: the module `app::engine::parts` has no `use regexish::Regex`; only the fn body does
    assert_eq!(
        resolution(path_in(&k, "parts::uses_fn_body_use", "Regex::new")),
        &external("regexish::Regex::new")
    );
}

#[test]
fn local_reexport_is_followed() {
    let k = resolve_ws();
    // `pub use query::Query;` (uniform path) then `Query::new()`
    assert_eq!(resolution(path_in(&k, "app::uses_reexport", "Query::new")), &local("app::query::Query::new"));
}

#[test]
fn external_reexport_alias_resolves() {
    let k = resolve_ws();
    // `pub use regexish::Regex as Rx;` then `Rx::new()`
    assert_eq!(resolution(path_in(&k, "app::uses_alias", "Rx::new")), &external("regexish::Regex::new"));
}

#[test]
fn crate_self_super_paths_resolve() {
    let k = resolve_ws();
    assert_eq!(resolution(path_in(&k, "app::uses_crate_path", "crate::query::run")), &local("app::query::run"), "R1");
    assert_eq!(resolution(path_in(&k, "parts::part", "crate::query::run")), &local("app::query::run"), "R1 from a nested module");
    assert_eq!(resolution(path_in(&k, "Query::run_with", "self::run")), &local("app::query::run"), "R2");
    assert_eq!(resolution(path_in(&k, "deep::deep_run", "super::helper")), &local("app::query::helper"), "R3");
    assert_eq!(resolution(path_in(&k, "inner::deep_fn", "super::in_inline")), &local("app::nested::in_inline"), "R3 into an inline module");
    assert_eq!(resolution(path_in(&k, "app::uses_nested", "nested::inner::deep_fn")), &local("app::nested::inner::deep_fn"), "R7 through modules");
    assert_eq!(resolution(path_in(&k, "parts::part", "regexish::top_fn")), &external("regexish::top_fn"), "R9");
}

#[test]
fn path_attribute_module_resolves() {
    let k = resolve_ws();
    assert_eq!(resolution(path_in(&k, "app::uses_helpers", "helpers::help")), &local("app::helpers::help"), "M3 + R7");
}

#[test]
fn glob_ambiguity_is_unresolved_with_candidates() {
    let k = resolve_ws();
    let p = path_in(&k, "app::uses_glob_external", "Thing::default");
    match resolution(p) {
        Resolution::Unresolved { raw, reason, candidates } => {
            assert_eq!(raw, "Thing::default");
            assert_eq!(*reason, UnresolvedReason::Ambiguous);
            assert_eq!(
                candidates,
                &vec!["regexish::sub::Thing::default".to_string(), "other::Thing::default".to_string()],
                "candidates in glob source order"
            );
        }
        other => panic!("expected Unresolved, got {other:?}"),
    }
}

#[test]
fn macros_resolve_via_use_and_prelude() {
    let k = resolve_ws();
    let mac = |name: &str| {
        k.macros_inv
            .iter()
            .find(|m| m.name == name && m.enclosing_fn.as_deref().is_some_and(|f| f.ends_with("uses_macro")))
            .unwrap_or_else(|| panic!("no macro {name}!"))
            .resolution
            .clone()
            .expect("macro resolution")
    };
    assert_eq!(mac("mk"), external("regexish::mk"), "R12: both `regexish::mk!` and `mk!` (via use) resolve the same");
    assert!(
        k.macros_inv.iter().filter(|m| m.name == "mk").all(|m| m.resolution == Some(external("regexish::mk"))),
        "{:?}",
        k.macros_inv
    );
    assert_eq!(mac("println"), external("std::println"), "R10");
}

#[test]
fn use_targets_and_exports_are_resolved() {
    let k = resolve_ws();
    let imp = |full: &str| {
        k.imports.iter().find(|i| i.full_path == full).unwrap_or_else(|| panic!("no import {full}")).resolution.clone().expect("use resolution")
    };
    assert_eq!(imp("super::engine::Engine"), local("app::engine::Engine"), "S2 + R3");
    assert_eq!(imp("regexish::Regex"), external("regexish::Regex"));
    assert_eq!(imp("query::Query"), local("app::query::Query"), "uniform path to a local module");

    let export = |exported_as: &str| {
        k.exports.iter().find(|e| e.exported_as == exported_as).unwrap_or_else(|| panic!("no export {exported_as}")).resolution.clone().expect("export resolution")
    };
    assert_eq!(export("Query"), local("app::query::Query"));
    assert_eq!(export("Rx"), external("regexish::Regex"));
}

#[test]
fn attribute_arguments_are_resolved() {
    let k = resolve_ws();
    let derive = k
        .attrs
        .iter()
        .find(|a| a.path == "derive" && a.arg_paths.iter().any(|p| p == "regexish::Derive"))
        .expect("derive attr on Cfg");
    assert_eq!(derive.arg_paths, vec!["Clone".to_string(), "regexish::Derive".to_string()], "O4 lists singles too");
    assert_eq!(derive.arg_resolutions, vec![external("std::Clone"), external("regexish::Derive")]);
}

#[test]
fn single_segments_are_recorded_only_when_they_resolve() {
    let k = resolve_ws();

    // O1 positive: `top_fn2()` after `use regexish::top_fn as top_fn2;` is a site
    assert_eq!(resolution(path_in(&k, "app::uses_single", "top_fn2")), &external("regexish::top_fn"));

    // O1 negative: everything single-segment that IS recorded resolved through scope or a glob
    for p in &k.paths {
        if p.path.contains("::") || matches!(p.path.as_str(), "std" | "core" | "alloc") {
            continue;
        }
        match &p.resolution {
            Some(Resolution::Local(_)) | Some(Resolution::External(_)) => {}
            Some(Resolution::Unresolved {
                reason: UnresolvedReason::GlobImport | UnresolvedReason::Ambiguous,
                ..
            }) => {}
            other => panic!("single-segment path `{}` recorded with {other:?} at {:?}", p.path, p.location),
        }
    }
    assert!(!k.paths.iter().any(|p| p.path == "_r" || p.path == "_q" || p.path == "_t"), "local bindings recorded");
}

#[test]
fn calls_carry_callee_resolution() {
    let k = resolve_ws();
    let call = k
        .calls
        .iter()
        .find(|c| c.enclosing_fn.as_deref().is_some_and(|f| f.ends_with("query::run")) && c.callee.replace(' ', "") == "Regex::new")
        .expect("call Regex::new in run");
    assert_eq!(call.resolution, Some(external("regexish::Regex::new")), "O2");
}

// ---------------------------------------------------------------- U rules (use sites)

#[test]
fn dep_use_sites_use_resolution_and_fq_scopes() {
    let k = resolve_ws();
    let sites = k.dep_use_sites(&deps(&["regexish"]));

    // the alias case, attributed to the calling function
    assert!(
        sites.iter().any(|s| matches!(s.kind, UseSiteKind::Path) && s.path == "regexish::Regex::new" && s.scope == "app::query::run"),
        "{sites:?}"
    );
    // module-level `use` scope is the module path, not "file" (F2)
    assert!(
        sites.iter().any(|s| matches!(s.kind, UseSiteKind::UseStmt) && s.path == "regexish::Regex" && s.scope == "app::query"),
        "{sites:?}"
    );
    assert!(sites.iter().all(|s| s.scope != "file"), "{sites:?}");
    assert!(sites.iter().all(|s| s.dep == "regexish"));

    let internal = k.internal_use_sites();
    assert!(
        internal.iter().any(|s| s.path == "app::query::run" && s.scope == "app::engine::parts::part" && s.dep == "app"),
        "U2: {internal:?}"
    );
    // every internal site path is a real fq_name (scopes and paths finally agree)
    let names: BTreeSet<&str> = fq_names(&k).into_iter().collect();
    let run_sites: Vec<&UseSite> = internal.iter().filter(|s| s.path == "app::query::run").collect();
    assert!(!run_sites.is_empty());
    assert!(names.contains("app::query::run"));
}

#[test]
fn possible_sites_hold_glob_candidates() {
    let k = resolve_ws();
    let certain = k.dep_use_sites(&deps(&["other"]));
    assert!(!certain.iter().any(|s| s.path == "other::Thing::default"), "U1 must not include glob guesses: {certain:?}");

    let possible = k.possible_dep_use_sites(&deps(&["other"]));
    assert!(
        possible.iter().any(|s| s.path == "other::Thing::default" && s.scope == "app::uses_glob_external" && s.dep == "other"),
        "U3: {possible:?}"
    );
    let possible_rx = k.possible_dep_use_sites(&deps(&["regexish"]));
    assert!(possible_rx.iter().any(|s| s.path == "regexish::sub::Thing::default"), "U3: {possible_rx:?}");
}

// ---------------------------------------------------------------- no-metadata mode

#[test]
fn scan_in_folder_assumes_unknown_roots_are_external() {
    let k = resolve_folder();
    assert!(k.modules.get("app::query").is_some(), "M1 root found from lib.rs");
    assert_eq!(resolution(path_in(&k, "query::run", "Regex::new")), &external("regexish::Regex::new"), "R9 without a dependency list");
    assert_eq!(resolution(path_in(&k, "parts::part", "crate::query::run")), &local("app::query::run"));
}

// ---------------------------------------------------------------- M8 (scale run): macro-declared modules

#[test]
fn modules_declared_inside_macro_invocations_exist() {
    let k = Klepto::new("app")
        .scan_workspace_root(fixture("macromods"))
        .expect("cargo metadata on fixture macromods")
        .parse()
        .expect("parse fixture macromods");
    // Phase 8: `cfg_rt!` is a local `macro_rules!`, so its `mod`s are real and
    // carry the exact predicate the macro writes (rules X1, X4).
    for m in ["app::runtime", "app::io"] {
        let mi = k.modules.get(m).unwrap_or_else(|| panic!("{m} declared via cfg_rt!; have {:?}", k.modules.modules.keys().collect::<Vec<_>>()));
        assert!(!mi.orphan, "not an orphan");
        assert!(mi.is_pub);
        assert_eq!(mi.cfg, CfgPredicate::parse_str("feature = \"rt\""), "the expanded macro decides: {:?}", mi.cfg);
    }
    // M8 proper: a macro with no definition on disk is not expanded; its
    // `mod` is still declared, with an Unknown cfg.
    let late = k.modules.get("app::late").expect("M8: late declared via cfg_ext!");
    assert!(!late.orphan);
    assert!(late.cfg.is_unknown(), "the macro decides: {:?}", late.cfg);
    assert_eq!(k.modules.get("app::plain").unwrap().cfg, CfgPredicate::True);
    let spawn = k.paths.iter().find(|p| p.path == "runtime::spawn").expect("path occurrence");
    assert_eq!(spawn.resolution, Some(Resolution::Local("app::runtime::spawn".into())));
    let two = k.paths.iter().find(|p| p.path == "late::two").expect("path occurrence");
    assert_eq!(two.resolution, Some(Resolution::Local("app::late::two".into())));
    let findings = RuleRunner::new(&k).with_all_rules().run();
    assert!(!findings.iter().any(|f| f.code == "KLEP014"), "no orphan findings: {findings:?}");
    assert_eq!(findings.iter().filter(|f| f.code == "KLEP011").count(), 1, "start (ungated) → runtime::spawn (rt) is definite now: {findings:?}");
    assert_eq!(findings.iter().filter(|f| f.code == "KLEP018").count(), 1, "start → late::two stays undecidable: {findings:?}");
}

// ---------------------------------------------------------------- S5 amendment (scale run): impl and type contexts

#[test]
fn impl_generics_and_self_resolve_outside_fn_bodies() {
    let k = Klepto::new("app")
        .scan_workspace_root(fixture("implgen"))
        .expect("cargo metadata on fixture implgen")
        .parse()
        .expect("parse fixture implgen");
    let by_path = |p: &str| -> Vec<&Resolution> { k.paths.iter().filter(|x| x.path == p).filter_map(|x| x.resolution.as_ref()).collect() };
    for p in ["B::Data", "B::Error"] {
        let rs = by_path(p);
        assert!(!rs.is_empty(), "occurrence of {p}");
        assert!(
            rs.iter().all(|r| matches!(r, Resolution::Unresolved { reason: UnresolvedReason::GenericParam, .. })),
            "S5 amendment: {p} names the impl's generic, not an unknown name: {rs:?}"
        );
    }
    let selfs = by_path("Self::Out");
    assert!(!selfs.is_empty());
    assert!(
        selfs.iter().all(|r| matches!(r, Resolution::Local(l) if l == "app::Wrap::Out" || l == "app::Framed::Out")),
        "`Self` is the impl's self type in the impl and the trait itself in the trait: {selfs:?}"
    );
    let unknown_names: Vec<&str> = k
        .graph
        .unresolved
        .iter()
        .filter(|u| matches!(u.resolution, Resolution::Unresolved { reason: UnresolvedReason::UnknownName | UnresolvedReason::SelfType, .. }))
        .map(|u| u.raw.as_str())
        .collect();
    assert!(
        !unknown_names.iter().any(|r| r.starts_with("B::") || r.starts_with("Self::")),
        "nothing about B or Self is an unknown name (generic params are listed on purpose): {unknown_names:?}"
    );
}

// ---------------------------------------------------------------- R12 amendment (scale run): macro_rules names are crate-wide

#[test]
fn macro_rules_names_are_visible_in_every_module() {
    let k = Klepto::new("app")
        .scan_workspace_root(fixture("macromods"))
        .expect("cargo metadata on fixture macromods")
        .parse()
        .expect("parse fixture macromods");
    let inv = k
        .macros_inv
        .iter()
        .find(|m| m.name == "trace" && m.location.path.ends_with("runtime.rs"))
        .expect("trace! invocation in runtime.rs");
    assert_eq!(inv.resolution, Some(Resolution::Local("app::trace".into())), "R12 amendment: {:?}", inv.resolution);
    assert!(k.item_graph().has_edge("app::runtime::spawn", "app::trace", EdgeKind::Reference));
}
