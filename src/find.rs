use crate::model::*;
use crate::klepto::Klepto;

pub struct Finder<'k> {
    k: &'k Klepto,
}
impl<'k> Finder<'k> {
    pub fn new(k: &'k Klepto) -> Self { Self { k } }

    pub fn paths_eq(&self, p: &str) -> Vec<PathOccurrence> { self.k.find_paths(p) }
    pub fn macros(&self, name: &str) -> Vec<MacroInvocation> { self.k.find_macro_invocations(name) }
    pub fn calls_containing(&self, s: &str) -> Vec<CallOccurrence> { self.k.find_calls(s) }

    pub fn unwrap_calls(&self) -> Vec<CallOccurrence> {
        self.k.calls.iter().cloned().filter(|c| c.callee == "unwrap" || c.callee.contains(".unwrap")).collect()
    }

    pub fn expect_calls(&self) -> Vec<CallOccurrence> {
        self.k.calls.iter().cloned().filter(|c| c.callee == "expect" || c.callee.contains(".expect")).collect()
    }
}
