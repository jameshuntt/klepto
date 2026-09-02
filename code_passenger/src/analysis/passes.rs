use super::{action::Action, state::AnalysisState};
use crate::engine::EngineOutput;

pub trait Pass: Send + Sync {
    fn id(&self) -> &'static str;
    fn actions(&self, raw: &EngineOutput, st: &AnalysisState) -> Vec<Action>;
}

pub mod build_file_view;
pub use build_file_view::BuildFileViews;

pub mod build_totals;
pub use build_totals::BuildTotals;

pub mod prelude {
    pub use super::BuildFileViews;
    pub use super::BuildTotals;
}
