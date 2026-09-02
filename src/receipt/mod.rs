//! The receipt — one versioned, digested, relocatable document
//! holding everything klepto collected.
//! Consumers (`code_passenger`, the KG export, other collectors' peers)
//! read this instead of holding a `Klepto`.
//!
//! Deliberate omission: raw `PathOccurrence`s are not carried —
//! `edges`, `unresolved` and `dep_uses` are their reduced form.

pub mod kg;
pub mod pipeline;

use crate::resolve::cfg::CfgPredicate;
use crate::resolve::features::FeatureModel;
use crate::graph::{ItemEdge, UnresolvedReference};
use crate::klepto::Klepto;
use crate::model::*;
use crate::resolve::{file_packages, CtxParts};
use crate::typing::types::{ImplDecl, TypeDecl};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Bumped whenever a field changes meaning; additive fields keep it.
pub const RECEIPT_VERSION: u32 = 1;

/// One scanned source file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileRecord {
    /// Relative to the receipt's root, `/`-separated.
    pub file: String,
    pub package: String,
    /// The module the file holds, when the module tree reached it.
    pub module: Option<String>,
    /// blake3 of the file's bytes, hex.
    pub blake3: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageRecord {
    pub name: String,
    /// Declared dependency names (`-` → `_`), sorted.
    pub deps: Vec<String>,
}

/// A dependency use site with its effective cfg.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepUseRecord {
    pub package: String,
    pub site: UseSite,
    pub cfg: CfgPredicate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkippedFile {
    pub file: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub version: u32,
    pub tool: String,
    pub tool_version: String,
    pub language: String,
    pub crate_name: String,
    /// The scanned root as given (informational; may be absolute).
    pub root: Option<String>,
    pub packages: Vec<PackageRecord>,
    pub files: Vec<FileRecord>,
    pub modules: Vec<ModuleInfo>,
    pub items: Vec<ItemDecl>,
    pub edges: Vec<ItemEdge>,
    pub unresolved: Vec<UnresolvedReference>,
    pub functions: Vec<CapturedFn>,
    pub types: Vec<TypeDecl>,
    pub impls: Vec<ImplDecl>,
    pub imports: Vec<StolenPath>,
    pub exports: Vec<ExportedSymbol>,
    pub calls: Vec<CallOccurrence>,
    pub macros_def: Vec<MacroDef>,
    pub macros_inv: Vec<MacroInvocation>,
    pub attrs: Vec<AttrOccurrence>,
    pub extern_crates: Vec<ExternCrateOccurrence>,
    pub features: BTreeMap<String, FeatureModel>,
    pub dep_uses: Vec<DepUseRecord>,
    pub skipped_files: Vec<SkippedFile>,
    /// blake3 hex of the compact JSON of this receipt with `digest` empty.
    #[serde(default)]
    pub digest: String,
}

/// What changed between two receipts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReceiptDiff {
    pub added_items: Vec<ItemDecl>,
    pub removed_items: Vec<ItemDecl>,
    /// Same id, different kind / cfg / visibility / parent: `(old, new)`.
    pub changed_items: Vec<(ItemDecl, ItemDecl)>,
    pub added_edges: Vec<ItemEdge>,
    pub removed_edges: Vec<ItemEdge>,
    /// `(fq_name, old signature, new signature)`.
    pub changed_signatures: Vec<(String, String, String)>,
    pub added_files: Vec<String>,
    pub removed_files: Vec<String>,
    pub changed_files: Vec<String>,
}

/// Paths relative to the scan root, `/`-separated. The root is
/// tried as given and canonicalised (file paths may have been canonicalised
/// while scanning); a path under neither is kept as it is.
pub(crate) struct Relocator {
    roots: Vec<PathBuf>,
}

impl Relocator {
    pub(crate) fn new(root: Option<&Path>) -> Self {
        let mut roots = Vec::new();
        if let Some(r) = root {
            roots.push(r.to_path_buf());
            if let Ok(c) = std::fs::canonicalize(r) {
                if c != r {
                    roots.push(c);
                }
            }
        }
        Self { roots }
    }

    pub(crate) fn rel(&self, p: &Path) -> String {
        let rel = self.roots.iter().find_map(|r| p.strip_prefix(r).ok()).unwrap_or(p);
        rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
    }

    pub(crate) fn path(&self, p: &Path) -> PathBuf {
        PathBuf::from(self.rel(p))
    }

    pub(crate) fn loc(&self, l: &FileLocation) -> FileLocation {
        FileLocation { path: self.path(&l.path), line: l.line, column: l.column }
    }
}

type LocKey = (String, Option<u32>, Option<u32>);

fn loc_key(l: &FileLocation) -> LocKey {
    (l.path.to_string_lossy().into_owned(), l.line, l.column)
}

fn edge_key(e: &ItemEdge) -> (String, String, String, String, Option<u32>, Option<u32>) {
    (
        e.from.clone(),
        e.to.clone(),
        format!("{:?}", e.kind),
        e.location.path.to_string_lossy().into_owned(),
        e.location.line,
        e.location.column,
    )
}

/// Every scanned package name: the files' packages plus anything the
/// manifest data named.
pub(crate) fn scanned_packages(k: &Klepto) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = file_packages(k).values().cloned().collect();
    names.extend(k.package_deps.keys().cloned());
    names.extend(k.features.keys().cloned());
    names
}

impl Receipt {
    /// Builds the receipt from a scan: every collected fact with paths made
    /// relative to the scan root, then digested.
    pub fn from_klepto(k: &Klepto) -> Receipt {
        let rl = Relocator::new(k.root.as_deref());
        let parts = CtxParts::new(k);

        let packages: Vec<PackageRecord> = scanned_packages(k)
            .into_iter()
            .map(|name| PackageRecord {
                deps: k.package_deps.get(&name).map(|d| d.iter().cloned().collect()).unwrap_or_default(),
                name,
            })
            .collect();

        let mut files: Vec<FileRecord> = k
            .files
            .iter()
            .map(|pf| FileRecord {
                file: rl.rel(&pf.path),
                package: parts.package_of(&pf.path),
                module: pf.module.clone(),
                blake3: blake3::hash(pf.source.as_bytes()).to_hex().to_string(),
                bytes: pf.source.len() as u64,
            })
            .collect();
        files.sort_by(|a, b| a.file.cmp(&b.file));

        let mut modules: Vec<ModuleInfo> = k
            .modules
            .modules
            .values()
            .map(|m| {
                let mut m = m.clone();
                m.file = rl.path(&m.file);
                m.declared_in = m.declared_in.as_ref().map(|p| rl.path(p));
                m.alternates = m.alternates.iter().map(|p| rl.path(p)).collect();
                m
            })
            .collect();
        modules.sort_by(|a, b| a.path.cmp(&b.path));

        let mut items: Vec<ItemDecl> = k.items.iter().map(|i| ItemDecl { location: rl.loc(&i.location), ..i.clone() }).collect();
        items.sort_by(|a, b| (&a.id, loc_key(&a.location)).cmp(&(&b.id, loc_key(&b.location))));

        let mut edges: Vec<ItemEdge> = k.graph.edges.iter().map(|e| ItemEdge { location: rl.loc(&e.location), ..e.clone() }).collect();
        edges.sort_by(|a, b| {
            (&a.from, &a.to, a.kind, loc_key(&a.location), a.cfg.render()).cmp(&(&b.from, &b.to, b.kind, loc_key(&b.location), b.cfg.render()))
        });

        let mut unresolved: Vec<UnresolvedReference> =
            k.graph.unresolved.iter().map(|u| UnresolvedReference { location: rl.loc(&u.location), ..u.clone() }).collect();
        unresolved.sort_by(|a, b| (&a.from, &a.raw, loc_key(&a.location)).cmp(&(&b.from, &b.raw, loc_key(&b.location))));

        let mut functions: Vec<CapturedFn> = k.functions.iter().map(|f| CapturedFn { location: rl.loc(&f.location), ..f.clone() }).collect();
        functions.sort_by(|a, b| (&a.fq_name, loc_key(&a.location)).cmp(&(&b.fq_name, loc_key(&b.location))));

        let mut types: Vec<TypeDecl> = k.types.iter().map(|t| TypeDecl { location: rl.loc(&t.location), ..t.clone() }).collect();
        types.sort_by(|a, b| a.id.cmp(&b.id));
        let mut impls: Vec<ImplDecl> = k.impls.iter().map(|i| ImplDecl { location: rl.loc(&i.location), ..i.clone() }).collect();
        impls.sort_by(|a, b| a.id.cmp(&b.id));

        let mut imports: Vec<StolenPath> = k.imports.iter().map(|i| StolenPath { location: rl.loc(&i.location), ..i.clone() }).collect();
        imports.sort_by(|a, b| (loc_key(&a.location), &a.full_path).cmp(&(loc_key(&b.location), &b.full_path)));
        let mut exports: Vec<ExportedSymbol> = k.exports.iter().map(|e| ExportedSymbol { location: rl.loc(&e.location), ..e.clone() }).collect();
        exports.sort_by(|a, b| (loc_key(&a.location), &a.exported_as).cmp(&(loc_key(&b.location), &b.exported_as)));
        let mut calls: Vec<CallOccurrence> = k.calls.iter().map(|c| CallOccurrence { location: rl.loc(&c.location), ..c.clone() }).collect();
        calls.sort_by(|a, b| (loc_key(&a.location), &a.callee).cmp(&(loc_key(&b.location), &b.callee)));
        let mut macros_def: Vec<MacroDef> = k.macros_def.iter().map(|m| MacroDef { location: rl.loc(&m.location), ..m.clone() }).collect();
        macros_def.sort_by(|a, b| (loc_key(&a.location), &a.name).cmp(&(loc_key(&b.location), &b.name)));
        let mut macros_inv: Vec<MacroInvocation> = k.macros_inv.iter().map(|m| MacroInvocation { location: rl.loc(&m.location), ..m.clone() }).collect();
        macros_inv.sort_by(|a, b| (loc_key(&a.location), &a.name).cmp(&(loc_key(&b.location), &b.name)));
        let mut attrs: Vec<AttrOccurrence> = k.attrs.iter().map(|a| AttrOccurrence { location: rl.loc(&a.location), ..a.clone() }).collect();
        attrs.sort_by(|a, b| (loc_key(&a.location), &a.path).cmp(&(loc_key(&b.location), &b.path)));
        let mut extern_crates: Vec<ExternCrateOccurrence> =
            k.extern_crates.iter().map(|e| ExternCrateOccurrence { location: rl.loc(&e.location), ..e.clone() }).collect();
        extern_crates.sort_by(|a, b| (loc_key(&a.location), &a.name).cmp(&(loc_key(&b.location), &b.name)));

        // Dependency use sites of every package, tagged with the package
        let all_deps: BTreeSet<String> = k.package_deps.values().flat_map(|d| d.iter().cloned()).collect();
        let mut dep_uses: Vec<DepUseRecord> = k
            .dep_use_sites_with_cfg(&all_deps)
            .into_iter()
            .filter_map(|(site, cfg)| {
                let package = parts.package_of(&site.location.path);
                if !k.package_deps.get(&package).is_some_and(|d| d.contains(&site.dep)) {
                    return None;
                }
                let site = UseSite { location: rl.loc(&site.location), ..site };
                Some(DepUseRecord { package, site, cfg })
            })
            .collect();
        dep_uses.sort_by(|a, b| {
            (&a.package, loc_key(&a.site.location), &a.site.path).cmp(&(&b.package, loc_key(&b.site.location), &b.site.path))
        });

        let mut skipped_files: Vec<SkippedFile> =
            k.skipped_files.iter().map(|(p, e)| SkippedFile { file: rl.rel(p), error: e.clone() }).collect();
        skipped_files.sort_by(|a, b| a.file.cmp(&b.file));

        let mut r = Receipt {
            version: RECEIPT_VERSION,
            tool: "klepto".into(),
            tool_version: env!("CARGO_PKG_VERSION").into(),
            language: "rust".into(),
            crate_name: k.crate_name.clone(),
            root: k.root.as_ref().map(|p| p.to_string_lossy().into_owned()),
            packages,
            files,
            modules,
            items,
            edges,
            unresolved,
            functions,
            types,
            impls,
            imports,
            exports,
            calls,
            macros_def,
            macros_inv,
            attrs,
            extern_crates,
            features: k.features.clone(),
            dep_uses,
            skipped_files,
            digest: String::new(),
        };
        r.digest = r.compute_digest();
        r
    }

    /// Blake3 over the compact JSON with `digest` cleared.
    pub fn compute_digest(&self) -> String {
        let mut copy = self.clone();
        copy.digest = String::new();
        let bytes = serde_json::to_vec(&copy).unwrap_or_default();
        blake3::hash(&bytes).to_hex().to_string()
    }

    pub fn verify_digest(&self) -> bool {
        !self.digest.is_empty() && self.digest == self.compute_digest()
    }

    pub fn to_json_string(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn to_json_compact(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn from_json_str(s: &str) -> Result<Receipt, serde_json::Error> {
        serde_json::from_str(s)
    }

    /// Files added, removed or whose bytes changed since `old`, sorted.
    pub fn stale_files(&self, old: &Receipt) -> Vec<String> {
        let old_map: BTreeMap<&str, &str> = old.files.iter().map(|f| (f.file.as_str(), f.blake3.as_str())).collect();
        let new_map: BTreeMap<&str, &str> = self.files.iter().map(|f| (f.file.as_str(), f.blake3.as_str())).collect();
        let mut out: BTreeSet<String> = BTreeSet::new();
        for (f, h) in &new_map {
            if old_map.get(f) != Some(h) {
                out.insert((*f).to_string());
            }
        }
        for f in old_map.keys() {
            if !new_map.contains_key(f) {
                out.insert((*f).to_string());
            }
        }
        out.into_iter().collect()
    }

    /// What changed since `old`: items and files added, removed or altered.
    pub fn diff(&self, old: &Receipt) -> ReceiptDiff {
        let mut d = ReceiptDiff::default();

        let old_items: BTreeMap<&str, &ItemDecl> = old.items.iter().map(|i| (i.id.as_str(), i)).collect();
        let new_items: BTreeMap<&str, &ItemDecl> = self.items.iter().map(|i| (i.id.as_str(), i)).collect();
        for (id, n) in &new_items {
            match old_items.get(id) {
                None => d.added_items.push((*n).clone()),
                Some(o) => {
                    if o.kind != n.kind || o.cfg != n.cfg || o.is_public != n.is_public || o.parent != n.parent {
                        d.changed_items.push(((*o).clone(), (*n).clone()));
                    }
                }
            }
        }
        for (id, o) in &old_items {
            if !new_items.contains_key(id) {
                d.removed_items.push((*o).clone());
            }
        }

        let old_edges: BTreeSet<_> = old.edges.iter().map(edge_key).collect();
        let new_edges: BTreeSet<_> = self.edges.iter().map(edge_key).collect();
        d.added_edges = self.edges.iter().filter(|e| !old_edges.contains(&edge_key(e))).cloned().collect();
        d.removed_edges = old.edges.iter().filter(|e| !new_edges.contains(&edge_key(e))).cloned().collect();

        let old_fns: BTreeMap<&str, &str> = old.functions.iter().map(|f| (f.fq_name.as_str(), f.signature.as_str())).collect();
        for f in &self.functions {
            if let Some(os) = old_fns.get(f.fq_name.as_str()) {
                if *os != f.signature {
                    d.changed_signatures.push((f.fq_name.clone(), (*os).to_string(), f.signature.clone()));
                }
            }
        }

        let old_files: BTreeMap<&str, &str> = old.files.iter().map(|f| (f.file.as_str(), f.blake3.as_str())).collect();
        let new_files: BTreeMap<&str, &str> = self.files.iter().map(|f| (f.file.as_str(), f.blake3.as_str())).collect();
        for (f, h) in &new_files {
            match old_files.get(f) {
                None => d.added_files.push((*f).to_string()),
                Some(oh) if oh != h => d.changed_files.push((*f).to_string()),
                _ => {}
            }
        }
        for f in old_files.keys() {
            if !new_files.contains_key(f) {
                d.removed_files.push((*f).to_string());
            }
        }
        d
    }
}
