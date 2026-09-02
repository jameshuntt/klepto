//! Built-in rules over the resolved facts. Every finding carries the ids it
//! is about in
//! `extra`; file names in `extra` are relative to the scan root.
//!
//! Scale-run amendment: a verdict of `Unknown` (an `Unknown` cfg is
//! involved, e.g. a module declared inside a macro) is never a violation —
//! it is reported under its own codes, KLEP018 / KLEP019, at Warn, so a
//! policy can deny the definite ones and merely watch the undecidable ones.

use crate::resolve::cfg::Implication;
use crate::graph::{EdgeKind, NodeKind};
use crate::klepto::Klepto;
use crate::model::{FileLocation, Finding, Severity};
use crate::receipt::Relocator;
use crate::rules::Rule;
use serde_json::json;
use std::collections::BTreeSet;

fn verdict_word(i: &Implication) -> &'static str {
    match i {
        Implication::Always => "always",
        Implication::Never => "never",
        Implication::Sometimes { .. } => "sometimes",
        Implication::Unknown => "unknown",
    }
}

fn kind_word(k: NodeKind) -> &'static str {
    match k {
        NodeKind::Fn => "fn",
        NodeKind::Method => "method",
        NodeKind::Struct => "struct",
        NodeKind::Enum => "enum",
        NodeKind::Union => "union",
        NodeKind::Trait => "trait",
        NodeKind::TypeAlias => "type_alias",
        NodeKind::Const => "const",
        NodeKind::Static => "static",
        NodeKind::Macro => "macro",
        NodeKind::Module => "module",
        NodeKind::Impl => "impl",
        NodeKind::External => "external",
    }
}

fn ungated_findings(k: &Klepto, code: &str, unknown: bool, severity: Severity) -> Vec<Finding> {
    k.ungated_dep_uses()
        .into_iter()
        .filter(|u| matches!(u.verdict, Implication::Unknown) == unknown)
        .map(|u| Finding {
            severity: severity.clone(),
            code: code.to_string(),
            message: format!(
                "`{}` is used via `{}` in `{}` but the site's cfg `{}` {} the dependency's `{}`",
                u.site.dep,
                u.site.path,
                u.site.scope,
                u.site_cfg.render(),
                if unknown { "cannot be shown to imply" } else { "does not always imply" },
                u.dep_predicate.render()
            ),
            location: u.site.location.clone(),
            extra: json!({
                "dep": u.site.dep,
                "path": u.site.path,
                "scope": u.site.scope,
                "package": u.package,
                "site_cfg": u.site_cfg.render(),
                "dep_predicate": u.dep_predicate.render(),
                "verdict": verdict_word(&u.verdict),
            }),
        })
        .collect()
}

fn violation_findings(k: &Klepto, code: &str, unknown: bool, severity: Severity) -> Vec<Finding> {
    k.cfg_edge_violations()
        .into_iter()
        .filter(|v| matches!(v.verdict, Implication::Unknown) == unknown)
        .map(|v| Finding {
            severity: severity.clone(),
            code: code.to_string(),
            message: format!(
                "`{}` references `{}` under cfg `{}`, which {} the target's cfg `{}`",
                v.from,
                v.to,
                v.edge_cfg.render(),
                if unknown { "cannot be shown to imply" } else { "does not always imply" },
                v.to_cfg.render()
            ),
            location: v.location.clone(),
            extra: json!({
                "from": v.from,
                "to": v.to,
                "package": v.package,
                "edge_cfg": v.edge_cfg.render(),
                "to_cfg": v.to_cfg.render(),
                "verdict": verdict_word(&v.verdict),
            }),
        })
        .collect()
}

/// KLEP010: a dependency used where its `dep_predicate` provably need not hold.
pub struct UngatedDepUse;

impl Rule for UngatedDepUse {
    fn code(&self) -> &'static str { "KLEP010" }
    fn name(&self) -> &'static str { "ungated-dep-use" }
    fn run(&self, k: &Klepto) -> Vec<Finding> {
        ungated_findings(k, self.code(), false, Severity::Deny)
    }
}

/// KLEP019: a dependency use whose cfg cannot be judged (an Unknown cfg).
pub struct UngatedDepUnknown;

impl Rule for UngatedDepUnknown {
    fn code(&self) -> &'static str { "KLEP019" }
    fn name(&self) -> &'static str { "dep-use-unknown" }
    fn run(&self, k: &Klepto) -> Vec<Finding> {
        ungated_findings(k, self.code(), true, Severity::Warn)
    }
}

/// KLEP011: an edge whose cfg provably does not imply its target's cfg.
pub struct CfgEdgeViolations;

impl Rule for CfgEdgeViolations {
    fn code(&self) -> &'static str { "KLEP011" }
    fn name(&self) -> &'static str { "cfg-edge-violation" }
    fn run(&self, k: &Klepto) -> Vec<Finding> {
        violation_findings(k, self.code(), false, Severity::Deny)
    }
}

/// KLEP018: an edge whose cfg relation cannot be judged (an Unknown cfg).
pub struct CfgEdgeUnknown;

impl Rule for CfgEdgeUnknown {
    fn code(&self) -> &'static str { "KLEP018" }
    fn name(&self) -> &'static str { "cfg-edge-unknown" }
    fn run(&self, k: &Klepto) -> Vec<Finding> {
        violation_findings(k, self.code(), true, Severity::Warn)
    }
}

/// KLEP012: a reference the graph could not place.
pub struct UnresolvedReferences;

impl Rule for UnresolvedReferences {
    fn code(&self) -> &'static str { "KLEP012" }
    fn name(&self) -> &'static str { "unresolved-reference" }
    fn run(&self, k: &Klepto) -> Vec<Finding> {
        k.graph
            .unresolved
            .iter()
            .map(|u| {
                let (reason, candidates) = match &u.resolution {
                    crate::model::Resolution::Unresolved { reason, candidates, .. } => (format!("{reason:?}"), candidates.clone()),
                    crate::model::Resolution::Local(_) => ("UnplacedLocal".to_string(), Vec::new()),
                    crate::model::Resolution::External(_) => ("UnplacedExternal".to_string(), Vec::new()),
                };
                Finding {
                    severity: Severity::Info,
                    code: self.code().to_string(),
                    message: format!("`{}` in `{}` could not be resolved ({reason})", u.raw, u.from),
                    location: u.location.clone(),
                    extra: json!({ "from": u.from, "raw": u.raw, "reason": reason, "candidates": candidates }),
                }
            })
            .collect()
    }
}

/// KLEP013: a private item nothing references.
pub struct UnreferencedItems;

const EXPORT_ATTRS: &[&str] = &["test", "no_mangle", "export_name", "used"];

impl Rule for UnreferencedItems {
    fn code(&self) -> &'static str { "KLEP013" }
    fn name(&self) -> &'static str { "unreferenced-item" }
    fn run(&self, k: &Klepto) -> Vec<Finding> {
        let g = &k.graph;
        let referenced: BTreeSet<&str> = g
            .edges
            .iter()
            .filter(|e| matches!(e.kind, EdgeKind::Reference | EdgeKind::Import | EdgeKind::ReExport))
            .map(|e| e.to.as_str())
            .collect();
        let exported: BTreeSet<&str> = k
            .attrs
            .iter()
            .filter(|a| EXPORT_ATTRS.contains(&a.path.as_str()))
            .filter_map(|a| a.enclosing_item.as_deref())
            .collect();
        g.nodes
            .values()
            .filter(|n| {
                matches!(
                    n.kind,
                    NodeKind::Fn
                        | NodeKind::Method
                        | NodeKind::Struct
                        | NodeKind::Enum
                        | NodeKind::Union
                        | NodeKind::Trait
                        | NodeKind::TypeAlias
                        | NodeKind::Const
                        | NodeKind::Static
                        | NodeKind::Macro
                ) && n.package.is_some()
                    && !n.is_public
                    && n.name != "main"
                    && !n.id.contains(" as ") // methods of trait impls are reached through the trait
                    && !referenced.contains(n.id.as_str())
                    && !exported.contains(n.id.as_str())
            })
            .map(|n| Finding {
                severity: Severity::Info,
                code: self.code().to_string(),
                message: format!("private {} `{}` is never referenced", kind_word(n.kind), n.id),
                location: n.location.clone().unwrap_or(FileLocation { path: Default::default(), line: None, column: None }),
                extra: json!({ "item": n.id, "kind": kind_word(n.kind), "package": n.package }),
            })
            .collect()
    }
}

/// KLEP014: a `.rs` file no `mod` declaration reaches.
pub struct OrphanFiles;

impl Rule for OrphanFiles {
    fn code(&self) -> &'static str { "KLEP014" }
    fn name(&self) -> &'static str { "orphan-file" }
    fn run(&self, k: &Klepto) -> Vec<Finding> {
        let rl = Relocator::new(k.root.as_deref());
        k.modules
            .modules
            .values()
            .filter(|m| m.orphan)
            .map(|m| Finding {
                severity: Severity::Warn,
                code: self.code().to_string(),
                message: format!("`{}` is not reached by any `mod` declaration (module `{}` is an orphan)", rl.rel(&m.file), m.path),
                location: FileLocation { path: m.file.clone(), line: None, column: None },
                extra: json!({ "file": rl.rel(&m.file), "module": m.path, "package": m.package }),
            })
            .collect()
    }
}

/// KLEP015: a file skipped because it did not parse.
pub struct SkippedFiles;

impl Rule for SkippedFiles {
    fn code(&self) -> &'static str { "KLEP015" }
    fn name(&self) -> &'static str { "skipped-file" }
    fn run(&self, k: &Klepto) -> Vec<Finding> {
        let rl = Relocator::new(k.root.as_deref());
        k.skipped_files
            .iter()
            .map(|(p, err)| Finding {
                severity: Severity::Warn,
                code: self.code().to_string(),
                message: format!("`{}` was skipped: {err}", rl.rel(p)),
                location: FileLocation { path: p.clone(), line: None, column: None },
                extra: json!({ "file": rl.rel(p), "error": err }),
            })
            .collect()
    }
}
