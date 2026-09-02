use crate::{error::{PassengerError, Result}, model::ManifestInfo};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;

fn norm_dep_key(dep: &str) -> String {
    dep.replace('-', "_")
}

fn collect_deps_from_tablelike(
    tbl: &dyn toml_edit::TableLike,
    all_deps: &mut BTreeSet<String>,
    optional_deps: &mut BTreeSet<String>,
) {
    for (k, v) in tbl.iter() {
        let key = norm_dep_key(k);
        all_deps.insert(key.clone());

        // dep value can be "1.0" OR { version="1", optional=true } OR table form
        let is_optional = v
            .as_table_like()
            .and_then(|t| t.get("optional"))
            .and_then(|x| x.as_bool())
            .unwrap_or(false);

        if is_optional {
            optional_deps.insert(key);
        }
    }
}

pub fn load_manifest(path: &str) -> Result<ManifestInfo> {
    let raw = fs::read_to_string(path)?;
    let doc = raw.parse::<toml_edit::DocumentMut>()
        .map_err(|e| PassengerError::Toml(e.to_string()))?;

    let crate_name = doc
        .get("package")
        .and_then(|t| t.get("name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or(PassengerError::MissingPackageName)?;

    let mut all_deps = BTreeSet::<String>::new();
    let mut optional_deps = BTreeSet::<String>::new();

    // plain deps tables
    for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(tbl) = doc.get(key).and_then(|t| t.as_table_like()) {
            collect_deps_from_tablelike(tbl, &mut all_deps, &mut optional_deps);
        }
    }

    // target-specific deps: [target.'cfg(...)'.dependencies] etc
    if let Some(targets) = doc.get("target").and_then(|t| t.as_table_like()) {
        for (_tname, titem) in targets.iter() {
            if let Some(ttbl) = titem.as_table_like() {
                for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
                    if let Some(tbl) = ttbl.get(key).and_then(|x| x.as_table_like()) {
                        collect_deps_from_tablelike(tbl, &mut all_deps, &mut optional_deps);
                    }
                }
            }
        }
    }

    // features (same as you already do)
    let mut features_raw: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if let Some(feats) = doc.get("features").and_then(|t| t.as_table_like()) {
        for (fname, v) in feats.iter() {
            let mut members = vec![];
            if let Some(arr) = v.as_array() {
                for item in arr.iter() {
                    if let Some(s) = item.as_str() {
                        members.push(s.to_string());
                    }
                }
            }
            features_raw.insert(fname.to_string(), members);
        }
    }

    // feature -> deps it enables (keep your exact logic, but it references optional_deps)
    let mut feature_deps: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (fname, members) in &features_raw {
        let mut deps_for_feature = BTreeSet::<String>::new();
        for m in members {
            if let Some(dep) = m.strip_prefix("dep:") {
                let dep = norm_dep_key(dep.trim());
                if optional_deps.contains(&dep) {
                    deps_for_feature.insert(dep);
                }
                continue;
            }

            if let Some((left, _right)) = m.split_once('/') {
                let left = norm_dep_key(left.trim());
                if optional_deps.contains(&left) {
                    deps_for_feature.insert(left);
                }
                continue;
            }

            let mnorm = norm_dep_key(m.trim());
            if optional_deps.contains(&mnorm) {
                deps_for_feature.insert(mnorm);
            }
        }
        feature_deps.insert(fname.clone(), deps_for_feature);
    }

    let mut dep_features: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (feat, deps) in &feature_deps {
        for dep in deps {
            dep_features.entry(dep.clone()).or_default().insert(feat.clone());
        }
    }

    Ok(ManifestInfo {
        crate_name,
        features_raw,
        all_deps,
        optional_deps,
        feature_deps,
        dep_features,
    })
}
