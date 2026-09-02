use std::collections::BTreeSet;

use crate::model::{FileReport, ScaffoldKind, ScaffoldOutput, ScaffoldRequest, UsedSymbols};
use crate::plans::HeaderPlan;
use crate::scaffolds::ScaffoldPlan;
use regex::{Regex, RegexSet};

#[derive(Default)]
pub struct RustPack;

impl RustPack {
    pub const BEGIN_MARK: &'static str = "//! code_passenger:begin";
    pub const END_MARK: &'static str = "//! code_passenger:end";

    #[allow(unused)]
    fn build_usage_set_original(optional_deps: &std::collections::BTreeSet<String>) -> (Vec<String>, RegexSet) {
        // Match common Rust crate usage:
        // - tokio::something
        // - ::tokio::something
        // - #[tokio::main]
        // - use tokio::...
        // - extern crate tokio;
        let deps: Vec<String> = optional_deps.iter().cloned().collect();
        let mut pats = Vec::with_capacity(deps.len() * 3);

        for d in &deps {
            let e = regex::escape(d);
            pats.push(format!(r"(?m)(?:^|[^A-Za-z0-9_])::?\s*{}\s*::", e));      // tokio:: or ::tokio::
            pats.push(format!(r"(?m)^\s*use\s+::?\s*{}\b", e));                  // use tokio
            pats.push(format!(r"(?m)^\s*extern\s+crate\s+{}\s*;", e));           // extern crate tokio;
            pats.push(format!(r"(?m)#\s*\[\s*::?\s*{}\s*::", e));                // #[tokio::...]
        }

        let set = RegexSet::new(&pats).expect("valid regex set");
        (deps, set)
    }


    fn build_usage_set(deps: &std::collections::BTreeSet<String>) -> (Vec<String>, RegexSet) {
        let deps: Vec<String> = deps.iter().cloned().collect();
        let mut pats = Vec::with_capacity(deps.len() * 4);

        for d in &deps {
            let e = regex::escape(d);

            // Match:
            // - tokio::something
            // - ::tokio::something
            pats.push(format!(r"(?m)(?:^|[^A-Za-z0-9_])(?:::)?\s*{}\s*::", e));

            // Match:
            // - use tokio::...
            // - use ::tokio::...
            pats.push(format!(r"(?m)^\s*use\s+(?:::)?\s*{}\b", e));

            // Match:
            // - extern crate tokio;
            pats.push(format!(r"(?m)^\s*extern\s+crate\s+{}\s*;", e));

            // Match:
            // - #[tokio::main]
            // - #[::tokio::main]
            pats.push(format!(r"(?m)#\s*\[\s*(?:::)?\s*{}\s*::", e));
        }

        let set = RegexSet::new(&pats).expect("valid regex set");
        (deps, set)
    }

    fn replace_or_insert(original: &str, header: &str) -> String {
        // Replace existing marked region if present
        if let Some(beg) = original.find(Self::BEGIN_MARK) {
            if let Some(end) = original.find(Self::END_MARK) {
                let end_line = original[end..].find('\n').map(|i| end + i + 1).unwrap_or(original.len());
                let mut out = String::new();
                out.push_str(&original[..beg]);
                out.push_str(header);
                out.push_str(&original[end_line..]);
                return out;
            }
        }

        // Insert after leading inner attrs `#![...]` (keep them first)
        let mut idx = 0usize;
        for line in original.lines() {
            let len = line.len();
            let line_with_nl = len + 1; // assume '\n' (works fine for typical files)
            let t = line.trim_start();
            if t.starts_with("#![") {
                idx += line_with_nl;
                continue;
            }
            break;
        }

        let mut out = String::new();
        out.push_str(&original[..idx]);
        out.push_str(header);
        out.push_str(&original[idx..]);
        out
    }
}

impl super::LanguagePack for RustPack {
    fn id(&self) -> &'static str { "rust" }

    fn matches_path(&self, path: &std::path::Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("rs")
    }

    fn detect_usage(&self, content: &str) -> UsedSymbols {
        // NOTE: usage detection depends on optional_deps; pack alone can’t know them.
        // So core will call `RustPack::detect_usage_with_optional_deps(...)`.
        // This method exists for trait completeness, but is unused.
        let _ = content;
        UsedSymbols::default()
    }

    fn render_header(&self, plan: &dyn HeaderPlan, report: &FileReport) -> String {
        plan.render_for_rust(report, Self::BEGIN_MARK, Self::END_MARK)
    }

    fn apply_header(&self, original: &str, header: &str) -> String {
        Self::replace_or_insert(original, header)
    }


    // inside impl super::LanguagePack for RustPack:
    fn scaffold(&self, plan: &dyn ScaffoldPlan, req: ScaffoldRequest) -> crate::error::Result<ScaffoldOutput> {
        let snake = req.name;

        let mut path = if matches!(req.kind, ScaffoldKind::Test) {
            std::path::PathBuf::from("tests")
        } else {
            std::path::PathBuf::from("src")
        };

        for seg in req.module_path {
            path.push(seg);
        }
        path.push(format!("{snake}.rs"));

        let ty = to_type_name(&snake);

        let tpl = plan.template(req.kind);
        let content = tpl
            .replace("{snake}", &snake)
            .replace("{Ty}", &ty);

        Ok(ScaffoldOutput { files: vec![(path, content)] })
    }

}

fn to_type_name(snake: &str) -> String {
    let mut out = String::new();
    let mut up = true;
    for c in snake.chars() {
        if c == '_' { up = true; continue; }
        if up { out.extend(c.to_uppercase()); up = false; }
        else { out.push(c); }
    }
    if out.is_empty() { "Thing".into() } else { out }
}

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

fn norm_ident(s: &str) -> String {
    s.replace('-', "_")
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