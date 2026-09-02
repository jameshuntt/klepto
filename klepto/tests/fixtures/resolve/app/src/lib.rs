// Phase 1 fixture: module tree + alias resolution. Parsed by klepto only,
// never compiled. See PHASE1_RULES.md and tests/phase1.rs.

pub mod query;            // -> query.rs            (rule M2)
pub mod engine;           // -> engine/mod.rs       (rule M2)
#[path = "helpers_impl.rs"]
mod helpers;              // -> helpers_impl.rs     (rule M3)
mod nested {              // inline                 (rule M4)
    pub mod inner;        // -> nested/inner.rs     (rule M4)
    pub fn in_inline() {}
}

pub use query::Query;                // local re-export, uniform path   (rules S1, R7)
pub use regexish::Regex as Rx;       // external re-export with alias    (rules S1, R9)
use regexish::sub::*;                // external glob                    (rule S3)
use other::*;                        // second external glob -> ambiguity
use regexish::mk;                    // macro brought into scope         (rule R12)
use regexish::top_fn as top_fn2;     // single-segment alias             (rule O1)
use std::collections::HashMap;

#[derive(Clone, regexish::Derive)]   // attribute args: prelude single + external path (rule O4)
pub struct Cfg;

pub fn root_fn() -> HashMap<u8, u8> { HashMap::new() }

pub fn uses_reexport() { let _q = Query::new(); }          // -> app::query::Query::new
pub fn uses_alias() { let _r = Rx::new(); }                // -> regexish::Regex::new
pub fn uses_glob_external() { let _t = Thing::default(); } // ambiguous: regexish::sub::Thing | other::Thing
pub fn uses_crate_path() { crate::query::run(); }          // -> app::query::run
pub fn uses_helpers() { helpers::help(); }                 // -> app::helpers::help
pub fn uses_nested() { nested::inner::deep_fn(); nested::in_inline(); }
pub fn uses_macro() { let _a = regexish::mk!(1); let _b = mk!(2); println!("x"); }
pub fn uses_single() { top_fn2(); }                        // single segment via alias -> regexish::top_fn
