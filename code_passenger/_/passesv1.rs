// src/analysis/passes.rs
// use crate::engine::EngineOutput;
// use super::state::AnalysisState;

pub trait Pass: Send + Sync {
    fn id(&self) -> &'static str;
    fn apply(&self, raw: &EngineOutput, st: AnalysisState) -> AnalysisState;
}



// src/analysis/passes.rs
use im::{OrdMap, OrdSet};
use crate::analysis::action::Action;
use crate::engine::EngineOutput;
use crate::model::FileReport;
use super::state::{AnalysisState, FileAnalysis};
use super::produce::produce;

pub struct BuildFileViews;

impl Pass for BuildFileViews {
    fn id(&self) -> &'static str { "build_file_views" }

    fn apply(&self, raw: &EngineOutput, st: AnalysisState) -> AnalysisState {
        produce(&st, |draft| {
            for r in &raw.reports {
                draft.files.insert(r.document.relative_path.clone(), file_view(r));
            }
        })
    }
}

fn file_view(r: &FileReport) -> FileAnalysis {
    let used_external: OrdSet<String> = r.used.packages.iter().cloned().collect();
    let used_internal_roots: OrdSet<String> = r.used.modules
        .iter()
        .map(|p| p.split("::").next().unwrap_or(p).to_string())
        .collect();

    // You already have these counts in FileReport — just convert to im::OrdMap if needed
    let external_symbol_counts = r.external_dep_symbol_counts.clone().into_iter().collect();
    let internal_symbol_counts = r.internal_dep_symbol_counts.clone().into_iter().collect();

    FileAnalysis {
        used_external,
        used_internal_roots,
        external_symbol_counts,
        internal_symbol_counts,
    }
}


pub struct BuildTotals {
    pub top_n: usize,
}

impl Default for BuildTotals {
    fn default() -> Self { Self { top_n: 25 } }
}

impl Pass for BuildTotals {
    fn id(&self) -> &'static str { "build_totals" }

    fn apply(&self, raw: &crate::engine::EngineOutput, mut st: AnalysisState) -> AnalysisState {
        use std::collections::BTreeMap;

        // dep -> sym -> count across whole crate
        let mut ext_sym_totals: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();

        for r in &raw.reports {
            // external counts
            for (dep, syms) in &r.external_dep_symbol_counts {
                for (sym, c) in syms {
                    *ext_sym_totals.entry(dep.clone()).or_default().entry(sym.clone()).or_insert(0) += *c;
                    *st.crate_totals.external_dep_hits.entry(dep.clone()).or_insert(0) += *c;
                }
            }

            // internal counts (roots like crate/self/super/<crate>)
            for (root, syms) in &r.internal_dep_symbol_counts {
                let mut total = 0usize;
                for (_sym, c) in syms {
                    total += *c;
                }
                *st.crate_totals.internal_root_hits.entry(root.clone()).or_insert(0) += total;
            }
        }

        // Build top symbols per dep
        for (dep, syms) in ext_sym_totals {
            let mut v: Vec<(String, usize)> = syms.into_iter().collect();
            v.sort_by_key(|(_k, c)| std::cmp::Reverse(*c));
            v.truncate(self.top_n);
            st.crate_totals.top_external_symbols.insert(dep, v);
        }

        st
    }
}

use crate::engine::EngineOutput;

pub mod action;
pub mod checks;
pub mod passes;
pub mod produce;
pub mod reducer;
pub mod state;
pub mod store;
