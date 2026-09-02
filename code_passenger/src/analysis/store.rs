use super::{produce::produce, reducer::reduce_in_place, action::Action, state::AnalysisState};

#[derive(Debug, Default, Clone)]
pub struct Store {
    st: AnalysisState,
}

impl Store {
    pub fn new() -> Self { Self { st: AnalysisState::default() } }
    pub fn state(&self) -> &AnalysisState { &self.st }
    pub fn into_state(self) -> AnalysisState { self.st }

    pub fn dispatch(&mut self, a: Action) {
        self.dispatch_many(std::iter::once(a));
    }

    pub fn dispatch_many<I: IntoIterator<Item = Action>>(&mut self, actions: I) {
        let next = produce(&self.st, |draft| {
            for a in actions {
                reduce_in_place(draft, a);
            }
        });
        self.st = next;
    }
}