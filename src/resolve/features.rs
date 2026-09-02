//! The Cargo feature model per package and the queries on it.

use crate::resolve::cfg::{judge_envs, CfgEnv, CfgPredicate, Implication};
use crate::klepto::Klepto;
use crate::model::{Resolution, UseSite};
use crate::resolve::{file_packages, norm};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

/// One entry of a `[features]` list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FeatureEnable {
    /// `"other_feature"`
    Feature(String),
    /// `"dep:name"`
    Dep(String),
    /// `"name/feat"` (`weak: false`, also enables an optional `name`) or
    /// `"name?/feat"` (`weak: true`, enables `feat` only if `name` is on).
    DepFeature { dep: String, feature: String, weak: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct FeatureInfo {
    pub enables: Vec<FeatureEnable>,
    /// Created by cargo for an optional dependency not referenced with `dep:`.
    pub implicit: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DepKind {
    Normal,
    Dev,
    Build,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DepInfo {
    /// The name used in source (`package = "x"` renames honoured, `-` → `_`);
    /// this is what use sites match on.
    pub name: String,
    /// The dependency's package name as published.
    pub package: String,
    pub optional: bool,
    pub kind: DepKind,
    /// `[target.'cfg(unix)'.dependencies]` → `Some(Key("unix"))`.
    pub target: Option<CfgPredicate>,
    pub default_features: bool,
    /// Features requested on the dependency itself (`features = [..]`).
    pub features: Vec<String>,
    /// An implicit feature named like the dependency exists.
    pub implicit_feature: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FeatureModel {
    pub package: String,
    /// Declared `[features]` plus implicit ones.
    pub features: BTreeMap<String, FeatureInfo>,
    pub dependencies: Vec<DepInfo>,
    /// Closed-assignment enumerations per key set (scale run: syn's twelve
    /// features enumerate 4096 assignments, and every distinct cfg asked
    /// for them again). Cleared whenever the model changes.
    #[serde(skip)]
    pub(crate) env_cache: EnvCache,
}

type EnvMemo = HashMap<Vec<String>, Option<Arc<Vec<CfgEnv>>>>;

/// A memo of `closed_envs` results; cloned with the model, never serialised.
#[derive(Default)]
pub struct EnvCache(std::sync::Mutex<EnvMemo>);

impl Clone for EnvCache {
    fn clone(&self) -> Self {
        EnvCache(std::sync::Mutex::new(self.0.lock().map(|m| m.clone()).unwrap_or_default()))
    }
}

impl std::fmt::Debug for EnvCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = self.0.lock().map(|m| m.len()).unwrap_or(0);
        write!(f, "EnvCache({n} key sets)")
    }
}

/// Cap on declared features for closed-assignment enumeration.
const MAX_CLOSED_FEATURES: usize = 12;
/// Cap on keys enumerated alongside.
const MAX_CLOSED_KEYS: usize = 4;

impl FeatureModel {
    pub fn dep(&self, name: &str) -> Option<&DepInfo> {
        self.dependencies.iter().find(|d| d.name == name)
    }

    fn is_optional_dep(&self, name: &str) -> bool {
        self.dep(name).is_some_and(|d| d.optional)
    }

    /// Transitive closure of `requested` (plus `default` when asked).
    pub fn active_features(&self, requested: &[&str], default_features: bool) -> BTreeSet<String> {
        let mut active: BTreeSet<String> = requested.iter().map(|s| s.to_string()).collect();
        if default_features && self.features.contains_key("default") {
            active.insert("default".into());
        }
        let mut work: Vec<String> = active.iter().cloned().collect();
        while let Some(f) = work.pop() {
            let Some(info) = self.features.get(&f) else { continue };
            for e in &info.enables {
                let next = match e {
                    FeatureEnable::Feature(g) => Some(g.clone()),
                    FeatureEnable::Dep(_) => None,
                    FeatureEnable::DepFeature { dep, weak: false, .. }
                        if self.is_optional_dep(dep) && self.features.get(dep).is_some_and(|i| i.implicit) =>
                    {
                        Some(dep.clone())
                    }
                    FeatureEnable::DepFeature { .. } => None,
                };
                if let Some(g) = next {
                    if active.insert(g.clone()) {
                        work.push(g);
                    }
                }
            }
        }
        active
    }

    /// Optional dependencies turned on (transitively) by one feature.
    pub fn deps_enabled_by(&self, feature: &str) -> BTreeSet<String> {
        let mut deps = BTreeSet::new();
        for f in self.active_features(&[feature], false) {
            let Some(info) = self.features.get(&f) else { continue };
            for e in &info.enables {
                match e {
                    FeatureEnable::Dep(d) if self.is_optional_dep(d) => {
                        deps.insert(d.clone());
                    }
                    FeatureEnable::DepFeature { dep, weak: false, .. } if self.is_optional_dep(dep) => {
                        deps.insert(dep.clone());
                    }
                    _ => {}
                }
            }
        }
        deps
    }

    /// Every feature that (transitively) enables an optional dependency;
    /// empty for a non-optional one.
    pub fn features_enabling(&self, dep: &str) -> BTreeSet<String> {
        if !self.is_optional_dep(dep) {
            return BTreeSet::new();
        }
        self.features.keys().filter(|f| self.deps_enabled_by(f).contains(dep)).cloned().collect()
    }

    /// The condition under which a dependency is available. A name
    /// declared several times (normal and dev, several targets) is
    /// available whenever any of its declarations is (scale-run fix: taking
    /// only the first declaration made `serde` in `serde_json` a test-only
    /// dependency).
    pub fn dep_predicate(&self, dep: &str) -> CfgPredicate {
        let mut alternatives: Vec<CfgPredicate> = Vec::new();
        for d in self.dependencies.iter().filter(|d| d.name == dep) {
            let mut p = if d.optional {
                CfgPredicate::Any(self.features_enabling(dep).into_iter().map(CfgPredicate::Feature).collect())
            } else {
                CfgPredicate::True
            };
            if let Some(t) = &d.target {
                p = p.and(t.clone());
            }
            match d.kind {
                DepKind::Dev => p = p.and(CfgPredicate::Key("test".into())),
                DepKind::Build => p = p.and(CfgPredicate::Key("build_script".into())),
                DepKind::Normal => {}
            }
            alternatives.push(p);
        }
        match alternatives.len() {
            0 => CfgPredicate::Unknown(dep.to_string()),
            1 => alternatives.pop().unwrap_or(CfgPredicate::True),
            _ => CfgPredicate::Any(alternatives).simplify(),
        }
    }

    /// Whether `dep` is enabled by `active` (target and kind ignored).
    pub fn dep_active(&self, dep: &str, active: &BTreeSet<String>) -> bool {
        match self.dep(dep) {
            None => false,
            Some(d) if !d.optional => true,
            Some(_) => self.features_enabling(dep).iter().any(|f| active.contains(f)),
        }
    }

    /// The default build: `active_features(&[], true)` as an environment with
    /// no keys.
    pub fn default_env(&self) -> CfgEnv {
        CfgEnv { features: self.active_features(&[], true), keys: BTreeSet::new() }
    }

    /// Every reachable assignment of the features `feats`
    /// (each subset, closed under the model's implications, with and without
    /// defaults) times every assignment of `keys`; the default build comes
    /// first. Only the features a question mentions are enumerated: a feature
    /// the question does not mention can only matter by implying one it does,
    /// and that assignment is reached by requesting the implied feature
    /// directly — so twenty declared features (clap) cost nothing. `None`
    /// past the caps.
    pub(crate) fn closed_envs(&self, keys: &[String], feats: &[String]) -> Option<Arc<Vec<CfgEnv>>> {
        let mut cache_key: Vec<String> = keys.to_vec();
        cache_key.push("|".to_string());
        cache_key.extend(feats.iter().cloned());
        if let Ok(cache) = self.env_cache.0.lock() {
            if let Some(hit) = cache.get(&cache_key) {
                return hit.clone();
            }
        }
        let computed = self.compute_closed_envs(keys, feats).map(Arc::new);
        if let Ok(mut cache) = self.env_cache.0.lock() {
            cache.insert(cache_key, computed.clone());
        }
        computed
    }

    fn compute_closed_envs(&self, keys: &[String], feats: &[String]) -> Option<Vec<CfgEnv>> {
        let declared: Vec<&String> = feats.iter().filter(|f| self.features.contains_key(*f)).collect();
        if declared.len() > MAX_CLOSED_FEATURES || keys.len() > MAX_CLOSED_KEYS {
            return None;
        }
        let mut seen: BTreeSet<BTreeSet<String>> = BTreeSet::new();
        let mut envs = Vec::new();
        for defaults in [true, false] {
            for mask in 0u32..(1u32 << declared.len()) {
                let requested: Vec<&str> = declared
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| mask & (1 << i) != 0)
                    .map(|(_, f)| f.as_str())
                    .collect();
                let feats = self.active_features(&requested, defaults);
                if !seen.insert(feats.clone()) {
                    continue;
                }
                for kmask in 0u32..(1u32 << keys.len()) {
                    let ks: BTreeSet<String> =
                        keys.iter().enumerate().filter(|(i, _)| kmask & (1 << i) != 0).map(|(_, k)| k.clone()).collect();
                    envs.push(CfgEnv { features: feats.clone(), keys: ks });
                }
            }
        }
        Some(envs)
    }

    /// `a ⇒ b` over closed assignments, the default build tried
    /// first as the counterexample. A feature name this package
    /// does not declare (serde's `feature = "result"` belongs to serde_core)
    /// is nothing klepto can enumerate: Unknown.
    pub fn closed_implies(&self, a: &CfgPredicate, b: &CfgPredicate) -> Implication {
        if a.is_unknown() || b.is_unknown() {
            return Implication::Unknown;
        }
        if a.features().union(&b.features()).any(|f| !self.features.contains_key(f)) {
            return Implication::Unknown;
        }
        let keys: Vec<String> = a.keys().union(&b.keys()).cloned().collect();
        let feats: Vec<String> = a.features().union(&b.features()).cloned().collect();
        let Some(envs) = self.closed_envs(&keys, &feats) else {
            return a.implies(b, Some(&self.default_env()));
        };
        let pref = self.default_env();
        if a.eval(&pref) == Some(true) && b.eval(&pref) == Some(false) {
            return Implication::Sometimes { counterexample: pref };
        }
        judge_envs(&envs, a, b)
    }
}

/// `cfg` is false in every closed assignment where `feature` is
/// inactive (an `Unknown` cfg never qualifies).
pub(crate) fn requires_feature(model: &FeatureModel, cfg: &CfgPredicate, feature: &str) -> bool {
    if cfg.is_unknown() {
        return false;
    }
    let keys: Vec<String> = cfg.keys().into_iter().collect();
    let mut feats: Vec<String> = cfg.features().into_iter().collect();
    if !feats.iter().any(|f| f == feature) {
        feats.push(feature.to_string());
    }
    let Some(envs) = model.closed_envs(&keys, &feats) else { return false };
    !envs.iter().any(|env| !env.features.contains(feature) && cfg.eval(env) == Some(true))
}

/// The dependency is unavailable in every closed assignment
/// where `feature` is inactive.
pub(crate) fn dep_lost_without(model: &FeatureModel, dep: &str, feature: &str) -> bool {
    let pred = model.dep_predicate(dep);
    if pred == CfgPredicate::True {
        return false;
    }
    requires_feature(model, &pred, feature)
}

/// A use of a dependency that is not gated by the dependency's own
/// availability condition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UngatedUse {
    pub package: String,
    pub site: UseSite,
    /// Effective cfg at the site.
    pub site_cfg: CfgPredicate,
    /// `FeatureModel::dep_predicate` of the site's dependency.
    pub dep_predicate: CfgPredicate,
    pub verdict: Implication,
}

/// Cargo's spelling of one `[features]` entry.
pub(crate) fn render_enable(e: &FeatureEnable) -> String {
    match e {
        FeatureEnable::Feature(f) => f.clone(),
        FeatureEnable::Dep(d) => format!("dep:{d}"),
        FeatureEnable::DepFeature { dep, feature, weak: true } => format!("{dep}?/{feature}"),
        FeatureEnable::DepFeature { dep, feature, weak: false } => format!("{dep}/{feature}"),
    }
}

/// Implicit features exist only for optional dependencies no
/// *other* feature references with `dep:`; recomputed from scratch.
pub(crate) fn recompute_implicit(model: &mut FeatureModel) {
    model.features.retain(|_, info| !info.implicit);
    for d in model.dependencies.iter_mut() {
        d.implicit_feature = false;
    }
    let optional: Vec<String> = model.dependencies.iter().filter(|d| d.optional).map(|d| d.name.clone()).collect();
    for dep in optional {
        let referenced = model
            .features
            .iter()
            .any(|(f, info)| f != &dep && info.enables.iter().any(|e| matches!(e, FeatureEnable::Dep(d) if d == &dep)));
        if referenced {
            continue;
        }
        let entry = model.features.entry(dep.clone()).or_insert_with(|| FeatureInfo {
            enables: vec![FeatureEnable::Dep(dep.clone())],
            implicit: true,
        });
        entry.implicit = true;
        for d in model.dependencies.iter_mut().filter(|d| d.name == dep) {
            d.implicit_feature = true;
        }
    }
}

pub(crate) fn parse_enable(s: &str) -> FeatureEnable {
    if let Some(rest) = s.strip_prefix("dep:") {
        return FeatureEnable::Dep(norm(rest));
    }
    if let Some((dep, feature)) = s.split_once('/') {
        let weak = dep.ends_with('?');
        return FeatureEnable::DepFeature { dep: norm(dep.trim_end_matches('?')), feature: feature.to_string(), weak };
    }
    FeatureEnable::Feature(s.to_string())
}

/// One model per scanned package, from `cargo metadata`.
pub(crate) fn build_feature_models(meta: &cargo_metadata::Metadata, scanned: &[String]) -> BTreeMap<String, FeatureModel> {
    let mut out = BTreeMap::new();
    for pkg in &meta.packages {
        let package = norm(&pkg.name);
        if !scanned.contains(&package) {
            continue;
        }
        let mut model = FeatureModel { package: package.clone(), ..Default::default() };
        for d in &pkg.dependencies {
            let kind = match d.kind {
                cargo_metadata::DependencyKind::Development => DepKind::Dev,
                cargo_metadata::DependencyKind::Build => DepKind::Build,
                _ => DepKind::Normal,
            };
            model.dependencies.push(DepInfo {
                name: norm(d.rename.as_deref().unwrap_or(&d.name)),
                package: d.name.clone(),
                optional: d.optional,
                kind,
                target: d.target.as_ref().map(|t| CfgPredicate::parse_str(&t.to_string())),
                default_features: d.uses_default_features,
                features: d.features.clone(),
                implicit_feature: false,
            });
        }
        for (name, list) in &pkg.features {
            let enables = list.iter().map(|s| parse_enable(s)).collect();
            model.features.insert(name.clone(), FeatureInfo { enables, implicit: false });
        }

        // Implicit features for optional dependencies not referenced with `dep:`
        recompute_implicit(&mut model);

        out.insert(package, model);
    }
    out
}

impl Klepto {
    pub fn feature_model(&self, package: &str) -> Option<&FeatureModel> {
        self.features.get(package)
    }

    /// Fq_names of items that cannot exist with `feature` off.
    pub fn items_requiring_feature(&self, package: &str, feature: &str) -> Vec<String> {
        let Some(model) = self.features.get(package) else {
            return Vec::new();
        };
        let file_pkg = file_packages(self);
        let mut out = Vec::new();
        for f in &self.functions {
            if file_pkg.get(&f.location.path).map(String::as_str) != Some(package) {
                continue;
            }
            if requires_feature(model, &f.cfg, feature) {
                out.push(f.fq_name.clone());
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Every dependency use whose effective cfg does not guarantee the
    /// dependency is available.
    pub fn ungated_dep_uses(&self) -> Vec<UngatedUse> {
        let sites = self.sites_with_cfg_and_package(
            &|res, _| match res {
                Resolution::External(e) => vec![e.clone()],
                _ => Vec::new(),
            },
            true,
        );
        let mut out = Vec::new();
        // distinct (package, dependency, site cfg) triples are few: judge each once
        let mut preds: BTreeMap<(String, String), CfgPredicate> = BTreeMap::new();
        let mut memo: BTreeMap<(String, String, String), Implication> = BTreeMap::new();
        for (site, cfg, package) in sites {
            let Some(model) = self.features.get(&package) else { continue };
            if model.dep(&site.dep).is_none() {
                continue;
            }
            let dep_predicate = preds
                .entry((package.clone(), site.dep.clone()))
                .or_insert_with(|| model.dep_predicate(&site.dep))
                .clone();
            if dep_predicate == CfgPredicate::True {
                continue;
            }
            let verdict = memo
                .entry((package.clone(), site.dep.clone(), cfg.render()))
                .or_insert_with(|| model.closed_implies(&cfg, &dep_predicate))
                .clone();
            if verdict != Implication::Always {
                out.push(UngatedUse { package, site, site_cfg: cfg, dep_predicate, verdict });
            }
        }
        out
    }
}
