mod deep;                       // -> query/deep.rs (rule M2, non-mod-rs style)

use regexish::Regex;
use super::engine::Engine;      // super:: in a use target (rule R3)

pub struct Query;

impl Query {
    pub fn new() -> Self { Self }
    pub fn run_with(&self) {
        Regex::new();           // alias -> regexish::Regex::new
        Engine::start();        // alias -> app::engine::Engine::start
        deep::deep_run();       // child module -> app::query::deep::deep_run
        self::run();            // self:: -> app::query::run
    }
}

pub fn run() {
    let _r = Regex::new();      // THE key case: alias use inside a fn body
}

fn helper() {}                  // same name as engine::helper; must not collide
