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
        // best-effort: if file contains unwrap calls, flag them if in same module as any public fn
        // (You can tighten this later by mapping call spans -> enclosing fn.)
        let pub_mods: std::collections::BTreeSet<Vec<String>> =
            k.functions.iter().filter(|f| f.is_public).map(|f| f.module_path.clone()).collect();

        // k.calls.iter()
        //     .filter(|c| pub_mods.contains(&c.module_path))
        //     .filter(|c| c.callee.contains("unwrap") || c.callee.contains("expect"))
        //     .map(|c| Finding {
        //         severity: Severity::Warn,
        //         code: self.code().into(),
        //         message: format!("potential panic path in public module: {}", c.callee),
        //         location: c.location.clone(),
        //         extra: json!({ "module": c.module_path }),
        //     })
        //     .collect()
        k.calls.iter()
            .filter(|c| c.enclosing_public == Some(true))
            .filter(|c| c.callee.contains("unwrap") || c.callee.contains("expect"))
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
    fn name(&self) -> &'static str { "panic/todo/unreachable in public modules" }

    fn run(&self, k: &Klepto) -> Vec<Finding> {
        let pub_mods: std::collections::BTreeSet<Vec<String>> =
            k.functions.iter().filter(|f| f.is_public).map(|f| f.module_path.clone()).collect();

        // k.macros_inv.iter()
        //     .filter(|m| pub_mods.contains(&m.module_path))
        //     .filter(|m| matches!(m.name.as_str(), "panic" | "todo" | "unreachable"))
        //     .map(|m| Finding {
        //         severity: Severity::Warn,
        //         code: self.code().into(),
        //         message: format!("macro in public module: {}!", m.name),
        //         location: m.location.clone(),
        //         extra: json!({ "module": m.module_path }),
        //     })
        //     .collect()
        k.macros_inv.iter()
            .filter(|m| m.enclosing_public == Some(true))
            .filter(|m| matches!(m.name.as_str(), "panic" | "todo" | "unreachable"))
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

        for i in &k.imports {
            if i.root == "std" {
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
            if p.path.starts_with("std::") || p.path == "std" {
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
