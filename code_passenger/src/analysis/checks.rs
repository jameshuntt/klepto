use super::{
    action::Action,
    state::{AnalysisState, Finding},
};
use crate::engine::EngineOutput;

pub trait Rule: Send + Sync {
    fn id(&self) -> &'static str;
    fn findings(&self, raw: &EngineOutput, st: &AnalysisState) -> Vec<Finding>;
}

pub fn run_rules(raw: &EngineOutput, st: &AnalysisState, rules: &[Box<dyn Rule>]) -> Vec<Action> {
    let mut out = Vec::new();
    for r in rules {
        let fs = r.findings(raw, st);
        if !fs.is_empty() {
            out.push(Action::AddFindings(fs));
        }
    }
    out.push(Action::SetPhase(super::state::Phase::ChecksRun));
    out
}
