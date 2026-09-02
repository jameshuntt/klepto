pub struct Engine;

impl Engine {
    pub fn new() -> Engine { Engine }
    pub fn tick(&self) {}
}

/// Uses the crate-root alias from another module (R9 amendment).
pub fn from_store() -> u32 { far::far() }
