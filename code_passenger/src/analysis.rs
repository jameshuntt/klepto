use crate::engine::EngineOutput;

pub mod action;
pub mod checks;
pub mod lens;
pub mod passes;
pub mod produce;
pub mod reducer;
pub mod state;
pub mod store;

use store::Store;
use passes::{Pass, BuildFileViews, BuildTotals};

pub fn run(raw: &EngineOutput) -> state::AnalysisState {
    let mut store = Store::new();

    let passes: Vec<Box<dyn Pass>> = vec![
        Box::new(BuildFileViews),
        Box::new(BuildTotals::default()),
    ];

    for p in passes {
        let actions = p.actions(raw, store.state());
        store.dispatch_many(actions);
    }

    // rules (optional)
    let rules: Vec<Box<dyn checks::Rule>> = vec![];
    let check_actions = checks::run_rules(raw, store.state(), &rules);
    store.dispatch_many(check_actions);

    store.dispatch(action::Action::SetPhase(state::Phase::Done));
    store.into_state()
}
