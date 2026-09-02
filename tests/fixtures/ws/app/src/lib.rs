// Fixture source is only parsed by klepto, never compiled, so it may
// reference items that do not exist (e.g. a derive macro in mydep).
pub mod util;
pub mod pbkdf;

extern crate outside as out;

use mydep::Widget;
use outside::outside_fn as ofn;
use std::collections::HashMap;

#[derive(mydep::Derive)]
#[mydep(rename = "cfg")]
pub struct Config {
    map: HashMap<String, String>,
}

fn local_helper() -> u8 { 1 }

pub fn top() -> u8 { Some(local_helper()).unwrap_or(0) }

pub fn shouty() {
    println!("hi");
    panic!("boom");
}

pub fn uses_alias() -> Widget { Widget::new() }

pub fn uses_inside() {
    use mydep::helper;
    helper();
}

pub fn full_path() { mydep::helper(); }

pub fn uses_macro() { let _v = mydep::mk!(1); }

pub fn really_panics() -> u8 { Some(1).unwrap() }

pub fn outside_call() { ofn(); outside::outside_fn(); }

#[cfg(feature = "with_extra")]
pub fn gated() { extra::extra_fn(); }
