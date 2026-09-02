//! The klepto-backed commands (rules G, F, R, X in `GATE_RULES.md`).
//! Each module exposes a library function the CLI and the tests call.

pub mod features;
pub mod gate;
pub mod lint;
pub mod patterns;
pub mod receipt;

use crate::error::{PassengerError, Result};
use klepto::Klepto;
use std::path::Path;

/// Rule G1: klepto over the whole workspace, as `scan` builds it.
pub fn klepto_for(root: &Path, crate_name: &str) -> Result<Klepto> {
    Klepto::new(crate_name)
        .scan_workspace_root(root)
        .map_err(|e| PassengerError::Unsupported(e.to_string()))?
        .exclude_generated()
        .map_err(|e| PassengerError::Unsupported(e.to_string()))?
        .parse()
        .map_err(|e| PassengerError::Unsupported(e.to_string()))
}

/// The name klepto is built with: the root manifest's `[package].name`, or
/// `workspace` for a virtual manifest.
pub fn crate_name_guess(root: &Path) -> String {
    crate::cargo::load_manifest(&root.join("Cargo.toml").to_string_lossy())
        .map(|m| m.crate_name.replace('-', "_"))
        .unwrap_or_else(|_| "workspace".to_string())
}

/// Rule G1: `--package`, else the root manifest's `[package].name`, else an
/// error naming the scanned packages.
pub fn resolve_package(k: &Klepto, root: &Path, requested: Option<&str>) -> Result<String> {
    if let Some(p) = requested {
        return Ok(p.replace('-', "_"));
    }
    if let Ok(manifest) = crate::cargo::load_manifest(&root.join("Cargo.toml").to_string_lossy()) {
        return Ok(manifest.crate_name.replace('-', "_"));
    }
    let names: Vec<&String> = k.features.keys().collect();
    Err(PassengerError::Unsupported(format!("no --package given and the root manifest has no [package]; scanned packages: {names:?}")))
}
