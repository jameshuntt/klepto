use std::collections::BTreeSet;

use crate::model::UsedSymbols;
use crate::packs::rust::RustPack;
use crate::packs::rust::helpers::norm_ident;
use regex::Regex;


/// Helper used by core (since usage depends on optional deps list).
pub fn detect_usage_with_optional_deps(
    content: &str,
    optional_deps: &std::collections::BTreeSet<String>,
) -> UsedSymbols {
    let (deps, set) = RustPack::build_usage_set(optional_deps);
    let matches = set.matches(content);

    let mut used = UsedSymbols::default();
    // pats contain multiple entries per dep; map by prefix grouping
    // simplest: re-check per dep by string contains once any match is found:
    // BUT we already have indices; so we do a conservative approach:
    for i in matches.into_iter() {
        // we added 4 patterns per dep
        let dep_idx = i / 4;
        if let Some(dep) = deps.get(dep_idx) {
            used.packages.insert(dep.clone());
        }
    }
    used
}

pub fn detect_usage_with_deps(
    content: &str,
    deps: &std::collections::BTreeSet<String>,
    crate_name: &str,
) -> UsedSymbols {
    let (deps_vec, set) = RustPack::build_usage_set(deps);
    let matches = set.matches(content);

    let mut used = UsedSymbols::default();
    for i in matches.into_iter() {
        let dep_idx = i / 4; // 4 patterns per dep
        if let Some(dep) = deps_vec.get(dep_idx) {
            used.packages.insert(dep.clone());
        }
    }

    used.modules = detect_internal_paths(content, crate_name);
    used
}

/// Collect internal crate path usage like:
/// - crate::engine::RunContext
/// - super::foo::Bar
/// - self::x
/// - code_passenger::model::FileReport   (crate name, common in tests/)
fn detect_internal_paths(content: &str, crate_name: &str) -> BTreeSet<String> {
    let crate_name = norm_ident(crate_name);
    let root_alt = format!("(?:crate|self|super|{})", regex::escape(&crate_name));

    // Capture root + tail path. Tail may include multiple :: segments.
    let re = Regex::new(&format!(
        r"(?m)(?:^|[^A-Za-z0-9_])(?:::)?\s*({})\s*::\s*([A-Za-z_][A-Za-z0-9_]*(?:\s*::\s*[A-Za-z_][A-Za-z0-9_]*)*)",
        root_alt
    )).expect("valid internal path regex");

    let split = Regex::new(r"\s*::\s*").expect("valid split regex");

    let mut out = BTreeSet::new();

    for cap in re.captures_iter(content) {
        let root = cap.get(1).unwrap().as_str();
        let tail = cap.get(2).unwrap().as_str();
        let parts: Vec<&str> = split.split(tail).filter(|s| !s.is_empty()).collect();
        if parts.is_empty() {
            continue;
        }

        // Always record root::top (crate::engine)
        out.insert(format!("{root}::{}", parts[0]));

        // Also record root::top::second when present (crate::packs::rust)
        if parts.len() >= 2 {
            out.insert(format!("{root}::{}::{}", parts[0], parts[1]));
        }
    }

    out
}