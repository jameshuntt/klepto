use crate::extract::*;
use crate::model::*;
use crate::query::*;
#[allow(unused)]use crate::find::*;
use crate::snapshot::*;
use crate::rules::*;

use cargo_metadata::MetadataCommand;
use globset::{Glob, GlobSet};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use thiserror::Error;
use walkdir::WalkDir;

#[derive(Debug, Error)]
pub enum KleptoError {
    #[error("io error reading {path}: {source}")]
    Io { path: PathBuf, #[source] source: std::io::Error },

    #[error("syn parse error in {path}: {source}")]
    Parse { path: PathBuf, #[source] source: syn::Error },

    #[error("glob error: {0}")]
    Glob(#[from] globset::Error),

    #[error("cargo metadata error: {0}")]
    CargoMeta(#[from] cargo_metadata::Error),
}

#[derive(Debug, Clone)]
pub struct ParsedFile {
    pub path: PathBuf,
    pub modified: SystemTime,
    pub source: String,
    pub ast: syn::File,
    pub is_no_std_crate_root: bool,
}

#[derive(Debug, Clone)]
pub struct Klepto {
    pub crate_name: String,
    pub files: Vec<ParsedFile>,

    // extracted caches (so queries are fast)
    pub functions: Vec<CapturedFn>,
    pub imports: Vec<StolenPath>,
    pub exports: Vec<ExportedSymbol>,

    pub macros_def: Vec<MacroDef>,
    pub macros_inv: Vec<MacroInvocation>,
    pub paths: Vec<PathOccurrence>,
    pub calls: Vec<CallOccurrence>,

    pub no_std_detected: bool,

    pub index: crate::index::EnclosingIndex,

}

impl Klepto {
    pub fn new(crate_name: impl Into<String>) -> KleptoBuilder {
        KleptoBuilder::new(crate_name)
    }

    // Queries
    pub fn functions(&self) -> FnQuery<'_> { FnQuery::new(self) }
    pub fn imports(&self) -> ImportQuery<'_> { ImportQuery::new(self) }

    // Presets
    pub fn public_api(&self) -> FnQuery<'_> { self.functions().public_only() }
    pub fn undocumented_public_api(&self) -> FnQuery<'_> { self.functions().public_only().no_docs() }

    // Public surface (pub use)
    pub fn public_surface(&self) -> PublicSurface { PublicSurface { exports: self.exports.clone() } }

    // Finders (fast, uses cached occurrences)
    pub fn find_paths(&self, needle: &str) -> Vec<PathOccurrence> {
        self.paths.iter().cloned().filter(|p| p.path == needle).collect()
    }

    pub fn find_macro_invocations(&self, name: &str) -> Vec<MacroInvocation> {
        self.macros_inv.iter().cloned().filter(|m| m.name == name).collect()
    }

    pub fn find_calls(&self, callee_contains: &str) -> Vec<CallOccurrence> {
        self.calls.iter().cloned().filter(|c| c.callee.contains(callee_contains)).collect()
    }

    pub fn doc_coverage(&self) -> DocCoverage {
        let public_total = self.functions.iter().filter(|f| f.is_public).count();
        let public_documented = self.functions.iter().filter(|f| f.is_public && f.has_docs).count();
        let percent = if public_total == 0 { 100.0 } else { (public_documented as f64) * 100.0 / (public_total as f64) };
        DocCoverage { public_total, public_documented, percent }
    }

    // Snapshot / diff
    pub fn snapshot(&self) -> Snapshot { Snapshot::from_klepto(self) }
    pub fn diff_snapshot(&self, old: &Snapshot) -> SnapshotDiff { self.snapshot().diff(old) }

    // Rules
    pub fn rules(&self) -> RuleRunner<'_> { RuleRunner::new(self) }
}



#[derive(Default)]
pub struct KleptoBuilder {
    crate_name: String,
    roots: Vec<PathBuf>,
    include: KleptoGlobSetBuilder,
    exclude: KleptoGlobSetBuilder,
    follow_links: bool,
    max_file_size: Option<u64>,
    ignore_parse_errors: bool,
    only_newest: Option<usize>,
    member_filter: Option<Vec<String>>,
    add_tests: bool,
    add_examples: bool,
    add_benches: bool,
    workspace_members: HashSet<String>,
    dependency_crates: HashSet<String>,

}

impl KleptoBuilder {
    pub fn new(crate_name: impl Into<String>) -> Self {
        let mut b = Self { crate_name: crate_name.into(), ..Default::default() };
        // default include rs
        b.include.add(Glob::new("**/*.rs").unwrap());
        b
    }

    pub fn scan_in_folder(mut self, path: impl Into<PathBuf>) -> Self {
        self.roots.push(path.into());
        self
    }

    pub fn scan_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.roots.push(path.into());
        self
    }

//     pub fn scan_workspace_root(mut self, root: impl Into<PathBuf>) -> Result<Self, KleptoError> {
//         let root = root.into();
//         let manifest = root.join("Cargo.toml");
//         let meta = MetadataCommand::new().manifest_path(manifest).exec()?;
// 
//         // choose which members
//         let mut members = Vec::new();
//         for id in &meta.workspace_members {
//             if let Some(pkg) = meta.packages.iter().find(|p| &p.id == id) {
//                 if let Some(f) = &self.member_filter {
//                     if !f.iter().any(|x| x == &pkg.name) { continue; }
//                 }
//                 members.push(pkg.manifest_path.clone().into_std_path_buf());
//             }
//         }
//         // Collect workspace member crate roots + dependency crate roots (normalized)
//         for id in &meta.workspace_members {
//             if let Some(pkg) = meta.packages.iter().find(|p| &p.id == id) {
//                 let ws_name = norm_crate_root(&pkg.name);
//                 self.workspace_members.insert(ws_name);
// 
//                 for dep in &pkg.dependencies {
//                     self.dependency_crates.insert(norm_crate_root(&dep.name));
//                 }
//             }
//         }
// 
//         for m in members {
//             if let Some(dir) = m.parent() {
//                 self.roots.push(dir.join("src"));
//                 if self.add_tests { self.roots.push(dir.join("tests")); }
//                 if self.add_examples { self.roots.push(dir.join("examples")); }
//                 if self.add_benches { self.roots.push(dir.join("benches")); }
//             }
//         }
// 
//         Ok(self)
//     }

    pub fn scan_workspace_root(mut self, root: impl Into<PathBuf>) -> Result<Self, KleptoError> {
        let root = root.into();
        let manifest = root.join("Cargo.toml");
        let meta = MetadataCommand::new().manifest_path(manifest).exec()?;

        // Choose which members + collect workspace/deps (normalized)
        let mut members: Vec<std::path::PathBuf> = Vec::new();

        for id in &meta.workspace_members {
            let Some(pkg) = meta.packages.iter().find(|p| &p.id == id) else {
                continue;
            };

            // Always collect workspace member roots + deps (regardless of member_filter)
            self.workspace_members.insert(norm_crate_root(&pkg.name));
            for dep in &pkg.dependencies {
                self.dependency_crates.insert(norm_crate_root(&dep.name));
            }

            // Optional filter: only scan selected members
            if let Some(f) = &self.member_filter {
                if !f.iter().any(|x| x == &pkg.name) {
                    continue;
                }
            }

            members.push(pkg.manifest_path.clone().into_std_path_buf());
        }

        for m in members {
            if let Some(dir) = m.parent() {
                self.roots.push(dir.join("src"));
                if self.add_tests { self.roots.push(dir.join("tests")); }
                if self.add_examples { self.roots.push(dir.join("examples")); }
                if self.add_benches { self.roots.push(dir.join("benches")); }
            }
        }

        Ok(self)
    }

    pub fn only_members(mut self, names: &[&str]) -> Self {
        self.member_filter = Some(names.iter().map(|s| s.to_string()).collect());
        self
    }

    pub fn include_glob(mut self, pat: &str) -> Result<Self, KleptoError> {
        self.include.add(Glob::new(pat)?);
        Ok(self)
    }

    pub fn exclude_glob(mut self, pat: &str) -> Result<Self, KleptoError> {
        self.exclude.add(Glob::new(pat)?);
        Ok(self)
    }

    pub fn exclude_generated(mut self) -> Result<Self, KleptoError> {
        // very common junk
        self = self.exclude_glob("**/target/**")?;
        self = self.exclude_glob("**/OUT_DIR/**")?;
        self = self.exclude_glob("**/*bindings*.rs")?;
        self = self.exclude_glob("**/*pb*.rs")?;
        Ok(self)
    }

    pub fn follow_links(mut self, yes: bool) -> Self { self.follow_links = yes; self }
    pub fn max_file_size(mut self, bytes: u64) -> Self { self.max_file_size = Some(bytes); self }
    pub fn ignore_parse_errors(mut self, yes: bool) -> Self { self.ignore_parse_errors = yes; self }
    pub fn only_newest(mut self, n: usize) -> Self { self.only_newest = Some(n); self }

    pub fn include_tests(mut self, yes: bool) -> Self { self.add_tests = yes; self }
    pub fn include_examples(mut self, yes: bool) -> Self { self.add_examples = yes; self }
    pub fn include_benches(mut self, yes: bool) -> Self { self.add_benches = yes; self }

    pub fn parse(self) -> Result<Klepto, KleptoError> {
        let include: GlobSet = self.include.build()?;
        let exclude: GlobSet = self.exclude.build()?;

        let mut candidates: Vec<(PathBuf, SystemTime)> = Vec::new();

        for root in &self.roots {
            if root.is_file() {
                push_if_match(root, &include, &exclude, &mut candidates);
                continue;
            }
            for entry in WalkDir::new(root)
                .follow_links(self.follow_links)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                let p = entry.path();
                if !p.is_file() { continue; }
                push_if_match(p, &include, &exclude, &mut candidates);
            }
        }

        candidates.sort_by_key(|(_, m)| *m);
        candidates.reverse();
        if let Some(n) = self.only_newest { candidates.truncate(n); }

        // parse files
        #[cfg(feature = "parallel")]
        let parsed: Result<Vec<ParsedFile>, KleptoError> = {
            use rayon::prelude::*;
            candidates
                .par_iter()
                .map(|(path, modified)| parse_one(path, *modified, self.max_file_size))
                .filter_map(|r| match r {
                    Ok(Some(pf)) => Some(Ok(pf)),
                    Ok(None) => None,
                    Err(e) => Some(Err(e)),
                })
                .collect()
        };

        #[cfg(not(feature = "parallel"))]
        let parsed: Result<Vec<ParsedFile>, KleptoError> = {
            let mut v = Vec::new();
            for (path, modified) in candidates {
                match parse_one(&path, modified, self.max_file_size)? {
                    Some(pf) => v.push(pf),
                    None => {}
                }
            }
            Ok(v)
        };

        let mut files = Vec::new();
        match parsed {
            Ok(v) => files = v,
            Err(e) if self.ignore_parse_errors => { /* keep empty */ }
            Err(e) => return Err(e),
        }

        // extract caches
        let mut functions = Vec::new();
        let mut imports = Vec::new();
        // for imp in &mut imports {
        //     use ImportOrigin::*;
        //     let origin = if imp.is_internal {
        //         Internal
        //     } else {
        //         match imp.root.as_str() {
        //             "std" => Std,
        //             "core" => Core,
        //             "alloc" => Alloc,
        //             r if workspace_members.contains(r) => WorkspaceMember,
        //             r if dependency_crates.contains(r) => Dependency,
        //             _ => UnknownExternal,
        //         }
        //     };
        //     imp.origin = Some(origin);
        // }
        classify_imports(&mut imports, &self.workspace_members, &self.dependency_crates);



        let mut exports = Vec::new();

        let mut macros_def = Vec::new();
        let mut macros_inv = Vec::new();
        let mut paths = Vec::new();
        let mut calls = Vec::new();

        let mut no_std_detected = false;

        for pf in &files {
            if pf.is_no_std_crate_root { no_std_detected = true; }

            functions.extend(extract_functions(&self.crate_name, &pf.path, &pf.ast));
            imports.extend(extract_imports(&pf.path, &pf.ast));
            exports.extend(extract_public_surface(&pf.path, &pf.ast));

            // let (md, mi, po, co) = extract_occurrences_v1(&pf.path, &pf.ast);
            let (md, mi, po, co) = extract_occurrences(&self.crate_name, &pf.path, &pf.ast);
            macros_def.extend(md);
            macros_inv.extend(mi);
            paths.extend(po);
            calls.extend(co);
        }

        let mut index = crate::index::EnclosingIndex::default();
        for pf in &files {
            index = index.merge(crate::index::EnclosingIndex::build(&self.crate_name, &pf.path, &pf.ast));
        }

        Ok(Klepto {
            crate_name: self.crate_name,
            files,
            functions,
            imports,
            exports,
            macros_def,
            macros_inv,
            paths,
            calls,
            no_std_detected,
            index
        })
    }
}

fn push_if_match(p: &Path, include: &GlobSet, exclude: &GlobSet, out: &mut Vec<(PathBuf, SystemTime)>) {
    if !include.is_match(p) { return; }
    if exclude.is_match(p) { return; }
    if p.extension().and_then(|e| e.to_str()) != Some("rs") { return; }
    let modified = std::fs::metadata(p).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
    out.push((p.to_path_buf(), modified));
}

fn parse_one(path: &Path, modified: SystemTime, max_size: Option<u64>) -> Result<Option<ParsedFile>, KleptoError> {
    let meta = std::fs::metadata(path).map_err(|e| KleptoError::Io { path: path.to_path_buf(), source: e })?;
    if let Some(max) = max_size {
        if meta.len() > max { return Ok(None); }
    }
    let source = std::fs::read_to_string(path).map_err(|e| KleptoError::Io { path: path.to_path_buf(), source: e })?;
    let ast = syn::parse_file(&source).map_err(|e| KleptoError::Parse { path: path.to_path_buf(), source: e })?;
    let is_no_std_crate_root = crate_is_no_std(&ast);
    Ok(Some(ParsedFile {
        path: path.to_path_buf(),
        modified,
        source,
        ast,
        is_no_std_crate_root,
    }))
}

fn norm_crate_root(s: &str) -> String {
    s.replace('-', "_")
}

fn classify_imports(
    imports: &mut [crate::model::StolenPath],
    workspace_members: &std::collections::HashSet<String>,
    dependency_crates: &std::collections::HashSet<String>,
) {
    use crate::model::ImportOrigin::*;

    for imp in imports {
        let root = norm_crate_root(&imp.root);

        let origin = if imp.is_internal {
            Internal
        } else {
            match root.as_str() {
                "std" => Std,
                "core" => Core,
                "alloc" => Alloc,
                r if workspace_members.contains(r) => WorkspaceMember,
                r if dependency_crates.contains(r) => Dependency,
                _ => UnknownExternal,
            }
        };

        imp.origin = Some(origin);
    }
}
