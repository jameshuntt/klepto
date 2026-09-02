//! Phase 8 typing fixture (rules T1–T6): receivers typed through std
//! shapes, patterns, closures, `?`, indexing and trait objects.
use std::cell::RefCell;
use std::collections::HashMap;

pub struct Item {
    pub name: String,
    pub weight: u32,
}

impl Item {
    pub fn new(name: &str) -> Item { Item { name: name.to_string(), weight: 1 } }
    pub fn label(&self) -> String { self.name.clone() }
    pub fn weight(&self) -> u32 { self.weight }
}

pub trait Describe {
    fn describe(&self) -> String;
}

impl Describe for Item {
    fn describe(&self) -> String { self.label() }
}

pub struct Store {
    items: Vec<Item>,
    by_name: HashMap<String, Item>,
    cache: RefCell<Vec<Item>>,
}

impl Store {
    pub fn items(&self) -> &[Item] { &self.items }
    pub fn find(&self, name: &str) -> Option<&Item> { self.by_name.get(name) }
    pub fn first(&self) -> Result<&Item, String> { self.items.first().ok_or_else(|| "empty".to_string()) }

    /// T4: the closure parameter is an item of the receiver.
    pub fn heaviest(&self) -> Option<&Item> { self.items.iter().max_by_key(|i| i.weight()) }
    pub fn labels(&self) -> Vec<String> { self.items.iter().map(|i| i.label()).collect() }

    /// T3: `for` binds the item.
    pub fn total(&self) -> u32 {
        let mut t = 0;
        for it in &self.items {
            t += it.weight();
        }
        t
    }

    /// T3: `if let` and `match` patterns.
    pub fn named(&self, n: &str) -> u32 {
        if let Some(it) = self.find(n) {
            return it.weight();
        }
        match self.first() {
            Ok(it) => it.weight(),
            Err(e) => e.len() as u32,
        }
    }

    /// T2: `?` unwraps.
    pub fn try_first(&self) -> Result<u32, String> {
        let it = self.first()?;
        Ok(it.weight())
    }

    /// T2: indexing a `Vec`.
    pub fn nth(&self, i: usize) -> String { self.items[i].label() }

    /// T3: tuple fields.
    pub fn tuple(&self) -> u32 {
        let pair = (Item::new("a"), 2u32);
        pair.0.weight() + pair.1
    }

    /// T2: `RefCell::borrow` is transparent, `Vec::len` is std.
    pub fn cached(&self) -> usize { self.cache.borrow().len() }

    /// T2: the map entry API.
    pub fn lookup_default(&mut self, n: &str) -> u32 { self.by_name.entry(n.to_string()).or_insert(Item::new(n)).weight() }

    /// T2: a scanned return type `Vec<String>` flows into the iterator.
    pub fn mapped(&self) -> Vec<u32> { self.labels().iter().map(|l| l.len() as u32).collect() }

    /// T1: `dyn Trait` / `impl Trait` receivers use the trait's methods.
    pub fn via_dyn(&self, it: &dyn Describe) -> String { it.describe() }
    pub fn via_impl(it: impl Describe) -> String { it.describe() }

    /// T5: a closure's result types the next adaptor.
    pub fn first_label(&self) -> Option<String> { self.items().first().map(|i| i.label()) }
    pub fn all_labels_len(&self) -> usize { self.items().iter().map(|i| i.label()).map(|l| l.len()).sum() }

    /// T6 (limit): `.into()` and a scanned type's `IntoIterator` stay untyped.
    pub fn untyped(&self, x: u8) -> u32 {
        let y: Item = Item::new("b").into();
        y.weight() + x as u32
    }
}

/// O1 amendment: a local binding named like a module is a value, never a
/// reference to the module.
pub mod meta {
    pub fn x() -> u32 { 7 }
}

pub fn shadow(meta: u32) -> u32 {
    let items = vec![meta];
    let total: u32 = items.iter().map(|meta| meta + 1).sum();
    match Some(total) {
        Some(meta) => meta,
        None => 0,
    }
}

pub fn uses_meta() -> u32 { meta::x() }

/// R15: `fn Token` (value namespace) beside `use types::Token` (type
/// namespace) — a type position means the type, a call means the fn.
pub mod types {
    pub struct Token {
        pub id: u32,
    }
}

use types::Token;

#[allow(non_snake_case)]
pub fn Token() -> Token { Token { id: 1 } }

pub fn take(t: Token) -> u32 { t.id }

pub fn make() -> u32 { Token().id }

/// O1: an upper-case pattern identifier is a const, still a reference.
pub const LIMIT: u32 = 3;

pub fn limit(total: u32) -> u32 {
    match total {
        crate::LIMIT => 1,
        _ => 0,
    }
}

/// R16: a path through a module follows that module's `use` re-export in
/// the namespace of the position (syn's `crate::ident::Ident`).
pub mod ident {
    pub use crate::types::Token as Marker;

    #[allow(non_snake_case)]
    pub fn Marker() -> u32 { 0 }
}

pub fn by_path(m: crate::ident::Marker) -> u32 { m.id + ident::Marker() }
