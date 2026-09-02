// src/reporting/mod.rs

use std::collections::{
//     BTreeMap,
    HashMap
};

#[derive(Debug, Clone)]
pub struct FeatureNote {
    pub feature_name: String,
    pub deps: Vec<String>,
    pub scope: Vec<String>,
    pub corpus: bool,
}

#[derive(Debug, Clone)]
pub struct FileReport {
    pub filename: String,
    pub description: Option<String>,
    pub used: bool,
    pub features: Vec<FeatureNote>,
}

/// Extract unused features from a set of reports.
pub fn extract_unused_features(reports: &[FileReport]) -> HashMap<String, Vec<String>> {
    let mut unused = HashMap::new();
    for file in reports {
        let list: Vec<String> = file.features
            .iter()
            .filter(|f| f.scope.is_empty())
            .map(|f| f.feature_name.clone())
            .collect();

        if !list.is_empty() {
            unused.insert(file.filename.clone(), list);
        }
    }
    unused
}

/// Generate a Rust doc header comment block.
pub fn generate_doc_header(report: &FileReport) -> String {
    let mut header = String::new();

    header.push_str("//! ----------------------------------------------\n");
    header.push_str("//! DOCUMENT DETAILS -----------------------------\n\n");
    header.push_str(&format!("//! filename:{}\n", report.filename));
    if let Some(desc) = &report.description {
        header.push_str(&format!("//! description:{}\n", desc));
    }
    header.push_str("//! usages:");
    header.push_str(if report.used { "used\n" } else { "not used\n" });
    header.push_str("//!\n");

    header.push_str("//! ----------------------------------------------\n");
    header.push_str("//! FEATURE NOTES --------------------------------\n\n");

    for feat in &report.features {
        header.push_str(&format!("//! feature_name:{}\n", feat.feature_name));
        header.push_str(&format!("//! deps:{:?}\n", feat.deps));
        header.push_str(&format!("//! scope:{:?}\n", feat.scope));
        header.push_str(&format!("//! corpus:{}\n\n", feat.corpus));
    }

    header.push_str("//! ----------------------------------------------\n");
    header.push_str("//! CORPUS FEATURES ------------------------------\n\n");

    for feat in &report.features {
        if feat.corpus {
            header.push_str(&format!("#![cfg(feature = \"{}\")]\n", feat.feature_name));
        }
    }

    header
}

/// Render a crate usage summary.
pub fn render_crate_usage_summary(reports: &[FileReport]) -> String {
    let mut summary = String::new();
    summary.push_str("Crate Feature Usage Report:\n\n");
    for report in reports {
        summary.push_str(&format!("File: {}\n", report.filename));
        for feat in &report.features {
            summary.push_str(&format!(
                "  - [{}] Feature: {} | Scope: {}\n",
                if feat.corpus { "*" } else { " " },
                feat.feature_name,
                if feat.scope.is_empty() { "(unused)".to_string() } else { feat.scope.join(", ") }
            ));
        }
        summary.push('\n');
    }
    summary
}

// src/report/doc_report.rs
use std::collections::{BTreeMap, BTreeSet};
use crate::model::{DocumentReport, SymbolUsageCounts};

/// Represents the auto-generated doc header for a Rust file.
#[derive(Debug, Clone)]
pub struct DocHeader {
    pub relative_path: String,
    pub used_deps: BTreeSet<String>,
    pub used_mods: BTreeSet<String>,
    pub corpus_features: BTreeSet<String>,
    pub external_counts: BTreeMap<String, usize>,
    pub internal_counts: BTreeMap<String, usize>,
    pub unused_features: Vec<String>,
}

impl DocHeader {
    pub fn from_report(report: &DocumentReport, all_features: &BTreeSet<String>) -> Self {
        let used_features = &report.corpus_features;
        let unused = all_features
            .difference(used_features)
            .cloned()
            .collect::<Vec<_>>();

        DocHeader {
            relative_path: report.document.relative_path.clone(),
            used_deps: report.used.packages.clone(),
            used_mods: report.used.modules.clone(),
            corpus_features: used_features.clone(),
            external_counts: sorted_top_n(&report.external_dep_symbol_counts, 5),
            internal_counts: sorted_top_n(&report.internal_dep_symbol_counts, 5),
            unused_features: unused,
        }
    }

    pub fn to_doc_string(&self) -> String {
        let mut out = String::new();
        out.push_str("//! Auto-generated file summary\n");
        out.push_str(&format!("//! Path: {}\n", self.relative_path));

        if !self.used_deps.is_empty() {
            out.push_str("//!\n//! Used Dependencies:\n");
            for dep in &self.used_deps {
                out.push_str(&format!("//! - {}\n", dep));
            }
        }

        if !self.used_mods.is_empty() {
            out.push_str("//!\n//! Used Modules:\n");
            for module in &self.used_mods {
                out.push_str(&format!("//! - {}\n", module));
            }
        }

        if !self.corpus_features.is_empty() {
            out.push_str("//!\n//! Features Used:\n");
            for feat in &self.corpus_features {
                out.push_str(&format!("//! - {}\n", feat));
            }
        }

        if !self.unused_features.is_empty() {
            out.push_str("//!\n//! Unused Features:\n");
            for feat in &self.unused_features {
                out.push_str(&format!("//! - {}\n", feat));
            }
        }

        out.push_str("//!\n//! Top External Symbols:\n");
        for (sym, count) in &self.external_counts {
            out.push_str(&format!("//! - {} ({}x)\n", sym, count));
        }

        out.push_str("//!\n//! Top Internal Symbols:\n");
        for (sym, count) in &self.internal_counts {
            out.push_str(&format!("//! - {} ({}x)\n", sym, count));
        }

        out
    }
}

/// Extract the top N symbol usage counts and sort alphabetically by symbol.
fn sorted_top_n(map: &SymbolUsageCounts, n: usize) -> BTreeMap<String, usize> {
    let mut entries: Vec<_> = map.iter().map(|(k, &v)| (k.clone(), v)).collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    entries.truncate(n);
    entries.into_iter().collect()
}