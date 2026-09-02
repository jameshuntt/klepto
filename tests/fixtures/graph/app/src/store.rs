pub const MAX: u32 = 10;

pub struct Engine;

impl Engine {                          // `app::store::<impl Engine>`
    pub fn new() -> Engine { Engine }
    pub fn tick(&self) {}
}

pub fn helper() { let _e = Engine::new(); }   // D1 -> app::store::Engine::new

#[cfg(feature = "y")]
pub fn store_gated() { opt_y::y(); }

pub fn store_uses_gated() { store_gated(); }   // Q7 VIOLATION in another module
