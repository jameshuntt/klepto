use crate::store::Engine;              // D2 Import app::api -> app::store::Engine
use dep_x::*;                          // external glob

pub fn serve() {
    let _ = Engine::new();             // D1 via alias -> app::store::Engine::new
    super::top();                      // D1 -> app::top
}

pub fn via_glob() { let _ = Gx::new(); }   // D5 unresolved (glob candidate dep_x::Gx::new), no edge
