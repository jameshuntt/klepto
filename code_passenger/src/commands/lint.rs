//! `lint` (rule R2 in `GATE_RULES.md`).

use crate::commands::{crate_name_guess, klepto_for};
use crate::error::{PassengerError, Result};
use klepto::{run_pipeline, Policy, Verdict};
use std::path::Path;

/// Rule R2: the policy used when none is given.
pub const DEFAULT_POLICY: &str = "[levels]\nKLEP010 = \"deny\"\nKLEP011 = \"deny\"\nKLEP001 = \"allow\"\nKLEP012 = \"allow\"\n";

/// Rule R2.
pub fn run(root: &Path, policy: Option<&Path>) -> Result<Verdict> {
    let text = match policy {
        Some(p) => std::fs::read_to_string(p)?,
        None => DEFAULT_POLICY.to_string(),
    };
    let policy = Policy::parse(&text).map_err(|e| PassengerError::Unsupported(e.to_string()))?;
    let k = klepto_for(root, &crate_name_guess(root))?;
    Ok(run_pipeline(&k, &policy))
}
