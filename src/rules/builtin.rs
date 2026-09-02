use crate::find::is_panicking_callee;
use crate::klepto::Klepto;
use crate::model::*;
use crate::rules::Rule;
use serde_json::json;

pub struct UndocumentedPublicApi;
impl Rule for UndocumentedPublicApi {
    fn code(&self) -> &'static str { "KLEP001" }
    fn name(&self) -> &'static str { "Undocumented public API" }

    fn run(&self, k: &Klepto) -> Vec<Finding> {
        k.functions.iter()
            .filter(|f| f.is_public && !f.has_docs)
            .map(|f| Finding {
                severity: Severity::Warn,
                code: self.code().into(),
                message: format!("public function missing docs: {}", f.fq_name),
                location: f.location.clone(),
                extra: json!({ "signature": f.signature }),
            })
            .collect()
    }
}

pub struct UnwrapInPublicApi;
impl Rule for UnwrapInPublicApi {
    fn code(&self) -> &'static str { "KLEP002" }
    fn name(&self) -> &'static str { "unwrap/expect in public API" }

    fn run(&self, k: &Klepto) -> Vec<Finding> {
        k.calls.iter()
            .filter(|c| c.enclosing_public == Some(true))
            .filter(|c| is_panicking_callee(&c.callee))
            .map(|c| Finding {
                severity: Severity::Warn,
                code: self.code().into(),
                message: format!(
                    "panic-ish call inside public fn {}: {}",
                    c.enclosing_fn.clone().unwrap_or_else(|| "<unknown>".into()),
                    c.callee
                ),
                location: c.location.clone(),
                extra: json!({ "enclosing_fn": c.enclosing_fn, "callee": c.callee }),
            })
            .collect()
    }
}

pub struct PanicMacrosInPublicApi;
impl Rule for PanicMacrosInPublicApi {
    fn code(&self) -> &'static str { "KLEP003" }
    fn name(&self) -> &'static str { "panic/todo/unreachable in public fns" }

    fn run(&self, k: &Klepto) -> Vec<Finding> {
        k.macros_inv.iter()
            .filter(|m| m.enclosing_public == Some(true))
            .filter(|m| matches!(m.name.as_str(), "panic" | "todo" | "unreachable" | "unimplemented"))
            .map(|m| Finding {
                severity: Severity::Warn,
                code: self.code().into(),
                message: format!(
                    "macro {}! inside public fn {}",
                    m.name,
                    m.enclosing_fn.clone().unwrap_or_else(|| "<unknown>".into())
                ),
                location: m.location.clone(),
                extra: json!({ "enclosing_fn": m.enclosing_fn, "macro": m.name }),
            })
            .collect()
    }
}

pub struct StdInNoStdCrate;
impl Rule for StdInNoStdCrate {
    fn code(&self) -> &'static str { "KLEP004" }
    fn name(&self) -> &'static str { "std usage in no_std crate" }

    fn run(&self, k: &Klepto) -> Vec<Finding> {
        if !k.no_std_detected { return Vec::new(); }

        // flag std:: imports and std paths
        let mut out = Vec::new();

        // Phase 2 amendment: a `std` use under a cfg (e.g. `feature = "std"`) is the
        // normal no_std pattern; only ungated uses are findings.
        for i in &k.imports {
            if i.root == "std" && i.cfg == crate::cfg::CfgPredicate::True {
                out.push(Finding {
                    severity: Severity::Deny,
                    code: self.code().into(),
                    message: format!("std import in no_std crate: {}", i.full_path),
                    location: i.location.clone(),
                    extra: json!({ "import": i.full_path }),
                });
            }
        }

        for p in &k.paths {
            if (p.path.starts_with("std::") || p.path == "std") && p.cfg == crate::cfg::CfgPredicate::True {
                out.push(Finding {
                    severity: Severity::Deny,
                    code: self.code().into(),
                    message: format!("std path in no_std crate: {}", p.path),
                    location: p.location.clone(),
                    extra: json!({ "path": p.path, "module": p.module_path }),
                });
            }
        }

        out
    }
}
