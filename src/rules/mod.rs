use crate::klepto::Klepto;
use crate::model::*;
pub mod builtin;

pub trait Rule {
    fn code(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn run(&self, k: &Klepto) -> Vec<Finding>;
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

    pub fn add_rule<R: Rule + 'static>(mut self, r: R) -> Self {
        self.rules.push(Box::new(r));
        self
    }

    pub fn run(self) -> Vec<Finding> {
        let mut all = Vec::new();
        for r in self.rules {
            all.extend(r.run(self.k));
        }
        all
    }
}
