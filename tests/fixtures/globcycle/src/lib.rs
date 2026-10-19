//! Four glob imports whose roots nothing in scope binds: each one's path can
//! only come through the others. Before the per-module glob memo this took
//! 4^32 resolutions.
mod m {
    use alpha::*;
    use beta::*;
    use gamma::*;
    use delta::*;
    pub fn f() -> Zed {
        todo!()
    }
}
