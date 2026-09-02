macro_rules! hidden {
    () => { 2 };
}

pub fn a() -> u32 { hidden!() }
