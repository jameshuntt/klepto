// Phase 6 fixture: receipt, KG export and the analysis frame. Parsed by
// klepto only, never compiled. See PHASE6_RULES.md and tests/phase6.rs.

pub mod store;
use std::collections::HashMap;                 // K: resolve_path challenge → std::collections::HashMap
pub use store::Engine as Motor;                // K: reexports edge

pub trait Runner {
    fn run(&self);
}

impl Runner for store::Engine {                // K: implements edge → item/app::Runner
    fn run(&self) {}
}

pub fn run() {
    let e = store::Engine::new();
    e.tick();                                  // K: receiver_type challenge → app::store::Engine; dependents_of tick → [app::run]
    let _m: HashMap<u8, u8> = HashMap::new();
}

#[cfg(feature = "x")]
pub fn only_x() { optx::go(); }                // gated correctly; K: gated_by edge → feat/app/x

pub fn calls_only_x() { only_x(); }            // A: KLEP011 cfg-edge violation (True ⇏ feature x)

pub fn oops() { optx::go(); }                  // A: KLEP010 ungated dep use of optx

pub fn far_away() { outside::far(); }          // K: references → item/outside::far (external), depends_on → crate/outside

fn never_called() {}                           // A: KLEP013 unreferenced private item

pub fn unknown() { (|| store::Engine::new())().frob(); }   // A: KLEP012 unresolved reference (untyped receiver)

// R9 amendment: a crate-root `extern crate` alias is visible in every module.
extern crate outside as far;
