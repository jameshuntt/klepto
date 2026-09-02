// M8 fixture: modules declared inside a macro invocation (tokio's `cfg_rt!`,
// hyper's `cfg_feature!`, `cfg_if!`). Since Phase 8 a local `macro_rules!`
// is expanded, so `cfg_rt!` yields real `#[cfg(feature = "rt")] mod` items;
// a macro klepto cannot expand (`cfg_ext!` below has no definition on disk)
// still declares its `mod name;` — with an Unknown cfg.

macro_rules! cfg_rt {
    ($($item:item)*) => { $( #[cfg(feature = "rt")] $item )* };
}

/// R12 amendment: a `macro_rules!` name is visible in every module of the
/// crate (textual scope), so `trace!(..)` in runtime.rs names this one.
macro_rules! trace {
    ($e:expr) => { $e };
}

cfg_rt! {
    pub mod runtime;
    pub mod io;
}

pub mod plain;

// M8 proper: no definition anywhere → not expanded → Unknown cfg.
cfg_ext! {
    pub mod late;
}

/// Uses items of macro-declared modules: `runtime` is really gated on `rt`
/// (a definite cfg-edge violation from this ungated fn), `late` is Unknown
/// (never a violation, never "always present" either).
pub fn start() -> u32 { runtime::spawn() + plain::one() + late::two() }
