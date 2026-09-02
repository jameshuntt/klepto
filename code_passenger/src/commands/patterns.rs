//! `patterns` (rule R3 in `GATE_RULES.md`).

use crate::commands::{crate_name_guess, klepto_for};
use crate::error::Result;
use klepto::{patterns, MiningOptions, PatternReport};
use std::path::Path;

/// Rule R3.
pub fn run(root: &Path, options: &MiningOptions) -> Result<PatternReport> {
    let k = klepto_for(root, &crate_name_guess(root))?;
    Ok(patterns::mine(&k, options))
}

/// Rule R3: the top `n` candidates, weight order.
pub fn render(report: &PatternReport, n: usize) -> String {
    let mut s = String::new();
    s.push_str("CODE PASSENGER :: PATTERNS\n──────────────────────────\n");
    s.push_str(&format!(
        "shaped: {} clusters: {} candidates: {} shared_method_sets: {} repeated_impls: {}\n\n",
        report.shapes.len(),
        report.clusters.len(),
        report.candidates.len(),
        report.shared_method_sets.len(),
        report.repeated_impls.len()
    ));
    for c in report.candidates.iter().take(n) {
        s.push_str(&format!("[{:?} w={} {}] {}\n", c.kind, c.weight, c.level.name(), c.members.join(", ")));
        s.push_str(&format!("  {}\n", c.note));
        for p in &c.params {
            s.push_str(&format!("  {} = {}\n", p.placeholder, p.fillers.join(" | ")));
        }
        for (slot, methods) in &c.required_methods {
            s.push_str(&format!("  {slot} needs [{}]\n", methods.join(", ")));
        }
    }
    for r in &report.repeated_impls {
        s.push_str(&format!("[repeated impl {}] {} for {}\n", r.level.name(), r.trait_id, r.types.join(", ")));
    }
    for m in &report.shared_method_sets {
        s.push_str(&format!("[shared methods] [{}] on {}\n", m.methods.join(", "), m.types.join(", ")));
    }
    s
}
