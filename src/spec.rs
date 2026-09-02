//! Phase 4: the desired feature layout, read from `klepto.toml` — rules
//! S1–S5 in `PHASE4_RULES.md`.

use crate::features::{parse_enable, recompute_implicit, render_enable, FeatureInfo, FeatureModel};
use crate::resolve::norm;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// One package's section of `klepto.toml`:
///
/// ```toml
/// [app]
/// optional = ["beta"]          # dependencies to make optional
/// default = ["fast"]           # replaces the `default` feature list
/// [app.features]               # same value syntax as Cargo `[features]`
/// a = ["dep:alpha", "dep:beta"]
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PackageSpec {
    #[serde(default)]
    pub features: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub optional: BTreeSet<String>,
    #[serde(default)]
    pub default: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeatureSpec {
    /// Keyed by package name (`-` → `_` applied on parse).
    pub packages: BTreeMap<String, PackageSpec>,
}

#[derive(Debug, thiserror::Error)]
pub enum SpecError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid klepto.toml: {0}")]
    Toml(String),
}

impl FeatureSpec {
    /// Rule S1.
    pub fn parse(text: &str) -> Result<Self, SpecError> {
        let raw: BTreeMap<String, PackageSpec> = toml::from_str(text).map_err(|e| SpecError::Toml(e.to_string()))?;
        let packages = raw.into_iter().map(|(k, v)| (k.replace('-', "_"), v)).collect();
        Ok(FeatureSpec { packages })
    }

    pub fn from_path(path: &Path) -> Result<Self, SpecError> {
        let text = std::fs::read_to_string(path).map_err(|e| SpecError::Io { path: path.to_path_buf(), source: e })?;
        Self::parse(&text)
    }

    pub fn package(&self, name: &str) -> Option<&PackageSpec> {
        self.packages.get(&name.replace('-', "_"))
    }
}

/// What must change in `Cargo.toml` (rule S5). Applying these is the
/// consumer's job (`code_passenger` with `toml_edit`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ManifestEdit {
    MakeOptional { dep: String },
    /// Set a feature's list (creating the feature if new).
    SetFeature { name: String, enables: Vec<String> },
    SetDefault { features: Vec<String> },
}

impl FeatureModel {
    /// Rules S2–S4: this model with `spec` applied (features replaced or
    /// added, dependencies made optional, `default` replaced, implicit
    /// features recomputed).
    pub fn apply_spec(&self, spec: &PackageSpec) -> FeatureModel {
        let mut m = self.clone();
        m.env_cache = crate::features::EnvCache::default(); // the memo belongs to the old model
        // S3: optional (unknown names are ignored)
        for name in &spec.optional {
            let n = norm(name);
            for d in m.dependencies.iter_mut().filter(|d| d.name == n) {
                d.optional = true;
            }
        }
        // S2: features
        for (name, list) in &spec.features {
            let enables = list.iter().map(|s| parse_enable(s)).collect();
            m.features.insert(name.clone(), FeatureInfo { enables, implicit: false });
        }
        // S3: default
        if let Some(def) = &spec.default {
            let enables = def.iter().map(|s| parse_enable(s)).collect();
            m.features.insert("default".into(), FeatureInfo { enables, implicit: false });
        }
        // S4
        recompute_implicit(&mut m);
        m
    }
}

fn default_list(m: &FeatureModel) -> Vec<String> {
    m.features.get("default").map(|i| i.enables.iter().map(render_enable).collect()).unwrap_or_default()
}

/// Rule S5: edits that turn `current` into `desired`, in application order.
pub fn manifest_edits(current: &FeatureModel, desired: &FeatureModel) -> Vec<ManifestEdit> {
    let mut out = Vec::new();

    let mut seen = BTreeSet::new();
    for d in desired.dependencies.iter().filter(|d| d.optional) {
        if !seen.insert(d.name.clone()) {
            continue;
        }
        let already = current.dependencies.iter().any(|c| c.name == d.name && c.optional);
        if !already {
            out.push(ManifestEdit::MakeOptional { dep: d.name.clone() });
        }
    }

    for (name, info) in &desired.features {
        if info.implicit || name == "default" {
            continue;
        }
        let same = current.features.get(name).is_some_and(|c| c.enables == info.enables);
        if !same {
            out.push(ManifestEdit::SetFeature { name: name.clone(), enables: info.enables.iter().map(render_enable).collect() });
        }
    }

    let (cur_def, want_def) = (default_list(current), default_list(desired));
    if cur_def != want_def {
        out.push(ManifestEdit::SetDefault { features: want_def });
    }

    out
}
