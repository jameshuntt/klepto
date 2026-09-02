pub struct BuildFileViews;
use crate::analysis::{
    action::Action,
    passes::Pass,
    state::{AnalysisState, FileAnalysis, Phase},
};
use im::{OrdMap, OrdSet};

impl Pass for BuildFileViews {
    fn id(&self) -> &'static str {
        "build_file_views"
    }

    fn actions(&self, raw: &crate::engine::EngineOutput, _st: &AnalysisState) -> Vec<Action> {
        let mut out = Vec::new();
        for r in &raw.reports {
            out.push(Action::UpsertFile {
                path: r.document.relative_path.clone(),
                analysis: file_view(r),
            });
        }
        out.push(Action::SetPhase(Phase::FileViewsBuilt));
        out
    }
}

fn file_view(r: &crate::model::FileReport) -> FileAnalysis {
    let used_external: OrdSet<String> = r.used.packages.iter().cloned().collect();

    let used_internal_roots: OrdSet<String> = r
        .used
        .modules
        .iter()
        .map(|p| p.split("::").next().unwrap_or(p).to_string())
        .collect();

    // BTreeMap<String, BTreeMap<String, usize>>  ->  OrdMap<String, OrdMap<String, usize>>
    let external_symbol_counts: OrdMap<String, OrdMap<String, usize>> = r
        .external_dep_symbol_counts
        .clone()
        .into_iter()
        .map(|(dep, syms)| {
            let inner: OrdMap<String, usize> = syms.into_iter().collect::<OrdMap<String, usize>>();
            (dep, inner)
        })
        .collect::<OrdMap<String, OrdMap<String, usize>>>();

    let internal_symbol_counts: OrdMap<String, OrdMap<String, usize>> = r
        .internal_dep_symbol_counts
        .clone()
        .into_iter()
        .map(|(root, syms)| {
            let inner: OrdMap<String, usize> = syms.into_iter().collect::<OrdMap<String, usize>>();
            (root, inner)
        })
        .collect::<OrdMap<String, OrdMap<String, usize>>>();

    FileAnalysis {
        used_external,
        used_internal_roots,
        external_symbol_counts,
        internal_symbol_counts,
    }
}
