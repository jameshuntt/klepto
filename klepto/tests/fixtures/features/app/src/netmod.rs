// Whole file is under `#[cfg(feature = "net")]` (declared in lib.rs), so
// every use of opt_a / opt_b here is gated (net enables both).
pub fn n() {
    opt_b::b();
    opt_a::a();
}

#[cfg(feature = "fast")]
pub fn n_fast() {}                                // effective cfg: all(net, fast)
