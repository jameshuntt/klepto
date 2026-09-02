use crate::{
    cargo::load_manifest,
    error::{PassengerError, Result},
    model::{DocumentDetails, FileReport, ManifestInfo, UsedSymbols},
    packs,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
use klepto::Klepto;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

fn rel_to_src(src_root: &Path, p: &Path) -> Result<String> {
    let rel = p.strip_prefix(src_root)
        .map_err(|e| PassengerError::Path(e.to_string()))?;
    Ok(rel.to_string_lossy().to_string())
}

// very pragmatic “scope notes”
pub(crate)fn scope_label_from_header(header: &str) -> Option<String> {
    let h = header.trim();

    if let Some(idx) = h.find("fn ") {
        let after = &h[idx + 3..];
        let name: String = after
            .chars()
            .skip_while(|c| c.is_whitespace())
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() { return Some(format!("fn {name}")); }
    }

    if h.contains("impl ") {
        let preview = h.replace('\n', " ");
        let preview = preview.split('{').next().unwrap_or(&preview);
        let preview = preview.split(" where ").next().unwrap_or(preview);
        let preview = preview.trim();
        if let Some(rest) = preview.strip_prefix("impl ") {
            return Some(format!("impl {}", rest.trim()));
        }
        return Some(format!("impl {}", preview));
    }

    for kw in ["mod", "trait", "struct", "enum"] {
        if let Some(idx) = h.find(&format!("{kw} ")) {
            let after = &h[idx + kw.len() + 1..];
            let name: String = after
                .chars()
                .skip_while(|c| c.is_whitespace())
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() { return Some(format!("{kw} {name}")); }
        }
    }

    None
}

pub struct RunContext {
    pub root: PathBuf,
    pub src_rel: String,
    pub manifest_path: PathBuf,
    pub lang: String,
}
#[derive(Serialize, Deserialize)]
pub struct EngineOutput {
    pub manifest: ManifestInfo,
    pub reports: Vec<FileReport>,
}

fn module_prefix(path: &str) -> Option<String> {
    let mut it = path.split("::");
    let a = it.next()?;
    let b = it.next()?;
    Some(format!("{a}::{b}"))
}

fn used_external_from_sites(sites: &[klepto::UseSite]) -> BTreeSet<String> {
    sites.iter().map(|u| u.dep.clone()).collect()
}

fn used_mods_from_internal_sites(sites: &[klepto::UseSite]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for u in sites {
        if let Some(m) = module_prefix(&u.path) {
            out.insert(m);
        } else {
            out.insert(u.dep.clone()); // fallback
        }
    }
    out
}

fn canon(p: &std::path::Path) -> std::path::PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

pub fn run_scan(ctx: &RunContext) -> Result<EngineOutput> {
    let manifest = load_manifest(ctx.manifest_path.to_str().ok_or_else(|| PassengerError::Path("bad manifest path".into()))?)?;
    let src_root = ctx.root.join(&ctx.src_rel);

    // 1) Build Klepto ONCE
    let k = Klepto::new(&manifest.crate_name)
        .scan_workspace_root(&ctx.root)     // <- use the real root, not "."
        .map_err(|e| PassengerError::Unsupported(e.to_string()))?
        .exclude_generated()
        .map_err(|e| PassengerError::Unsupported(e.to_string()))?
        .parse()
        .map_err(|e| PassengerError::Unsupported(e.to_string()))?;

    // 2) Choose deps you care about (better: from cargo_metadata)
    let used_deps: BTreeSet<String> = manifest.all_deps.iter().cloned().collect();

    // 3) Build per-file indices
    let mut ext_by_file: BTreeMap<std::path::PathBuf, Vec<klepto::UseSite>> = BTreeMap::new();
    for u in k.dep_use_sites(&used_deps) {
        ext_by_file.entry(canon(&u.location.path)).or_default().push(u.clone());
    }

    let mut int_by_file: BTreeMap<std::path::PathBuf, Vec<klepto::UseSite>> = BTreeMap::new();
    for u in k.internal_use_sites() {
        int_by_file.entry(canon(&u.location.path)).or_default().push(u.clone());
    }

    // 4) Now your existing WalkDir loop becomes “attach facts to this file”
    let mut reports = Vec::new();
    let pack = packs::get_pack(&ctx.lang)?;

    for ent in WalkDir::new(&src_root).into_iter().filter_map(|e| e.ok()) {
        if !ent.file_type().is_file() { continue; }
        let path = ent.path();
        if !pack.matches_path(path) { continue; }

        let rel = rel_to_src(&src_root, path)?;
        let filename = path.file_name().unwrap().to_string_lossy().to_string();
        let abs = canon(path);

        let external_use_sites = ext_by_file.get(&abs).cloned().unwrap_or_default();
        let internal_use_sites = int_by_file.get(&abs).cloned().unwrap_or_default();

        // counts per file (now they WILL differ per file)
        let external_dep_symbol_counts = count_use_sites(&external_use_sites);
        let internal_dep_symbol_counts = count_use_sites(&internal_use_sites);

        // NEW: derive used symbols from klepto slices (so lists and counts agree)
        let used_external = used_external_from_sites(&external_use_sites);
        let used_mods = used_mods_from_internal_sites(&internal_use_sites);

        let used = UsedSymbols { packages: used_external, modules: used_mods };

        // ... your used symbols + notes/corpus logic ...
        reports.push(FileReport {
            document: DocumentDetails { filename, relative_path: rel },
            used,// pack.detect_usage(&content),
            notes: vec![],
            corpus_features: vec![],
            internal_use_sites,
            internal_dep_symbol_counts,
            external_use_sites,
            external_dep_symbol_counts,
        });
    }

    Ok(EngineOutput { manifest, reports })
}

fn count_use_sites(sites: &[klepto::UseSite]) -> BTreeMap<String, BTreeMap<String, usize>> {
    let mut out = BTreeMap::<String, BTreeMap<String, usize>>::new();
    for u in sites {
        out.entry(u.dep.clone())
            .or_default()
            .entry(u.head.clone())
            .and_modify(|c| *c += 1)
            .or_insert(1);
    }
    out
}

