//! R14 fixture: bindings that are twins under complementary cfgs.

pub mod real {
    pub fn handle() -> u32 { 1 }
}

#[cfg(test)]
pub mod mocks {
    pub fn handle() -> u32 { 2 }
}

// tokio's `fs/file.rs`: the same name imported from two places
#[cfg(test)]
use mocks::handle;
#[cfg(not(test))]
use real::handle;

/// Resolves to the binding of the default build; together the twins cover
/// every build, so this reference is never a cfg-edge violation.
pub fn go() -> u32 { handle() }

pub mod real_log {
    pub fn trace() {}
}

// rustls's `lib.rs`: a `use` under the feature, a module without it
#[cfg(feature = "logging")]
use crate::real_log as log;
#[cfg(not(feature = "logging"))]
mod log {
    pub fn trace() {}
}

pub fn run() { log::trace() }

/// Under `test` this reference means the mock: a reference inside a test
/// module resolves to the twin its own cfg implies.
#[cfg(test)]
mod tests {
    pub fn t() -> u32 { super::handle() }
}

/// Q10: `wrap_help = ["help"]`, so `wrap_help` code may use `help` code —
/// decided by enumerating the features the two cfgs mention, however many
/// the package declares.
#[cfg(all(feature = "help", feature = "wrap_help"))]
pub fn wrapper() {}

#[cfg(feature = "wrap_help")]
pub fn wrap() { wrapper() }

// tokio's `runtime`: one file declared under complementary cfgs (M6); its
// children are present exactly when it is
#[cfg(feature = "logging")]
pub mod rt;
#[cfg(not(feature = "logging"))]
pub(crate) mod rt;

pub fn use_park() -> u32 { rt::park::new() }
