// Stand-in for an external dependency (think `regex`).
pub struct Regex;

impl Regex {
    pub fn new() -> Self { Regex }
}

pub mod sub {
    pub struct Thing;
}

pub fn top_fn() {}

#[macro_export]
macro_rules! mk {
    ($e:expr) => { $e };
}
