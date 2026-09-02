use super::{action::Action, state::AnalysisState};

pub fn reduce_in_place(st: &mut AnalysisState, a: Action) {
    match a {
        Action::SetPhase(p) => st.phase = p,

        Action::UpsertFile { path, analysis } => {
            st.files.insert(path, analysis);
        }

        Action::AddFinding(f) => st.findings.push(f),
        Action::AddFindings(mut fs) => st.findings.append(&mut fs),

        Action::IncExternalDepHit { dep, by } => {
            let cur = st.crate_totals.external_dep_hits.get(&dep).copied().unwrap_or(0);
            st.crate_totals.external_dep_hits.insert(dep, cur + by);
        }

        Action::IncInternalRootHit { root, by } => {
            let cur = st.crate_totals.internal_root_hits.get(&root).copied().unwrap_or(0);
            st.crate_totals.internal_root_hits.insert(root, cur + by);
        }

        Action::SetTopExternalSymbols { dep, top } => {
            st.crate_totals.top_external_symbols.insert(dep, top);
        }
    }
}
