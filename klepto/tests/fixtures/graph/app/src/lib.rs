// Phase 3 fixture: item graph. Parsed by klepto only, never compiled.
// See PHASE3_RULES.md and tests/phase3.rs.

pub mod store;                         // N1 module node, D3 Contains app -> app::store
pub mod api;
pub use api::serve;                    // D2 ReExport app -> app::api::serve
use std::collections::HashMap;         // D2 Import to a true external (N3)

pub fn uses_std() -> HashMap<u8, u8> { HashMap::new() }   // N3 external nodes std::collections::HashMap[::new]

pub struct Config {                    // N1 Struct
    pub name: String,
    pub engine: store::Engine,         // D1 field type: app::Config -> app::store::Engine
}

impl Config {                          // N1 Impl `app::<impl Config>`
    pub fn engine(&self) -> &store::Engine { &self.engine }   // D1 return type
}

pub trait Runner {                     // N1 Trait; D3 Contains -> app::Runner::run
    fn run(&self);
}

impl Runner for Config {               // N1 Impl `app::<impl app::Runner for Config>`; D8 header refs
    fn run(&self) {
        self.engine.tick();            // D5 → Phase 5 G1: typed receiver, edge to Engine::tick
        store::Engine::new();          // D1 -> app::store::Engine::new
    }
}

pub fn top() { store::helper(); }      // D1 fn -> fn

#[cfg(feature = "y")]
pub fn gated_fn() { opt_y::y(); }      // gated item using the optional dep

pub fn uses_gated() { gated_fn(); }    // Q7 VIOLATION: True does not imply feature y

#[cfg(feature = "y")]
pub fn ok_caller() { gated_fn(); }     // gated caller: fine

pub fn sneaky() { opt_y::y(); }        // ungated use of opt_y (Phase 2) — Q6 uses_lost_dep

pub const LIMIT: u32 = store::MAX;     // N1 Const; D1 const -> const
pub static NAME: &str = "x";           // N1 Static
pub type Alias = store::Engine;        // N1 TypeAlias; D1 -> app::store::Engine

macro_rules! shout {                   // N1 Macro `app::shout`
    ($e:expr) => { $e };
}
pub fn uses_macro() { let _ = shout!(1); }   // D1 fn -> macro

pub fn uses_dep_x() { dep_x::x(); }    // D1 -> External dep_x::x

pub fn chain_a() { chain_b(); }        // Q3/Q5 transitive chain to dep_x::x2
fn chain_b() { chain_c(); }
fn chain_c() { dep_x::x2(); }

pub fn island() {}                     // no edges at all

pub fn recursive(n: u32) { if n > 0 { recursive(n - 1); } }   // D6 self-edge dropped
