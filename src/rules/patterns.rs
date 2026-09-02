//! Phase 7 rules over the pattern report (rules R1–R2 in `PHASE7_RULES.md`).

use crate::klepto::Klepto;
use crate::model::{FileLocation, Finding, Severity};
use crate::patterns::ShapeLevel;
use crate::rules::Rule;
use serde_json::json;

/// KLEP016: functions with the same code (exact or renamed).
pub struct DuplicateFunctions;

impl Rule for DuplicateFunctions {
    fn code(&self) -> &'static str { "KLEP016" }
    fn name(&self) -> &'static str { "duplicate-function" }
    fn run(&self, k: &Klepto) -> Vec<Finding> {
        let report = k.patterns();
        report
            .clusters
            .iter()
            .filter(|c| c.tightest && c.level <= ShapeLevel::Renamed)
            .map(|c| {
                let location = k
                    .functions
                    .iter()
                    .find(|f| f.fq_name == c.exemplar)
                    .map(|f| f.location.clone())
                    .unwrap_or(FileLocation { path: Default::default(), line: None, column: None });
                Finding {
                    severity: Severity::Info,
                    code: self.code().to_string(),
                    message: format!(
                        "{} functions share the same code{}: {}",
                        c.members.len(),
                        if c.level == ShapeLevel::Renamed { " up to local names" } else { "" },
                        c.members.join(", ")
                    ),
                    location,
                    extra: json!({ "level": c.level.name(), "hash": c.hash, "members": c.members }),
                }
            })
            .collect()
    }
}

/// KLEP017: one trait implemented for several types with the same bodies.
pub struct RepeatedImpls;

impl Rule for RepeatedImpls {
    fn code(&self) -> &'static str { "KLEP017" }
    fn name(&self) -> &'static str { "repeated-impl" }
    fn run(&self, k: &Klepto) -> Vec<Finding> {
        let report = k.patterns();
        report
            .repeated_impls
            .iter()
            .map(|r| {
                let location = r
                    .impls
                    .first()
                    .and_then(|id| k.impls.iter().find(|i| &i.id == id))
                    .map(|i| i.location.clone())
                    .unwrap_or(FileLocation { path: Default::default(), line: None, column: None });
                Finding {
                    severity: Severity::Info,
                    code: self.code().to_string(),
                    message: format!(
                        "`{}` is implemented with the same bodies ({}) for {}: a macro_rules! or blanket impl could replace them",
                        r.trait_id,
                        r.level.name(),
                        r.types.join(", ")
                    ),
                    location,
                    extra: json!({ "trait": r.trait_id, "level": r.level.name(), "types": r.types, "impls": r.impls }),
                }
            })
            .collect()
    }
}
