// Phase 4 fixture: gate planning. Parsed by klepto only, never compiled.
// See PHASE4_RULES.md and tests/phase4.rs. Line numbers are referenced by
// tests/fixtures/oracle/*.jsonl — keep them stable.

pub mod inner;

use beta::early;                         // P4 Use gate: module-level ungated use of beta

pub fn open() { alpha::run(); }          // P4: ungated alpha; public → reaches_public
pub fn calls_open() { open(); }          // P5 cascade from open
pub fn calls_calls_open() { calls_open(); }   // P5 cascade, second level

#[cfg(feature = "a")]
pub fn fine() { alpha::run(); }          // correctly gated: nothing to do

pub fn ok_dev() {}                       // no dep use; the recorded oracle blames this line (L rules)

pub fn both() { alpha::run(); devd::d(); }   // P7: two requirements → all(feature = "a", test)

#[cfg(version("1.70"))]
pub fn weird() { alpha::run(); }         // P10: Unknown cfg → plan.unknown

pub struct Holder { pub v: beta::Thing } // P4 on a struct; P5 cascades to its users and its impl
pub fn uses_holder(h: Holder) -> Holder { h }
impl Holder {
    pub fn get(&self) -> &beta::Thing { &self.v }   // carrier = the method
}

#[cfg(feature = "a")]
pub fn gated_target() {}
pub fn calls_gated() { gated_target(); } // P6 existing violation → gate

fn private_user() { alpha::run(); }      // gated, but private: not in reaches_public
