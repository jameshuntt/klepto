//! Phase 8 fixture: `macro_rules!` expansion (rules X1–X8).

/// X1/X5: impl items from a local macro, with a repetition.
macro_rules! make_getters {
    ($($name:ident : $ty:ty),* $(,)?) => {
        $( pub fn $name(&self) -> $ty { self.$name.clone() } )*
    };
}

pub struct Config {
    host: String,
    port: u16,
}

impl Config {
    make_getters!(host: String, port: u16);
}

/// X1: a struct and an impl from one invocation; `$vis` and `$crate`.
macro_rules! newtype {
    ($vis:vis $name:ident($inner:ty)) => {
        $vis struct $name(pub $inner);
        impl $name {
            pub fn get(&self) -> &$inner { &self.0 }
            pub fn config() -> $crate::Config { $crate::Config { host: String::new(), port: 0 } }
        }
    };
}

newtype!(pub Id(u64));

/// X4: `cfg_if!` is built in; every branch gets its exact predicate.
cfg_if::cfg_if! {
    if #[cfg(feature = "fast")] {
        pub mod fast;
        pub fn pick() -> u32 { fast::go() }
    } else {
        pub fn pick() -> u32 { 0 }
    }
}

/// X1: expression position.
macro_rules! call_it {
    ($e:expr) => { $e.host() };
}

pub fn uses() -> usize {
    let c = Config { host: String::new(), port: 1 };
    let h = call_it!(c);
    h.len() + Id(1).get().count_ones() as usize
}

/// X2: a dependency's `#[macro_export]` macro, harvested from the registry.
lazy_static::lazy_static! {
    static ref TABLE: Vec<u32> = vec![1, 2];
}

pub fn table_len() -> usize { TABLE.len() }

/// X8: no definition on disk → left in place, recorded as ProcMacroOrUnknown.
mystery_macro! { pub fn never() {} }

/// X3: a rule that does not match → Unmatched, left in place.
newtype!(nope);

/// X5 amendment: twin definitions under complementary cfgs — what the one
/// klepto takes produces exists only under that definition's cfg.
#[cfg(feature = "fast")]
macro_rules! only_fast {
    ($e:expr) => { $e };
}
#[cfg(not(feature = "fast"))]
macro_rules! only_fast {
    ($e:expr) => { 0 };
}

pub fn gated() -> u32 { only_fast!(fast::go()) }
