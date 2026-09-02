//! Extraction: items, imports, calls, attributes and the built-in lints.

mod common;

use common::*;
// Also proves `use klepto::*` compiles downstream (the crate root used to
// re-export a module named `klepto` that collided with the crate name).
use klepto::*;
use std::collections::BTreeSet;

fn deps(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

#[test]
fn unparseable_file_is_skipped_when_asked_and_fatal_otherwise() {
    let dir = fixture("broken");

    let k = Klepto::new("broken").scan_in_folder(&dir).ignore_parse_errors(true).parse().unwrap();
    assert_eq!(k.files.len(), 1);
    assert_eq!(file_name(&k.files[0].path), "good.rs");
    assert_eq!(k.skipped_files.len(), 1);
    assert_eq!(file_name(&k.skipped_files[0].0), "bad.rs");

    let err = Klepto::new("broken").scan_in_folder(&dir).parse().unwrap_err();
    assert!(matches!(err, KleptoError::Parse { .. }), "{err}");
}

#[test]
fn exclude_generated_matches_file_names_not_substrings() {
    let k = Klepto::new("gen").scan_in_folder(fixture("gen")).exclude_generated().unwrap().parse().unwrap();
    let mut files: Vec<String> = k.files.iter().map(|f| file_name(&f.path)).collect();
    files.sort();
    assert_eq!(files, vec!["keep_me.rs", "pbkdf.rs"]);
}

#[test]
fn import_origins_are_classified() {
    let k = ws();
    let origin = |full: &str| {
        k.imports.iter().find(|i| i.full_path == full).unwrap_or_else(|| panic!("no import {full}")).origin.clone()
    };
    assert_eq!(origin("mydep::Widget"), Some(ImportOrigin::WorkspaceMember));
    assert_eq!(origin("std::collections::HashMap"), Some(ImportOrigin::Std));
    assert_eq!(origin("outside::outside_fn"), Some(ImportOrigin::Dependency));
}

#[test]
fn import_query_origin_filters_apply() {
    let k = ws();

    let ws_only = k.imports().workspace_only().collect();
    assert!(!ws_only.is_empty());
    assert!(ws_only.iter().all(|i| i.origin == Some(ImportOrigin::WorkspaceMember)), "{ws_only:?}");

    let std_only = k.imports().stdish_only().collect();
    assert!(!std_only.is_empty());
    assert!(std_only.iter().all(|i| i.root == "std"), "{std_only:?}");
}

#[test]
fn statement_position_macros_are_captured() {
    let k = app_folder();
    let println = k.find_macro_invocations("println");
    assert_eq!(println.len(), 1, "{:?}", k.macros_inv);
    assert!(println[0].enclosing_fn.as_deref().unwrap().ends_with("shouty"));
    assert_eq!(k.find_macro_invocations("panic").len(), 1);
}

#[test]
fn attribute_paths_are_captured_as_attribute_use_sites() {
    let k = app_folder();

    let derive = k.attrs.iter().find(|a| a.path == "derive").expect("derive attr");
    assert_eq!(derive.arg_paths, vec!["mydep::Derive".to_string()]);
    assert!(k.attrs.iter().any(|a| a.path == "mydep"), "{:?}", k.attrs);

    let sites = k.dep_use_sites(&deps(&["mydep"]));
    let attr_sites: Vec<&UseSite> = sites.iter().filter(|s| matches!(s.kind, UseSiteKind::Attribute)).collect();
    assert!(attr_sites.iter().any(|s| s.path == "mydep::Derive"), "{attr_sites:?}");
    assert!(attr_sites.iter().any(|s| s.path == "mydep"), "{attr_sites:?}");
}

#[test]
fn use_inside_fn_body_is_captured_with_its_scope() {
    let k = app_folder();

    let imp = k.imports.iter().find(|i| i.full_path == "mydep::helper").expect("fn-body use");
    assert!(imp.enclosing_fn.as_deref().unwrap().ends_with("uses_inside"), "{imp:?}");

    let sites = k.dep_use_sites(&deps(&["mydep"]));
    assert!(
        sites.iter().any(|s| matches!(s.kind, UseSiteKind::UseStmt) && s.path == "mydep::helper" && s.scope.ends_with("uses_inside")),
        "{sites:?}"
    );
}

#[test]
fn extern_crate_is_captured() {
    let k = app_folder();

    let ec = k.extern_crates.iter().find(|e| e.name == "outside").expect("extern crate");
    assert_eq!(ec.alias.as_deref(), Some("out"));

    let sites = k.dep_use_sites(&deps(&["outside"]));
    assert!(sites.iter().any(|s| matches!(s.kind, UseSiteKind::ExternCrate)), "{sites:?}");
}

#[test]
fn path_style_macro_call_is_one_use_site() {
    let k = app_folder();
    let sites = k.dep_use_sites(&deps(&["mydep"]));
    let mk: Vec<&UseSite> = sites.iter().filter(|s| s.path == "mydep::mk").collect();
    assert_eq!(mk.len(), 1, "{mk:?}");
    assert!(matches!(mk[0].kind, UseSiteKind::MacroCall));
}

#[test]
fn klep002_flags_unwrap_but_not_unwrap_or() {
    let k = app_folder();
    let findings = k.rules().with_default_rules().run();
    let klep002: Vec<&str> = findings.iter().filter(|f| f.code == "KLEP002").map(|f| f.message.as_str()).collect();
    assert!(klep002.iter().any(|m| m.contains("really_panics")), "{klep002:?}");
    assert!(!klep002.iter().any(|m| m.contains("::top:")), "{klep002:?}");
}

#[test]
fn klep003_sees_statement_position_panic() {
    let k = app_folder();
    let findings = k.rules().with_default_rules().run();
    assert!(findings.iter().any(|f| f.code == "KLEP003" && f.message.contains("shouty")), "{findings:?}");
}

#[test]
fn workspace_scan_does_not_write_a_lockfile() {
    let lock = fixture("ws").join("Cargo.lock");
    let _ = std::fs::remove_file(&lock);
    let _ = ws();
    assert!(!lock.exists(), "scan_workspace_root resolved dependencies and wrote {}", lock.display());
}

#[test]
fn locations_have_line_numbers_by_default() {
    let k = app_folder();
    let top = k.functions.iter().find(|f| f.name == "top").unwrap();
    assert!(top.location.line.is_some(), "{top:?}");
}

#[test]
fn table_report_truncates_multibyte_paths_safely() {
    let f = Finding {
        severity: Severity::Warn,
        code: "KLEP000".into(),
        message: "m".into(),
        location: FileLocation {
            path: "/tmp/éééééééééééééééééééééééééééééééé/x.rs".into(),
            line: Some(1),
            column: Some(1),
        },
        extra: serde_json::json!({}),
    };
    let table = findings_to_table(&[f]);
    assert!(table.contains("..."), "{table}");
}
