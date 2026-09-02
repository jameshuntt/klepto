use crate::klepto::Klepto;
use crate::model::*;
pub mod builtin;
pub mod phase6;
pub mod patterns;

pub trait Rule {
    fn code(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn run(&self, k: &Klepto) -> Vec<Finding>;
}

/// Every built-in rule, Phase 0 lints and Phase 6 analyses alike (rule A1).
pub fn all_rules() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(builtin::UndocumentedPublicApi),
        Box::new(builtin::UnwrapInPublicApi),
        Box::new(builtin::StdInNoStdCrate),
        Box::new(builtin::PanicMacrosInPublicApi),
        Box::new(phase6::UngatedDepUse),
        Box::new(phase6::CfgEdgeViolations),
        Box::new(phase6::UnresolvedReferences),
        Box::new(phase6::UnreferencedItems),
        Box::new(phase6::OrphanFiles),
        Box::new(phase6::SkippedFiles),
        Box::new(patterns::DuplicateFunctions),
        Box::new(patterns::RepeatedImpls),
        Box::new(phase6::CfgEdgeUnknown),
        Box::new(phase6::UngatedDepUnknown),
    ]
}

/// `(code, name)` of every built-in rule, in code order.
pub fn all_codes() -> Vec<(&'static str, &'static str)> {
    let mut v: Vec<(&'static str, &'static str)> = all_rules().iter().map(|r| (r.code(), r.name())).collect();
    v.sort();
    v
}

/// Rule A3: findings in a stable order — code, then location, then message.
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
        self.rules.push(Box::new(builtin::UndocumentedPublicApi));
        self.rules.push(Box::new(builtin::UnwrapInPublicApi));
        self.rules.push(Box::new(builtin::StdInNoStdCrate));
        self.rules.push(Box::new(builtin::PanicMacrosInPublicApi));
        self
    }

    /// The Phase 0 lints plus the Phase 6 analyses (rule A1).
    pub fn with_all_rules(mut self) -> Self {
        self.rules.extend(all_rules());
        self
    }

    pub fn add_rule<R: Rule + 'static>(mut self, r: R) -> Self {
        self.rules.push(Box::new(r));
        self
    }

    /// Findings of every rule, in the stable order of rule A3.
    pub fn run(self) -> Vec<Finding> {
        let mut all = Vec::new();
        for r in self.rules {
            all.extend(r.run(self.k));
        }
        sort_findings(&mut all);
        all
    }
}
