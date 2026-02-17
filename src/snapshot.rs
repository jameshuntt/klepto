use crate::model::*;
use crate::klepto::Klepto;
use blake3::Hasher;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FnFinger {
    pub fq_name: String,
    pub sig_hash: String,
    pub signature: String,
    pub location: FileLocation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportFinger {
    pub exported_as: String,
    pub source_path: String,
    pub location: FileLocation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub crate_name: String,
    pub no_std: bool,
    pub functions: Vec<FnFinger>,
    pub exports: Vec<ExportFinger>,
    pub imports: Vec<String>, // full paths
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotDiff {
    pub added_functions: Vec<FnFinger>,
    pub removed_functions: Vec<FnFinger>,
    pub changed_signatures: Vec<(FnFinger, FnFinger)>, // (old,new)

    pub added_exports: Vec<ExportFinger>,
    pub removed_exports: Vec<ExportFinger>,

    pub added_imports: Vec<String>,
    pub removed_imports: Vec<String>,
}

fn hash_sig(s: &str) -> String {
    let mut h = Hasher::new();
    h.update(s.as_bytes());
    h.finalize().to_hex().to_string()
}

impl Snapshot {
    pub fn from_klepto(k: &Klepto) -> Self {
        let functions = k.functions.iter().map(|f| FnFinger {
            fq_name: f.fq_name.clone(),
            sig_hash: hash_sig(&f.signature),
            signature: f.signature.clone(),
            location: f.location.clone(),
        }).collect();

        let exports = k.exports.iter().map(|e| ExportFinger {
            exported_as: e.exported_as.clone(),
            source_path: e.source_path.clone(),
            location: e.location.clone(),
        }).collect();

        let imports = {
            let mut v: Vec<String> = k.imports.iter().map(|i| i.full_path.clone()).collect();
            v.sort();
            v.dedup();
            v
        };

        Snapshot { crate_name: k.crate_name.clone(), no_std: k.no_std_detected, functions, exports, imports }
    }

    pub fn to_json_string(&self) -> String {
        serde_json::to_string_pretty(self).unwrap()
    }

    pub fn diff(&self, old: &Snapshot) -> SnapshotDiff {
        let mut old_map: BTreeMap<String, &FnFinger> = BTreeMap::new();
        for f in &old.functions { old_map.insert(f.fq_name.clone(), f); }

        let mut new_map: BTreeMap<String, &FnFinger> = BTreeMap::new();
        for f in &self.functions { new_map.insert(f.fq_name.clone(), f); }

        let mut added_functions = Vec::new();
        let mut removed_functions = Vec::new();
        let mut changed_signatures = Vec::new();

        for (k, nf) in &new_map {
            match old_map.get(k) {
                None => added_functions.push((*nf).clone()),
                Some(of) => {
                    if of.sig_hash != nf.sig_hash {
                        changed_signatures.push(((*of).clone(), (*nf).clone()));
                    }
                }
            }
        }
        for (k, of) in &old_map {
            if !new_map.contains_key(k) {
                removed_functions.push((*of).clone());
            }
        }

        let old_exports: BTreeSet<(String,String)> = old.exports.iter().map(|e| (e.exported_as.clone(), e.source_path.clone())).collect();
        let new_exports: BTreeSet<(String,String)> = self.exports.iter().map(|e| (e.exported_as.clone(), e.source_path.clone())).collect();

        let mut added_exports = Vec::new();
        let mut removed_exports = Vec::new();

        for e in &self.exports {
            if !old_exports.contains(&(e.exported_as.clone(), e.source_path.clone())) {
                added_exports.push(e.clone());
            }
        }
        for e in &old.exports {
            if !new_exports.contains(&(e.exported_as.clone(), e.source_path.clone())) {
                removed_exports.push(e.clone());
            }
        }

        let old_imports: BTreeSet<String> = old.imports.iter().cloned().collect();
        let new_imports: BTreeSet<String> = self.imports.iter().cloned().collect();

        let added_imports = new_imports.difference(&old_imports).cloned().collect();
        let removed_imports = old_imports.difference(&new_imports).cloned().collect();

        SnapshotDiff {
            added_functions,
            removed_functions,
            changed_signatures,
            added_exports,
            removed_exports,
            added_imports,
            removed_imports,
        }
    }
}
