// src/analysis/checks.rs
use crate::engine::EngineOutput;
use super::state::{AnalysisState, Finding};

pub trait Rule: Send + Sync {
    fn id(&self) -> &'static str;
    fn eval(&self, raw: &EngineOutput, st: &AnalysisState) -> Vec<Finding>;
}

pub fn run_rules(raw: &EngineOutput, st: &mut AnalysisState, rules: &[Box<dyn Rule>]) {
    for r in rules {
        st.findings.extend(r.eval(raw, st));
    }
}