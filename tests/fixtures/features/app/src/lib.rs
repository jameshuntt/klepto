// Phase 2 fixture: cfg predicates + Cargo feature model. Parsed by klepto
// only, never compiled. See PHASE2_RULES.md and tests/phase2.rs.

pub mod always;                                   // cfg: True

#[cfg(feature = "net")]
pub mod netmod;                                   // E2: gates netmod.rs entirely

#[cfg(feature = "net")]
use opt_b::b as bb;                               // E1 on a `use` item

#[cfg(all(feature = "net", not(feature = "fast")))]
pub fn net_slow() { opt_b::b(); }                 // gated correctly: net ⇒ opt_b available

pub fn oops() { opt_a::a(); }                     // UNGATED: opt_a is optional, nothing gates this

#[cfg(feature = "fast")]
pub fn fast_only() { opt_b::b(); }                // gated by `fast` (a default feature), but opt_b needs `net` → UNGATED

#[cfg(unix)]
pub fn unix_fn() { unixonly::u(); }               // target-gated dep, gated correctly

#[cfg(target_os = "linux")]
pub fn linux_fn() {}                              // C2 key = value

pub fn uses_dev() { devonly::d(); }               // dev-dependency used in lib code → UNGATED (needs cfg(test))

#[cfg(test)]
mod tests {
    pub fn t() { devonly::d(); }                  // fine: cfg(test) ⇒ dev-deps available
}

#[cfg_attr(feature = "net", derive(opt_b::Derive))]
pub struct Cfgd;                                  // E5: the derive exists only under `net`

pub fn stmt_level() {
    #[cfg(feature = "net")]
    opt_b::b();                                   // E3: statement-level cfg gates this call
    let _x = cfg!(feature = "net");               // E6: `cfg!` is an expression, not a gate
}

pub fn core_use() { core_dep::c(); }              // non-optional dep: always available

impl Cfgd {
    #[cfg(feature = "net")]
    pub fn net_method(&self) { opt_b::b(); }      // E1 on an impl item
}

#[cfg(feature = "net")]
impl Cfgd {
    pub fn in_gated_impl(&self) { opt_b::b(); }   // E1 inherited from the impl block
}

#[cfg(any(feature = "net", feature = "full"))]
pub fn either() { opt_b::b(); }                   // any(): still implies opt_b (full ⇒ net)

#[cfg(version("1.70"))]
pub fn weird() {}                                 // C5: not cfg grammar → Unknown, never assumed absent
