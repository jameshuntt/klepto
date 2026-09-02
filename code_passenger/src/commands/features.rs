//! `feature-map`, `feature-sites`, `feature-probe` (rules F1–F3 in
//! `GATE_RULES.md`): the 42-step lineage's review surface on klepto facts.

use crate::commands::{crate_name_guess, klepto_for};
use crate::error::{PassengerError, Result};
use crate::gate::relative;
use klepto::graph::NodeKind;
use klepto::{CfgPredicate, DepKind, FeatureEnable, FeatureModel, FeatureSpec, GateReason, Implication, Klepto, Receipt};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureRow {
    pub name: String,
    /// As Cargo spells them: `dep:beta`, `other`, `dep?/feat`.
    pub enables: Vec<String>,
    pub default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepRow {
    pub name: String,
    pub kind: String,
    pub optional: bool,
    /// Use sites of this dependency in the package.
    pub sites: usize,
    /// KLEP010 findings for this dependency.
    pub ungated: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageMap {
    pub name: String,
    pub features: Vec<FeatureRow>,
    pub deps: Vec<DepRow>,
    /// Items whose own cfg names a feature.
    pub gated_items: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureMap {
    pub root: String,
    pub packages: Vec<PackageMap>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteRow {
    pub line: Option<u32>,
    pub kind: String,
    pub path: String,
    pub scope: String,
    pub cfg: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSites {
    /// Relative to the root, `/`-separated.
    pub file: String,
    pub sites: Vec<SiteRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureSites {
    pub root: String,
    pub target: String,
    /// `"dep"` or `"feature"`.
    pub kind: String,
    pub deps: Vec<String>,
    pub files: Vec<FileSites>,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UngatedRow {
    pub file: String,
    pub line: Option<u32>,
    pub scope: String,
    pub dep: String,
    pub site_cfg: String,
    pub required: String,
    pub verdict: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposedGate {
    pub item: String,
    pub file: String,
    pub line: Option<u32>,
    pub attribute: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureProbe {
    pub root: String,
    pub target: String,
    pub kind: String,
    pub deps: Vec<String>,
    pub spec: Option<String>,
    pub ungated: Vec<UngatedRow>,
    pub proposed: Vec<ProposedGate>,
    /// Sorted item ids the plan would gate, plus `reaches_public`.
    pub lost: Vec<String>,
    /// Always false: a probe reviews, it never writes.
    pub writes: bool,
}

// ---------------------------------------------------------------------------

fn cfg_text(p: &CfgPredicate) -> String {
    if *p == CfgPredicate::True { "true".into() } else { p.render() }
}

fn enable_text(e: &FeatureEnable) -> String {
    match e {
        FeatureEnable::Feature(f) => f.clone(),
        FeatureEnable::Dep(d) => format!("dep:{d}"),
        FeatureEnable::DepFeature { dep, feature, weak } => format!("{dep}{}/{feature}", if *weak { "?" } else { "" }),
    }
}

fn dep_kind_text(k: DepKind) -> &'static str {
    match k {
        DepKind::Normal => "normal",
        DepKind::Dev => "dev",
        DepKind::Build => "build",
    }
}

fn verdict_word(i: &Implication) -> &'static str {
    match i {
        Implication::Always => "always",
        Implication::Never => "never",
        Implication::Sometimes { .. } => "sometimes",
        Implication::Unknown => "unknown",
    }
}

/// `default` and everything it enables, transitively.
fn default_closure(model: &FeatureModel) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut stack = vec!["default".to_string()];
    while let Some(f) = stack.pop() {
        if !out.insert(f.clone()) {
            continue;
        }
        if let Some(info) = model.features.get(&f) {
            for e in &info.enables {
                if let FeatureEnable::Feature(g) = e {
                    stack.push(g.clone());
                }
            }
        }
    }
    out
}

fn packages_for<'k>(k: &'k Klepto, package: Option<&str>) -> Result<Vec<(&'k String, &'k FeatureModel)>> {
    match package {
        Some(p) => {
            let p = p.replace('-', "_");
            k.features
                .get_key_value(&p)
                .map(|(n, m)| vec![(n, m)])
                .ok_or_else(|| PassengerError::Unsupported(format!("no feature model for package `{p}`")))
        }
        None => Ok(k.features.iter().collect()),
    }
}

/// A target is a dependency name or a feature name; returns
/// `(kind, deps, owning packages)` (rule F2).
fn classify(k: &Klepto, package: Option<&str>, target: &str) -> Result<(String, Vec<String>, Vec<String>)> {
    // crate names are spelled with `_` in klepto; feature names keep their dashes
    let t = target.trim().replace('-', "_");
    let feature = target.trim().to_string();
    let mut deps: BTreeSet<String> = BTreeSet::new();
    let mut owners: Vec<String> = Vec::new();
    let mut kind = None;
    for (name, model) in packages_for(k, package)? {
        if model.dep(&t).is_some() {
            kind.get_or_insert("dep".to_string());
            deps.insert(t.clone());
            owners.push(name.clone());
        } else if let Some(f) = [feature.as_str(), t.as_str()].into_iter().find(|f| model.features.contains_key(*f)) {
            kind.get_or_insert("feature".to_string());
            deps.extend(model.deps_enabled_by(f));
            owners.push(name.clone());
        }
    }
    let kind = kind.ok_or_else(|| PassengerError::Unsupported(format!("`{target}` is neither a dependency nor a feature of the scanned packages")))?;
    Ok((kind, deps.into_iter().collect(), owners))
}

fn root_text(root: &Path) -> String {
    root.display().to_string()
}

/// Rule F1.
pub fn feature_map(root: &Path, package: Option<&str>) -> Result<FeatureMap> {
    let k = klepto_for(root, &crate_name_guess(root))?;
    let receipt = Receipt::from_klepto(&k);
    let ungated = k.ungated_dep_uses();
    let mut packages = Vec::new();
    for (name, model) in packages_for(&k, package)? {
        let defaults = default_closure(model);
        let features = model
            .features
            .iter()
            .map(|(f, info)| FeatureRow { name: f.clone(), enables: info.enables.iter().map(enable_text).collect(), default: defaults.contains(f) })
            .collect();
        let deps = model
            .dependencies
            .iter()
            .map(|d| DepRow {
                name: d.name.clone(),
                kind: dep_kind_text(d.kind).to_string(),
                optional: d.optional,
                sites: receipt.dep_uses.iter().filter(|u| &u.package == name && u.site.dep == d.name).count(),
                ungated: ungated.iter().filter(|u| &u.package == name && u.site.dep == d.name).count(),
            })
            .collect();
        let gated_items = k
            .graph
            .nodes
            .values()
            .filter(|n| n.package.as_deref() == Some(name.as_str()) && !matches!(n.kind, NodeKind::Module | NodeKind::External) && !n.cfg.features().is_empty())
            .count();
        packages.push(PackageMap { name: name.clone(), features, deps, gated_items });
    }
    Ok(FeatureMap { root: root_text(root), packages })
}

/// Rule F2.
pub fn feature_sites(root: &Path, package: Option<&str>, target: &str) -> Result<FeatureSites> {
    let k = klepto_for(root, &crate_name_guess(root))?;
    let (kind, deps, owners) = classify(&k, package, target)?;
    let receipt = Receipt::from_klepto(&k);
    let mut by_file: BTreeMap<String, Vec<SiteRow>> = BTreeMap::new();
    for u in receipt.dep_uses.iter().filter(|u| owners.contains(&u.package) && deps.contains(&u.site.dep)) {
        by_file.entry(u.site.location.path.to_string_lossy().into_owned()).or_default().push(SiteRow {
            line: u.site.location.line,
            kind: format!("{:?}", u.site.kind).to_lowercase(),
            path: u.site.path.clone(),
            scope: u.site.scope.clone(),
            cfg: cfg_text(&u.cfg),
        });
    }
    let mut files: Vec<FileSites> = by_file
        .into_iter()
        .map(|(file, mut sites)| {
            sites.sort_by(|a, b| (a.line, &a.path).cmp(&(b.line, &b.path)));
            FileSites { file, sites }
        })
        .collect();
    files.sort_by(|a, b| a.file.cmp(&b.file));
    let total = files.iter().map(|f| f.sites.len()).sum();
    Ok(FeatureSites { root: root_text(root), target: target.to_string(), kind, deps, files, total })
}

/// Rule F3.
pub fn feature_probe(root: &Path, package: Option<&str>, target: &str, spec: Option<&Path>) -> Result<FeatureProbe> {
    let k = klepto_for(root, &crate_name_guess(root))?;
    let (kind, deps, owners) = classify(&k, package, target)?;
    let receipt = Receipt::from_klepto(&k);
    let spec_model = match spec {
        Some(p) => Some(FeatureSpec::from_path(p).map_err(|e| PassengerError::Unsupported(e.to_string()))?),
        None => None,
    };
    let mut ungated = Vec::new();
    let mut proposed = Vec::new();
    let mut lost: BTreeSet<String> = BTreeSet::new();
    for owner in &owners {
        let Some(current) = k.feature_model(owner) else { continue };
        let desired = spec_model.as_ref().map(|s| current.apply_spec(&s.package(owner).cloned().unwrap_or_default()));
        let model = desired.as_ref().unwrap_or(current);
        for u in receipt.dep_uses.iter().filter(|u| &u.package == owner && deps.contains(&u.site.dep)) {
            let required = model.dep_predicate(&u.site.dep).simplify();
            let verdict = u.cfg.implies(&required, None);
            if matches!(verdict, Implication::Always) {
                continue;
            }
            ungated.push(UngatedRow {
                file: u.site.location.path.to_string_lossy().into_owned(),
                line: u.site.location.line,
                scope: u.site.scope.clone(),
                dep: u.site.dep.clone(),
                site_cfg: cfg_text(&u.cfg),
                required: cfg_text(&required),
                verdict: verdict_word(&verdict).to_string(),
            });
        }
        if let Some(desired) = &desired {
            let plan = k.plan_gates(owner, desired);
            // gates related to the target's deps, through cascades and violations
            let mut related: BTreeSet<String> = plan
                .gates
                .iter()
                .filter(|g| g.reasons.iter().any(|r| matches!(r, GateReason::UngatedDep { dep } if deps.contains(dep))))
                .map(|g| g.item.clone())
                .collect();
            loop {
                let before = related.len();
                for g in &plan.gates {
                    let linked = g.reasons.iter().any(|r| match r {
                        GateReason::Cascade { from } => related.contains(from),
                        GateReason::ExistingViolation { to } => related.contains(to),
                        _ => false,
                    });
                    if linked {
                        related.insert(g.item.clone());
                    }
                }
                if related.len() == before {
                    break;
                }
            }
            for g in plan.gates.iter().filter(|g| related.contains(&g.item)) {
                proposed.push(ProposedGate {
                    item: g.item.clone(),
                    file: relative(root, &g.location.path).map(|r| r.to_string_lossy().into_owned()).unwrap_or_else(|_| g.location.path.display().to_string()),
                    line: g.location.line,
                    attribute: if g.existing_cfg == CfgPredicate::True {
                        g.attribute_text()
                    } else {
                        crate::gate::merged_attribute(&g.existing_cfg, &g.predicate)
                    },
                });
            }
            lost.extend(plan.gates.iter().map(|g| g.item.clone()));
            lost.extend(plan.reaches_public.iter().cloned());
        }
    }
    ungated.sort_by(|a, b| (&a.file, a.line, &a.scope).cmp(&(&b.file, b.line, &b.scope)));
    proposed.sort_by(|a, b| (&a.file, a.line, &a.item).cmp(&(&b.file, b.line, &b.item)));
    Ok(FeatureProbe {
        root: root_text(root),
        target: target.to_string(),
        kind,
        deps,
        spec: spec.map(|p| p.display().to_string()),
        ungated,
        proposed,
        lost: lost.into_iter().collect(),
        writes: false,
    })
}

// ---------------------------------------------------------------------------
// renderers (the lineage's shape)

pub fn render_map(m: &FeatureMap) -> String {
    let mut s = String::new();
    s.push_str("CODE PASSENGER :: FEATURE MAP\n─────────────────────────────\n");
    s.push_str(&format!("root: {}\n\n", m.root));
    for p in &m.packages {
        s.push_str(&format!("CRATE {}\n", p.name));
        s.push_str(&format!("  gated_items: {}\n", p.gated_items));
        s.push_str("  features:\n");
        if p.features.is_empty() {
            s.push_str("    NONE\n");
        }
        for f in &p.features {
            s.push_str(&format!("    {}{} = [{}]\n", f.name, if f.default { " (default)" } else { "" }, f.enables.join(", ")));
        }
        s.push_str("  deps:\n");
        if p.deps.is_empty() {
            s.push_str("    NONE\n");
        }
        for d in &p.deps {
            s.push_str(&format!(
                "    {} kind={} optional={} sites={} ungated={}\n",
                d.name, d.kind, d.optional, d.sites, d.ungated
            ));
        }
        s.push('\n');
    }
    s
}

pub fn render_sites(x: &FeatureSites) -> String {
    let mut s = String::new();
    s.push_str("CODE PASSENGER :: FEATURE SITES\n───────────────────────────────\n");
    s.push_str(&format!("root: {}\ntarget: {}\nkind: {}\ndeps: [{}]\n\n", x.root, x.target, x.kind, x.deps.join(", ")));
    if x.files.is_empty() {
        s.push_str("  NONE no use sites\n");
    }
    for f in &x.files {
        s.push_str(&format!("  FILE {} sites={}\n", f.file, f.sites.len()));
        for site in &f.sites {
            let line = site.line.map(|l| l.to_string()).unwrap_or_else(|| "?".into());
            s.push_str(&format!("    L{line} {} {} scope={} cfg={}\n", site.kind, site.path, site.scope, site.cfg));
        }
    }
    s.push_str(&format!("\ntotal_sites: {}\n", x.total));
    s
}

pub fn render_probe(p: &FeatureProbe) -> String {
    let mut s = String::new();
    s.push_str("CODE PASSENGER :: FEATURE PROBE\n───────────────────────────────\n");
    s.push_str(&format!("root: {}\ntarget: {}\nkind: {}\ndeps: [{}]\nspec: {}\n\n", p.root, p.target, p.kind, p.deps.join(", "), p.spec.as_deref().unwrap_or("<current manifest>")));
    s.push_str(&format!("ungated: {}\n", p.ungated.len()));
    for u in &p.ungated {
        let line = u.line.map(|l| l.to_string()).unwrap_or_else(|| "?".into());
        s.push_str(&format!("  {}:{line} {} uses {} under cfg={} needs={} ({})\n", u.file, u.scope, u.dep, u.site_cfg, u.required, u.verdict));
    }
    s.push_str(&format!("proposed: {}\n", p.proposed.len()));
    for g in &p.proposed {
        let line = g.line.map(|l| l.to_string()).unwrap_or_else(|| "?".into());
        s.push_str(&format!("  {}:{line} {} {}\n", g.file, g.attribute, g.item));
    }
    s.push_str(&format!("lost: {}\n", p.lost.len()));
    for l in &p.lost {
        s.push_str(&format!("  {l}\n"));
    }
    s.push_str("writes: false\n");
    s
}
