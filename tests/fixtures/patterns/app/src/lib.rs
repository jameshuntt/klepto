// Phase 7 fixture: pattern mining. Parsed by klepto only, never compiled.
// See PHASE7_RULES.md and tests/phase7.rs. Line numbers matter for the
// metrics test (`complex` spans lines 51-64).

pub struct Engine { pub name: String, pub id: u32 }
pub struct Motor { pub name: String, pub id: u32 }
pub struct Pump { pub name: String }

impl Engine {
    pub fn new(name: String, id: u32) -> Engine { Engine { name, id } }
    pub fn ready(&self) -> bool { self.id > 0 }
    pub fn describe(&self) -> String { format!("{}:{}", self.name, self.id) }   // E: same-named method → Trait candidate
    pub fn stop(&self) {}
}

impl Motor {
    pub fn new(name: String, id: u32) -> Motor { Motor { name, id } }
    pub fn ready(&self) -> bool { self.id > 0 }
    pub fn describe(&self) -> String { format!("{}:{}", self.name, self.id) }
    pub fn stop(&self) {}
}

impl Pump {
    pub fn new(name: String) -> Pump { Pump { name } }
    pub fn ready(&self) -> bool { !self.name.is_empty() }
}

pub trait Named { fn name(&self) -> &str; }
impl Named for Engine { fn name(&self) -> &str { &self.name } }    // H: repeated trait impl
impl Named for Motor { fn name(&self) -> &str { &self.name } }

// A: exact duplicates (level exact)
pub fn sum_a(xs: &[u32]) -> u32 { let mut t = 0; for x in xs { t += x; } t }
pub fn sum_b(xs: &[u32]) -> u32 { let mut t = 0; for x in xs { t += x; } t }
// B: renamed locals and parameters (level renamed)
pub fn sum_c(items: &[u32]) -> u32 { let mut total = 0; for it in items { total += it; } total }
// C: different literals (level literals)
pub fn scale_a(xs: &mut [u32]) { for x in xs.iter_mut() { *x *= 2; *x += 1; } }
pub fn scale_b(xs: &mut [u32]) { for x in xs.iter_mut() { *x *= 3; *x += 7; } }
// D: different types (level types); Phase 5 types the receivers, so the
//    type slot's required methods are known
pub fn check_engine(e: &Engine) -> bool { let ok = e.ready(); ok && e.id > 0 }
pub fn check_motor(m: &Motor) -> bool { let ok = m.ready(); ok && m.id > 0 }
// F: different callees (level callees)
pub fn apply_a(xs: &[u32]) -> Vec<u32> { xs.iter().map(|x| double(*x)).collect() }
pub fn apply_b(xs: &[u32]) -> Vec<u32> { xs.iter().map(|x| triple(*x)).collect() }
// tiny bodies: below the default size threshold
fn double(x: u32) -> u32 { x * 2 }
fn triple(x: u32) -> u32 { x * 3 }
// I: metrics
pub fn complex(n: u32) -> u32 {
    let mut acc = 0;
    for i in 0..n {
        if i % 2 == 0 && i > 3 {
            acc += i;
        } else {
            match i {
                1 => acc += 1,
                _ => {}
            }
        }
    }
    acc
}
