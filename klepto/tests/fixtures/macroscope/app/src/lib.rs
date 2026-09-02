//! R12 fixture: `macro_rules!` visibility follows the module tree and
//! `#[macro_use]`, in the resolver and in the expander alike.

#[macro_use]
mod util;

mod inner;

pub fn root() -> u32 { lifted!() + inner::a() }

/// `assert_eq!` here is std's: the one in `tests` below is not lifted.
pub fn assert_eq_user() -> u32 {
    assert_eq!(1, 1);
    0
}

#[cfg(test)]
mod tests {
    macro_rules! assert_eq {
        ($a:expr, $b:expr) => {};
    }

    pub fn t() {
        assert_eq!(1, 1);
    }
}
