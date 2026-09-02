use crate::klepto::Klepto;
use crate::model::*;
pub mod api;
pub mod structure;
pub mod patterns;

pub trait Rule {
    fn code(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn run(&self, k: &Klepto) -> Vec<Finding>;
}

/// Every built-in rule, API lints and structure analyses alike.
pub fn all_rules() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(api::UndocumentedPublicApi),
        Box::new(api::UnwrapInPublicApi),
        Box::new(api::StdInNoStdCrate),
        Box::new(api::PanicMacrosInPublicApi),
        Box::new(structure::UngatedDepUse),
        Box::new(structure::CfgEdgeViolations),
        Box::new(structure::UnresolvedReferences),
        Box::new(structure::UnreferencedItems),
        Box::new(structure::OrphanFiles),
        Box::new(structure::SkippedFiles),
        Box::new(patterns::DuplicateFunctions),
        Box::new(patterns::RepeatedImpls),
        Box::new(structure::CfgEdgeUnknown),
        Box::new(structure::UngatedDepUnknown),
    ]
}

/// `(code, name)` of every built-in rule, in code order.
pub fn all_codes() -> Vec<(&'static str, &'static str)> {
    let mut v: Vec<(&'static str, &'static str)> = all_rules().iter().map(|r| (r.code(), r.name())).collect();
    v.sort();
    v
}

/// Findings in a stable order — code, then location, then message.
pub fn sort_findings(findings: &mut [Finding]) {
    findings.sort_by(|a, b| {
        (a.code.as_str(), &a.location.path, a.location.line, a.location.column, a.message.as_str())
            .cmp(&(b.code.as_str(), &b.location.path, b.location.line, b.location.column, b.message.as_str()))
    });
}

pub struct RuleRunner<'k> {
    k: &'k Klepto,
    rules: Vec<Box<dyn Rule>>,
}

impl<'k> RuleRunner<'k> {
    pub fn new(k: &'k Klepto) -> Self {
        Self { k, rules: Vec::new() }
    }

    pub fn with_default_rules(mut self) -> Self {
        self.rules.push(Box::new(api::UndocumentedPublicApi));
        self.rules.push(Box::new(api::UnwrapInPublicApi));
        self.rules.push(Box::new(api::StdInNoStdCrate));
        self.rules.push(Box::new(api::PanicMacrosInPublicApi));
        self
    }

    /// The API lints plus the structure analyses.
    pub fn with_all_rules(mut self) -> Self {
        self.rules.extend(all_rules());
        self
    }

    pub fn add_rule<R: Rule + 'static>(mut self, r: R) -> Self {
        self.rules.push(Box::new(r));
        self
    }

    /// Findings of every rule, in stable order.
    pub fn run(self) -> Vec<Finding> {
        let mut all = Vec::new();
        for r in self.rules {
            all.extend(r.run(self.k));
        }
        sort_findings(&mut all);
        all
    }
}
