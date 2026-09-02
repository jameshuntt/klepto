use crate::error::{PassengerError, Result};
use crate::model::{FeatureNote, FileReport, PlanSection};
use std::collections::BTreeSet;

pub trait HeaderPlan: Send + Sync {
    fn id(&self) -> &'static str;

    /// Rust rendering (pack supplies markers; plan decides sections + gate selection).
    fn render_for_rust(&self, report: &FileReport, begin: &str, end: &str) -> String;
}

/// v2 plan metadata: section ordering + gate selection rules.
/// (Doesn't replace HeaderPlan yet; it complements it.)
pub trait HeaderPlanV2: Send + Sync {
    fn id(&self) -> &'static str;

    fn sections(&self) -> &[PlanSection];

    fn select_gates(&self, notes: &[FeatureNote]) -> Vec<String> {
        let mut set = BTreeSet::<String>::new();
        for n in notes {
            if n.corpus && n.used_in_file {
                set.insert(n.feature_name.clone());
            }
        }
        set.into_iter().collect()
    }
}

#[cfg(feature = "plan_verbose")]
pub mod verbose;
#[cfg(feature = "plan_compact")]
pub mod compact;

pub fn get_plan(plan: &str) -> Result<Box<dyn HeaderPlan>> {
    match plan {
        #[cfg(feature = "plan_verbose")]
        "verbose" => Ok(Box::new(verbose::VerbosePlan)),
        #[cfg(feature = "plan_compact")]
        "compact" => Ok(Box::new(compact::CompactPlan)),
        _ => Err(PassengerError::Unsupported(format!("header plan '{plan}' not available"))),
    }
}