// N6 amendment fixture: twin definitions under complementary cfgs are
// always present, so ungated callers are not cfg-edge violations.

#[cfg(feature = "x")]
pub fn pos() -> u32 { 1 }

#[cfg(not(feature = "x"))]
pub fn pos() -> u32 { 0 }

/// Ungated caller of the twins: fine.
pub fn caller() -> u32 { pos() }

/// A real gated item, for contrast.
#[cfg(feature = "x")]
pub fn only_x() -> u32 { 2 }

/// A real violation: ungated caller of a gated item.
pub fn bad_caller() -> u32 { only_x() }

/// Twins that do NOT cover everything: still gated.
#[cfg(unix)]
pub fn partial() -> u32 { 3 }

#[cfg(windows)]
pub fn partial() -> u32 { 4 }

pub fn calls_partial() -> u32 { partial() }

/// The same at module level (serde_json's `ser`): one file declared twice
/// under complementary cfgs is always present — with either visibility.
#[cfg(feature = "x")]
pub mod twin;
#[cfg(not(feature = "x"))]
mod twin;

pub fn calls_twin() -> u32 { twin::t() }
