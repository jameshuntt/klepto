use im::{OrdMap, OrdSet};
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnalysisState {
    pub phase: Phase,

    // per-file derived view (key = relative path)
    pub files: OrdMap<String, FileAnalysis>,

    // crate-level rollups
    pub crate_totals: CrateTotals,

    // findings from checks/passes
    pub findings: Vec<Finding>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileAnalysis {
    pub used_external: OrdSet<String>,
    pub used_internal_roots: OrdSet<String>,     // crate/self/super/code_passenger
    pub external_symbol_counts: OrdMap<String, OrdMap<String, usize>>,
    pub internal_symbol_counts: OrdMap<String, OrdMap<String, usize>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CrateTotals {
    pub external_dep_hits: OrdMap<String, usize>, // dep -> total hits
    pub internal_root_hits: OrdMap<String, usize>,// root -> total hits
    pub external_head_hits: OrdMap<String, usize>, // dep -> total hits
    pub internal_head_hits: OrdMap<String, usize>,// root -> total hits
    pub top_external_symbols: OrdMap<String, Vec<(String, usize)>>, // dep -> [(sym,count)]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub severity: Severity,
    pub file: Option<String>,
    pub code: String,
    pub message: String,
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Severity { Info, Warn, Error }

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum Phase {
    #[default]
    Init,
    FileViewsBuilt,
    TotalsBuilt,
    ChecksRun,
    Done,
}