// code_passenger gate fixture 2: `use` gates, impl-method gates, multi-line
// attributes. This workspace COMPILES. See GATE_RULES.md, tests/gate.rs.

use beta::beep;

/// Uses the imported name: the `use` line above gets a gate too.
pub fn via_import() -> u32 { beep() }

#[cfg_attr(
    test,
    allow(dead_code)
)]
/// A struct holding a `beta` type behind a multi-line attribute: the gate
/// goes after the whole attribute, right above `pub struct`.
pub struct Holder {
    pub thing: beta::Thing,
}

pub struct Engine;

impl Engine {
    /// Indented method using beta: the gate keeps the indentation.
    pub fn tick(&self) -> u32 {
        beta::beep() + 1
    }

    pub fn plain(&self) -> u32 { 7 }
}
