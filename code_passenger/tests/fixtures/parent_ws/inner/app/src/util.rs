/// Two identical helpers: the `patterns` command reports one candidate.
pub fn twice(x: u32) -> u32 { let mut acc = 0; for _ in 0..2 { acc += x; } acc }
pub fn double(x: u32) -> u32 { let mut acc = 0; for _ in 0..2 { acc += x; } acc }

/// Private and never referenced: KLEP013 for the `lint` command.
fn unused_helper() -> u32 { 7 }
