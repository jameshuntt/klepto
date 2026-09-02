use std::collections::BTreeMap;

use crate::analysis::{action::Action, passes::Pass, state::{AnalysisState, Phase}};

pub struct BuildTotals {
    pub top_n: usize,
}
impl Default for BuildTotals {
    fn default() -> Self {
        Self { top_n: 25 }
    }
}

impl Pass for BuildTotals {
    fn id(&self) -> &'static str {
        "build_totals"
    }

    fn actions(&self, raw: &crate::engine::EngineOutput, _st: &AnalysisState) -> Vec<Action> {
        let mut out = Vec::new();

        // dep -> sym -> count across whole crate
        let mut ext_sym_totals: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();

        for r in &raw.reports {
            // external counts
            for (dep, syms) in &r.external_dep_symbol_counts {
                for (sym, c) in syms {
                    *ext_sym_totals
                        .entry(dep.clone())
                        .or_default()
                        .entry(sym.clone())
                        .or_insert(0) += *c;
                    out.push(Action::IncExternalDepHit {
                        dep: dep.clone(),
                        by: *c,
                    });
                }
            }

            // internal counts (roots like crate/self/super/<crate>)
            for (root, syms) in &r.internal_dep_symbol_counts {
                let total = syms.values().copied().sum::<usize>();
                if total > 0 {
                    out.push(Action::IncInternalRootHit {
                        root: root.clone(),
                        by: total,
                    });
                }
            }
        }

        // top symbols per dep
        for (dep, syms) in ext_sym_totals {
            let mut v: Vec<(String, usize)> = syms.into_iter().collect();
            v.sort_by_key(|(_k, c)| std::cmp::Reverse(*c));
            v.truncate(self.top_n);
            out.push(Action::SetTopExternalSymbols { dep, top: v });
        }

        out.push(Action::SetPhase(Phase::TotalsBuilt));
        out
    }
}
