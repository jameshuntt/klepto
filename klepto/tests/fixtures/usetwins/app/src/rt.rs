//! Declared twice in lib.rs (`logging` / `not(logging)`): present in every
//! build, and so is everything below it.
pub mod park {
    pub fn new() -> u32 { 4 }
}
