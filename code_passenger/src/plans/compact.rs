use crate::model::{FileReport, PlanSection};
use super::{HeaderPlan, HeaderPlanV2};


pub struct CompactPlan;

impl HeaderPlan for CompactPlan {
    fn id(&self) -> &'static str { "compact" }

    fn render_for_rust(&self, report: &FileReport, begin: &str, end: &str) -> String {
        let mut out = String::new();
        out.push_str(begin);
        out.push('\n');

        out.push_str(&format!("//! file: {}\n", report.document.relative_path));
        out.push_str(&format!("//! used_deps: {:?}\n", report.used.packages));
        out.push_str("//!\n");
        for f in &report.corpus_features {
            out.push_str(&format!("#![cfg(feature = \"{}\")]\n", f));
        }

        out.push_str(end);
        out.push('\n');
        out
    }
}

impl HeaderPlanV2 for CompactPlan {
    fn id(&self) -> &'static str { "compact" }

    fn sections(&self) -> &[PlanSection] {
        const SECTIONS: &[PlanSection] = &[
            PlanSection::DocumentDetails,
            PlanSection::CorpusGates,
        ];
        SECTIONS
    }
}
