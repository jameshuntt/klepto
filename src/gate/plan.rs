//! Gate proposals. The plan is
//! data; nothing here edits a file.

use crate::resolve::cfg::{CfgPredicate, Implication};
use crate::resolve::features::FeatureModel;
use crate::graph::{EdgeKind, NodeKind};
use crate::klepto::Klepto;
use crate::model::{FileLocation, Resolution};
use crate::resolve::{file_packages, join_key, norm};
use crate::gate::spec::{manifest_edits, ManifestEdit};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum GateKind {
    /// An attribute on an item (fn, method, struct, impl block, …).
    Item,
    /// An attribute on a module-level `use` (or `extern crate`) item.
    Use,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum GateReason {
    /// The carrier uses dependency `dep` without its availability condition.
    UngatedDep { dep: String },
    /// The carrier references a gated item `to` without its condition.
    ExistingViolation { to: String },
    /// The carrier depends on `from`, which this plan gates.
    Cascade { from: String },
    /// The compiler reported an error here in a configuration the static
    /// analysis thought was fine.
    Forced { code: String, mention: String },
}

/// One `#[cfg(..)]` to add.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GateEdit {
    /// Carrier id (node id); for `GateKind::Use`, the module key.
    pub item: String,
    pub kind: GateKind,
    /// Where the attribute goes: the carrier's declaration start.
    pub location: FileLocation,
    /// The carrier's cfg before this plan.
    pub existing_cfg: CfgPredicate,
    /// The predicate to add (already simplified).
    pub predicate: CfgPredicate,
    pub reasons: Vec<GateReason>,
}

impl GateEdit {
    /// The attribute to write: `#[cfg(<predicate>)]`.
    pub fn attribute_text(&self) -> String {
        format!("#[cfg({})]", self.predicate.render())
    }
}

/// A site the planner cannot judge.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnknownGate {
    pub item: String,
    pub location: FileLocation,
    pub why: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GatePlan {
    pub package: String,
    pub manifest: Vec<ManifestEdit>,
    /// Sorted by (file, line).
    pub gates: Vec<GateEdit>,
    /// Public items this plan gates: functionality the feature will remove.
    pub reaches_public: Vec<String>,
    pub unknown: Vec<UnknownGate>,
}

impl GatePlan {
    pub fn gate_for(&self, item: &str) -> Option<&GateEdit> {
        self.gates.iter().find(|g| g.item == item && g.kind == GateKind::Item)
    }

    pub fn is_empty(&self) -> bool {
        self.manifest.is_empty() && self.gates.is_empty()
    }
}

/// A gate the converge loop learned from the compiler.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ForcedGate {
    pub item: String,
    pub predicate: CfgPredicate,
    pub reason: GateReason,
}

// ---------------------------------------------------------------------------

fn crate_root(s: &str) -> &str {
    s.split("::").next().unwrap_or("")
}

/// One dependency use, with what the planner needs to place a gate.
struct Site {
    dep: String,
    carrier: Option<String>,
    /// A module-level `use` / `extern crate`: the item itself is the carrier.
    module_item: bool,
    module: String,
    location: FileLocation,
    cfg: CfgPredicate,
}

fn dep_sites(k: &Klepto, package: &str, desired: &FeatureModel) -> Vec<Site> {
    let file_pkg = file_packages(k);
    let in_pkg = |p: &Path| file_pkg.get(p).map(String::as_str) == Some(package);
    let dep_of = |res: Option<&Resolution>| -> Option<String> {
        match res {
            Some(Resolution::External(e)) => {
                let root = crate_root(e);
                desired.dep(root).map(|_| root.to_string())
            }
            _ => None,
        }
    };
    let mut out = Vec::new();

    for imp in &k.imports {
        if !in_pkg(&imp.location.path) {
            continue;
        }
        if let Some(dep) = dep_of(imp.resolution.as_ref()) {
            out.push(Site {
                dep,
                carrier: imp.enclosing_item.clone(),
                module_item: imp.enclosing_item.is_none(),
                module: join_key(package, &imp.module_path),
                location: imp.location.clone(),
                cfg: imp.cfg.clone(),
            });
        }
    }
    for p in &k.paths {
        if !in_pkg(&p.location.path) {
            continue;
        }
        if let Some(dep) = dep_of(p.resolution.as_ref()) {
            out.push(Site { dep, carrier: p.enclosing_item.clone(), module_item: false, module: join_key(package, &p.module_path), location: p.location.clone(), cfg: p.cfg.clone() });
        }
    }
    for m in &k.macros_inv {
        if !in_pkg(&m.location.path) {
            continue;
        }
        if let Some(dep) = dep_of(m.resolution.as_ref()) {
            out.push(Site { dep, carrier: m.enclosing_item.clone(), module_item: false, module: join_key(package, &m.module_path), location: m.location.clone(), cfg: m.cfg.clone() });
        }
    }
    for a in &k.attrs {
        if !in_pkg(&a.location.path) {
            continue;
        }
        for res in std::iter::once(a.resolution.as_ref()).chain(a.arg_resolutions.iter().map(Some)) {
            if let Some(dep) = dep_of(res) {
                out.push(Site { dep, carrier: a.enclosing_item.clone(), module_item: false, module: join_key(package, &a.module_path), location: a.location.clone(), cfg: a.cfg.clone() });
            }
        }
    }
    for e in &k.extern_crates {
        if !in_pkg(&e.location.path) {
            continue;
        }
        let res = Resolution::External(norm(&e.name));
        if let Some(dep) = dep_of(Some(&res)) {
            out.push(Site { dep, carrier: None, module_item: true, module: join_key(package, &e.module_path), location: e.location.clone(), cfg: e.cfg.clone() });
        }
    }
    out
}

struct Planner<'a> {
    k: &'a Klepto,
    package: &'a str,
    desired: &'a FeatureModel,
    gates: HashMap<String, GateEdit>,
    unknown: Vec<UnknownGate>,
}

impl Planner<'_> {
    /// Merge into an existing gate; returns whether the predicate changed.
    fn add(&mut self, key: String, edit: GateEdit) -> bool {
        match self.gates.get_mut(&key) {
            None => {
                self.gates.insert(key, edit);
                true
            }
            Some(g) => {
                let merged = g.predicate.clone().and(edit.predicate).simplify();
                let changed = merged != g.predicate;
                g.predicate = merged;
                for r in edit.reasons {
                    if !g.reasons.contains(&r) {
                        g.reasons.push(r);
                    }
                }
                changed
            }
        }
    }

    fn gate_item(&mut self, id: &str, predicate: CfgPredicate, reason: GateReason) -> bool {
        let Some(node) = self.k.graph.node(id) else { return false };
        let location = node.location.clone().unwrap_or(FileLocation { path: PathBuf::new(), line: None, column: None });
        if node.cfg.is_unknown() {
            self.unknown.push(UnknownGate { item: id.to_string(), location, why: "carrier cfg is Unknown".into() });
            return false;
        }
        let edit = GateEdit { item: id.to_string(), kind: GateKind::Item, location, existing_cfg: node.cfg.clone(), predicate, reasons: vec![reason] };
        self.add(id.to_string(), edit)
    }

    fn gate_use(&mut self, module: String, location: FileLocation, existing_cfg: CfgPredicate, predicate: CfgPredicate, reason: GateReason) {
        let key = format!("use@{}:{:?}:{:?}", location.path.display(), location.line, location.column);
        let edit = GateEdit { item: module, kind: GateKind::Use, location, existing_cfg, predicate, reasons: vec![reason] };
        self.add(key, edit);
    }

    /// The carrier's cfg once this plan is applied.
    fn effective(&self, id: &str) -> Option<CfgPredicate> {
        let node = self.k.graph.node(id)?;
        Some(match self.gates.get(id) {
            Some(g) => g.existing_cfg.clone().and(g.predicate.clone()).simplify(),
            None => node.cfg.clone(),
        })
    }

    /// Propagates a gate to the items that depend on the gated one, until
    /// nothing new is gated.
    fn cascade(&mut self, mut work: Vec<String>) {
        while let Some(x) = work.pop() {
            let Some(gx) = self.gates.get(&x).cloned() else { continue };
            let x_cfg = gx.existing_cfg.clone().and(gx.predicate.clone()).simplify();
            for y in self.k.graph.dependents_of(&x) {
                let Some(yn) = self.k.graph.node(&y) else { continue };
                if yn.package.as_deref() != Some(self.package) || yn.cfg.is_unknown() {
                    continue;
                }
                let y_cfg = self.effective(&y).unwrap_or_else(|| yn.cfg.clone());
                if self.desired.closed_implies(&y_cfg, &x_cfg) == Implication::Always {
                    continue;
                }
                if self.gate_item(&y, gx.predicate.clone(), GateReason::Cascade { from: x.clone() }) {
                    work.push(y);
                }
            }
        }
    }
}

impl Klepto {
    /// The gate plan for `package` against the desired model.
    pub fn plan_gates(&self, package: &str, desired: &FeatureModel) -> GatePlan {
        self.plan_gates_with(package, desired, &[])
    }

    /// Same, with extra gates the compiler oracle insisted on.
    pub fn plan_gates_with(&self, package: &str, desired: &FeatureModel, forced: &[ForcedGate]) -> GatePlan {
        let mut pl = Planner { k: self, package, desired, gates: HashMap::new(), unknown: Vec::new() };

        // Every ungated dependency use
        for s in dep_sites(self, package, desired) {
            let p = desired.dep_predicate(&s.dep).simplify();
            if p == CfgPredicate::True {
                continue;
            }
            let item = s.carrier.clone().unwrap_or_else(|| s.module.clone());
            if p.is_unknown() || s.cfg.is_unknown() {
                pl.unknown.push(UnknownGate { item, location: s.location.clone(), why: format!("cannot judge use of `{}`: Unknown cfg", s.dep) });
                continue;
            }
            match desired.closed_implies(&s.cfg, &p) {
                Implication::Always => continue,
                Implication::Unknown => {
                    pl.unknown.push(UnknownGate { item, location: s.location.clone(), why: format!("cannot judge use of `{}`", s.dep) });
                    continue;
                }
                _ => {}
            }
            let reason = GateReason::UngatedDep { dep: s.dep.clone() };
            match (&s.carrier, s.module_item) {
                (Some(id), _) => {
                    pl.gate_item(id, p, reason);
                }
                (None, true) => pl.gate_use(s.module.clone(), s.location.clone(), s.cfg.clone(), p, reason),
                (None, false) => pl.unknown.push(UnknownGate { item, location: s.location.clone(), why: "module-level occurrence has no carrier".into() }),
            }
        }

        // References to already-gated items
        let g = &self.graph;
        for e in g.edges.iter().filter(|e| matches!(e.kind, EdgeKind::Reference | EdgeKind::ReExport)) {
            let (Some(from), Some(to)) = (g.node(&e.from), g.node(&e.to)) else { continue };
            if from.package.as_deref() != Some(package) || to.package.as_deref() != Some(package) {
                continue;
            }
            if to.cfg == CfgPredicate::True || to.cfg.is_unknown() || e.cfg.is_unknown() || from.kind == NodeKind::Module {
                continue;
            }
            match desired.closed_implies(&e.cfg, &to.cfg) {
                Implication::Always => {}
                Implication::Unknown => pl.unknown.push(UnknownGate { item: e.from.clone(), location: e.location.clone(), why: format!("cannot judge reference to `{}`", e.to) }),
                _ => {
                    pl.gate_item(&e.from, to.cfg.clone().simplify(), GateReason::ExistingViolation { to: e.to.clone() });
                }
            }
        }

        // What the compiler insisted on
        for f in forced {
            pl.gate_item(&f.item, f.predicate.clone().simplify(), f.reason.clone());
        }

        // Propagate
        let seeds: Vec<String> = pl.gates.values().filter(|g| g.kind == GateKind::Item).map(|g| g.item.clone()).collect();
        pl.cascade(seeds);
        let mut gates: Vec<GateEdit> = pl.gates.into_values().collect();
        gates.sort_by(|a, b| (&a.location.path, a.location.line, a.location.column).cmp(&(&b.location.path, b.location.line, b.location.column)));
        let mut reaches_public: Vec<String> = gates
            .iter()
            .filter(|g| g.kind == GateKind::Item && self.graph.node(&g.item).is_some_and(|n| n.is_public))
            .map(|g| g.item.clone())
            .collect();
        reaches_public.sort();
        reaches_public.dedup();

        let manifest = self.features.get(package).map(|current| manifest_edits(current, desired)).unwrap_or_default();

        GatePlan { package: package.to_string(), manifest, gates, reaches_public, unknown: pl.unknown }
    }
}
