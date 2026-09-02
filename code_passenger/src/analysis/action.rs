use super::state::{FileAnalysis, Finding, Phase};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Action {
    SetPhase(Phase),
    UpsertFile {
        path: String,
        analysis: FileAnalysis,
    },
    AddFinding(Finding),
    AddFindings(Vec<Finding>),
    IncExternalDepHit {
        dep: String,
        by: usize,
    },
    IncInternalRootHit {
        root: String,
        by: usize,
    },
    SetTopExternalSymbols {
        dep: String,
        top: Vec<(String, usize)>,
    },
}
