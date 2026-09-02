use regex::Regex;
use std::{
    collections::HashMap,
    hash::{Hash},
    sync::{Arc, Mutex, OnceLock},
};

/// One canonical place to name + describe all regex patterns your engine uses.
///
/// All patterns MUST capture:
/// - `dep`  : the root identifier (dep name OR internal root)
/// - `tail` : the rest of the path segments (1+ segments separated by ::)
/// - `bang` : optional `!` for macro calls (named capture, presence = macro-ish)
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum UsePatternKey {
    /// Matches `dep::a::b` and `::dep::a::b`
    DepPath,

    /// Matches `crate::...`, `self::...`, `super::...`, and `<crate_ident>::...`
    /// where `<crate_ident>` is the Rust-ident form of the crate name (hyphens -> underscores).
    InternalRoot { crate_ident: String },
}

#[derive(Debug, Clone)]
pub struct PatternInfo {
    pub name: &'static str,
    pub purpose: &'static str,
    pub dep_capture: &'static str,
    pub tail_capture: &'static str,
    pub bang_capture: &'static str,
}

impl UsePatternKey {
    pub fn info(&self) -> PatternInfo {
        match self {
            UsePatternKey::DepPath => PatternInfo {
                name: "dep_path",
                purpose: "Find external dependency paths like `dep::x::y` (and `::dep::x::y`).",
                dep_capture: "dep",
                tail_capture: "tail",
                bang_capture: "bang",
            },
            UsePatternKey::InternalRoot { .. } => PatternInfo {
                name: "internal_root",
                purpose: "Find internal roots like `crate::..`, `self::..`, `super::..`, `<crate>::..`.",
                dep_capture: "dep",
                tail_capture: "tail",
                bang_capture: "bang",
            },
        }
    }

    fn compile(&self) -> Regex {
        match self {
            UsePatternKey::DepPath => Regex::new(
                r#"(?x)
                (?P<global>::\s*)?
                (?P<dep>[A-Za-z_][A-Za-z0-9_]*)
                \s*::\s*
                (?P<tail>[A-Za-z_][A-Za-z0-9_]*(?:\s*::\s*[A-Za-z_][A-Za-z0-9_]*)*)
                (?P<bang>\s*!)?
                "#
            ).expect("valid dep_path regex"),

            UsePatternKey::InternalRoot { crate_ident } => Regex::new(&format!(
                r#"(?x)
                (?P<global>::\s*)?
                (?P<dep>\bcrate\b|\bself\b|\bsuper\b|\b{}\b)
                \s*::\s*
                (?P<tail>[A-Za-z_][A-Za-z0-9_]*(?:\s*::\s*[A-Za-z_][A-Za-z0-9_]*)*)
                (?P<bang>\s*!)?
                "#,
                regex::escape(crate_ident),
            )).expect("valid internal_root regex"),
        }
    }
}

fn crate_ident(crate_name: &str) -> String {
    crate_name.replace('-', "_")
}

#[derive(Default)]
pub struct UsePatternRegistry {
    cache: Mutex<HashMap<UsePatternKey, Arc<Regex>>>,
}

impl UsePatternRegistry {
    pub fn global() -> &'static UsePatternRegistry {
        static REG: OnceLock<UsePatternRegistry> = OnceLock::new();
        REG.get_or_init(UsePatternRegistry::default)
    }

    pub fn get(&self, key: UsePatternKey) -> Arc<Regex> {
        // Fast path: try cached
        if let Some(hit) = self.cache.lock().unwrap().get(&key).cloned() {
            return hit;
        }

        // Compile, insert, return
        let re = Arc::new(key.compile());
        self.cache.lock().unwrap().insert(key, re.clone());
        re
    }

    /// Convenience: internal root pattern from a Cargo crate name.
    pub fn internal_root_for_crate(&self, crate_name: &str) -> Arc<Regex> {
        let crate_ident = crate_ident(crate_name);
        self.get(UsePatternKey::InternalRoot { crate_ident })
    }

    /// Convenience: external dep-path pattern.
    pub fn dep_path(&self) -> Arc<Regex> {
        self.get(UsePatternKey::DepPath)
    }
}
