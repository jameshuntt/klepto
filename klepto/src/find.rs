use crate::klepto::Klepto;
use crate::model::*;

/// Last segment of a callee as recorded by the occurrence visitor
/// (`"unwrap"`, `"Option :: unwrap"`, `"foo :: bar"`).
pub fn callee_name(callee: &str) -> &str {
    callee.rsplit("::").next().unwrap_or(callee).trim()
}

/// `unwrap` / `expect` and their `_err` variants. `unwrap_or*` never panic
/// and are deliberately not included.
pub fn is_panicking_callee(callee: &str) -> bool {
    matches!(callee_name(callee), "unwrap" | "expect" | "unwrap_err" | "expect_err")
}

pub struct Finder<'k> {
    k: &'k Klepto,
}
impl<'k> Finder<'k> {
    pub fn new(k: &'k Klepto) -> Self { Self { k } }

    pub fn paths_eq(&self, p: &str) -> Vec<PathOccurrence> { self.k.find_paths(p) }
    pub fn macros(&self, name: &str) -> Vec<MacroInvocation> { self.k.find_macro_invocations(name) }
    pub fn calls_containing(&self, s: &str) -> Vec<CallOccurrence> { self.k.find_calls(s) }

    pub fn unwrap_calls(&self) -> Vec<CallOccurrence> {
        self.k.calls.iter().filter(|c| callee_name(&c.callee) == "unwrap").cloned().collect()
    }

    pub fn expect_calls(&self) -> Vec<CallOccurrence> {
        self.k.calls.iter().filter(|c| callee_name(&c.callee) == "expect").cloned().collect()
    }
}
