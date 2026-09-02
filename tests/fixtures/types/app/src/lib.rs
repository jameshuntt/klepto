// Phase 5 fixture: type records + method-call resolution. Parsed by klepto
// only, never compiled. See PHASE5_RULES.md and tests/phase5.rs.

pub mod store;
use std::collections::HashMap;

#[derive(Debug, Clone, ext::Tag)]                       // T6 derives: std::Debug, std::Clone, ext::Tag
pub struct Config<'a, T: Runner + Default, const N: usize> {   // T3 generics: lifetime, bounded type, const
    pub name: &'a str,                                  // T2 field, no mentions (primitive)
    pub engine: store::Engine,                          // T2 field → Local
    map: HashMap<String, Vec<T>>,                       // private; mentions incl. a generic param
    #[cfg(feature = "x")]
    pub extra: Option<ext::Ext>,                        // field cfg
}

pub struct Pair(pub u8, store::Engine);                 // T2 tuple fields

pub enum Mode {                                         // T4 variants
    Off = 0,
    On(u8),
    Custom { level: u8, engine: store::Engine },
}

impl Mode {
    pub fn describe(&self) {}
}

pub union Raw {                                         // T2 union
    pub bits: u32,
    pub float: f32,
}

pub trait Runner: std::fmt::Debug + Sized {             // T5 supertraits, associated items
    type Output;
    const LIMIT: u32;
    fn run(&self) -> Self::Output;                      // no default body
    fn stop(&self) {}                                   // default body
}

pub type EngineMap = HashMap<String, store::Engine>;    // T7 alias target

impl<'a, T: Runner + Default, const N: usize> Config<'a, T, N> {   // T8 inherent impl
    pub fn new(engine: store::Engine) -> Self { unimplemented!() }
    pub fn engine(&self) -> &store::Engine { &self.engine }
    pub fn tick(&self) { self.engine.tick(); }          // M1 self → M4 field → M2 method
    pub fn chained(&self) { self.engine().tick(); }     // M3 one level of chaining
}

impl<'a, T: Runner + Default, const N: usize> Runner for Config<'a, T, N> {   // T8 trait impl
    type Output = u8;
    const LIMIT: u32 = 3;
    fn run(&self) -> u8 { 0 }
}

pub fn typed_param(e: &store::Engine) { e.tick(); }     // M1 typed parameter (through &)
pub fn let_annot() { let e: store::Engine = store::Engine::new(); e.tick(); }   // M1 annotated let
pub fn let_ctor() { let e = store::Engine::new(); e.tick(); }      // M1 constructor returning Engine
pub fn let_literal() { let p = Pair(1, store::Engine::new()); p.1.tick(); }   // M1 tuple-struct literal, M4 tuple field
pub fn let_variant() { let m = Mode::On(1); m.describe(); }        // M1 enum variant constructor
pub fn let_free_fn() { let e = make(); e.tick(); }      // M1 free fn with a written return type
pub fn generic_bound<R: Runner>(r: R) { r.run(); r.stop(); }       // M6 generic receiver → trait declaration
pub fn external_recv(x: ext::Ext) { x.go(); }           // M2 external receiver
pub fn prelude_recv(v: Vec<u8>) { v.len(); }            // M2 prelude type receiver
pub fn really_unknown() { let z = (|| store::Engine::new())(); z.frob(); }   // M7 unknown: stays unresolved

fn make() -> store::Engine { store::Engine::new() }
