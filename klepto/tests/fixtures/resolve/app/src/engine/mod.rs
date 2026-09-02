mod parts;                      // -> engine/parts.rs (rule M2, mod-rs style)

pub struct Engine;

impl Engine {
    pub fn start() { parts::part(); }
}

pub fn helper() {}              // same name as query::helper; distinct fq_name

impl std::fmt::Display for Engine {
    fn fmt(&self, _f: &mut std::fmt::Formatter) -> std::fmt::Result { Ok(()) }
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, _f: &mut std::fmt::Formatter) -> std::fmt::Result { Ok(()) }
}
