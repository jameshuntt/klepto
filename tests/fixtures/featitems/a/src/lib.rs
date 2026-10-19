pub fn a_always() {}

#[cfg(not(feature = "x"))]
pub fn a_without_x() {}

#[cfg(feature = "d")]
pub fn d_fn() {}

#[cfg(feature = "x")]
pub fn x_fn() {}

#[cfg(feature = "x")]
pub mod x_mod {
    pub struct XThing;
}
