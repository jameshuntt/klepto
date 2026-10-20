#[cfg_attr(feature = "x", derive(helper::Thing))]
pub struct Kept;

#[cfg(feature = "x")]
pub fn gone() -> u8 {
    helper::VALUE
}

pub fn uses() -> u8 {
    helper::VALUE
}
