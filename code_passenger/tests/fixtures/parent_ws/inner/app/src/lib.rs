// code_passenger gate fixture. This workspace COMPILES (the oracle runs
// real `cargo check` on a scratch copy). See GATE_RULES.md, tests/gate.rs.

pub mod util;

/// Ungated use of `beta`: the plan gates this item with `feature = "b"`.
pub fn uses_beta() -> u32 { beta::beep() }

/// Already under a cfg: the gate must MERGE, not stack a second attribute.
#[cfg(unix)]
pub fn unix_beta() -> u32 { beta::beep() + 1 }

/// Calls a gated fn: the cascade gates this too.
pub fn calls_uses_beta() -> u32 { uses_beta() * 2 }

/// Nothing to do with beta: stays as it is.
pub fn plain() -> u32 { util::twice(3) }
