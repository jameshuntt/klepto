use crate::resolve::cfg::CfgPredicate;
use crate::syntax::extract::*;
use crate::resolve::features::FeatureModel;
use crate::syntax::find::Finder;
use crate::graph::ItemGraph;
use crate::model::*;
use crate::syntax::query::*;
use crate::resolve::PackageRoot;
use crate::rules::*;
use crate::syntax::snapshot::*;

use cargo_metadata::MetadataCommand;
use globset::{Glob, GlobBuilder, GlobSet};
use std::collections::{BTreeMap, BTreeSet, HashSet};
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

/// A parsed file. Its serde form holds everything but the syntax tree: syn's trees have no serde form, and klepto's
/// tree is the expanded one (macros' items spliced in), which only a parse makes. Read back, `ast` is empty.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ParsedFile {
    pub path: PathBuf,
    pub modified: SystemTime,
    pub source: String,
    #[serde(skip, default = "empty_file")]
    pub ast: syn::File,
    pub is_no_std_crate_root: bool,
    /// Canonical path of the module this file holds (`app::query`), filled
    /// by the module tree.
    pub module: Option<String>,
}

/// An empty syntax tree: what a [`ParsedFile`] read back from its serde form holds.
fn empty_file() -> syn::File {
    syn::File { shebang: None, attrs: Vec::new(), items: Vec::new() }
}

/// A parsed workspace. Its serde form is the whole model (every record, the module tree, the graph, the feature
/// models, the packages, the enclosing-function index) and each file's source, without the syntax trees: read back,
/// every record answers as the parse that wrote it did, with no cargo metadata, resolution or macro expansion run
/// again; a reader that walks the trees (`ParsedFile::ast`, empty when read back) parses. A consumer keys a stored
/// model by the code it was parsed from (`fingerprint`) and by [`Klepto::MODEL_VERSION`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Klepto {
    pub crate_name: String,
    /// The scanned root (`scan_workspace_root` / `scan_in_folder`), used by
    /// the receipt to make every path relative; `None` for `scan_file`.
    pub root: Option<PathBuf>,
    pub files: Vec<ParsedFile>,

    // extracted caches (so queries are fast)
    pub functions: Vec<CapturedFn>,
    pub imports: Vec<StolenPath>,
    pub exports: Vec<ExportedSymbol>,

    pub macros_def: Vec<MacroDef>,
    pub macros_inv: Vec<MacroInvocation>,
    pub paths: Vec<PathOccurrence>,
    pub calls: Vec<CallOccurrence>,
    pub attrs: Vec<AttrOccurrence>,
    pub extern_crates: Vec<ExternCrateOccurrence>,
    /// Every item declaration of every kind.
    pub items: Vec<ItemDecl>,
    /// Type records.
    pub types: Vec<crate::typing::types::TypeDecl>,
    /// Impl records.
    pub impls: Vec<crate::typing::types::ImplDecl>,

    /// Module tree of every scanned package.
    pub modules: ModuleTree,
    /// Item graph, built after resolution.
    pub graph: ItemGraph,
    /// Declared dependencies per package (names with `-` → `_`), from cargo
    /// metadata; empty for `scan_in_folder` / `scan_file`.
    pub package_deps: BTreeMap<String, BTreeSet<String>>,
    /// Cargo feature model per scanned package; empty for
    /// `scan_in_folder` / `scan_file`.
    pub features: BTreeMap<String, FeatureModel>,
    /// Every workspace member with its manifest and targets (the member
    /// filter does not apply); empty for `scan_in_folder` / `scan_file`.
    pub packages: BTreeMap<String, PackageInfo>,
    /// Crate-root name → its package: a bin named after its target (`appd`)
    /// is a root of its own whose items are `appd::...`, and its package
    /// (`app`) is what its dependencies and features are declared on.
    pub crate_roots: BTreeMap<String, String>,

    pub no_std_detected: bool,

    /// Every macro invocation met while expanding, with its outcome
    ///; nothing is dropped silently.
    pub expansions: Vec<crate::resolve::expand::Expansion>,
    /// `macro_rules!` names klepto could expand, with their crate.
    pub macros_known: Vec<(String, String)>,
    /// Why dependency macros were not (fully) harvested, if so.
    pub dep_macro_note: Option<String>,
    /// The features each dependency that is not a member declares (its package name, `-` → `_`), from the same
    /// frozen `cargo metadata` that finds its macros: what a `dep/feat` entry can name. Empty when that failed.
    #[serde(default)]
    pub dependency_features: BTreeMap<String, BTreeSet<String>>,

    pub index: crate::syntax::index::EnclosingIndex,

    /// Files that failed to parse and were skipped because
    /// `ignore_parse_errors(true)` was set, with the parser's message.
    pub skipped_files: Vec<(PathBuf, String)>,

    /// String literals in code, each with the call it is an argument of.
    pub literals: Vec<LiteralOccurrence>,
    /// `match` expressions with their arms' pattern paths, resolved.
    pub matches: Vec<MatchOccurrence>,

    /// Single-identifier paths waiting for the resolver; drained by the resolver.
    #[serde(skip)]
    pub(crate) pending_singles: Vec<PathOccurrence>,
}

impl Klepto {
    /// The version of the serde form: a stored model written by another klepto is parsed again, never read.
    pub const MODEL_VERSION: &'static str = concat!("klepto-model-", env!("CARGO_PKG_VERSION"));

    #[allow(clippy::new_ret_no_self)]
    pub fn new(crate_name: impl Into<String>) -> KleptoBuilder {
        KleptoBuilder::new(crate_name)
    }

    // Queries
    pub fn functions(&self) -> FnQuery<'_> { FnQuery::new(self) }
    pub fn imports(&self) -> ImportQuery<'_> { ImportQuery::new(self) }
    pub fn finder(&self) -> Finder<'_> { Finder::new(self) }

    // Presets
    pub fn public_api(&self) -> FnQuery<'_> { self.functions().public_only() }
    pub fn undocumented_public_api(&self) -> FnQuery<'_> { self.functions().public_only().no_docs() }

    // Public surface (pub use)
    pub fn public_surface(&self) -> PublicSurface { PublicSurface { exports: self.exports.clone() } }

    // Finders (fast, uses cached occurrences)
    pub fn find_paths(&self, needle: &str) -> Vec<PathOccurrence> {
        self.paths.iter().filter(|p| p.path == needle).cloned().collect()
    }

    pub fn find_macro_invocations(&self, name: &str) -> Vec<MacroInvocation> {
        self.macros_inv.iter().filter(|m| m.name == name).cloned().collect()
    }

    pub fn find_calls(&self, callee_contains: &str) -> Vec<CallOccurrence> {
        self.calls.iter().filter(|c| c.callee.contains(callee_contains)).cloned().collect()
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
    scan_root: Option<PathBuf>,
    /// Lib/bin target roots from cargo metadata; empty for
    /// `scan_in_folder` / `scan_file`.
    package_roots: Vec<PackageRoot>,
    /// Test, example and bench targets: crate roots when their directories
    /// are read (`include_tests` and the rest, in any order with the scan).
    extra_targets: Vec<PackageRoot>,
    /// Every member's directory, for the tests / examples / benches roots.
    member_dirs: Vec<PathBuf>,
    package_deps: BTreeMap<String, BTreeSet<String>>,
    features: BTreeMap<String, FeatureModel>,
    packages: BTreeMap<String, PackageInfo>,
    /// `src` dirs of dependencies (name, dir) for `#[macro_export]`
    /// harvesting; empty when the lockfile cannot be resolved offline.
    dep_sources: Vec<(String, PathBuf)>,
    dep_macro_note: Option<String>,
    dependency_features: BTreeMap<String, BTreeSet<String>>,
    no_dep_macros: bool,
}

/// `*` and `?` stop at `/`, so `**/*_pb.rs` matches a file name and never a
/// directory somewhere up the path.
fn glob(pat: &str) -> Result<Glob, KleptoError> {
    Ok(GlobBuilder::new(pat).literal_separator(true).build()?)
}

impl KleptoBuilder {
    pub fn new(crate_name: impl Into<String>) -> Self {
        let mut b = Self { crate_name: crate_name.into(), ..Default::default() };
        b.include.add(glob("**/*.rs").expect("static glob"));
        b
    }

    pub fn scan_in_folder(mut self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        if self.scan_root.is_none() {
            self.scan_root = Some(path.clone());
        }
        self.roots.push(path);
        self
    }

    pub fn scan_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.roots.push(path.into());
        self
    }

    /// Adds every workspace member's `src/` (plus tests/examples/benches if
    /// enabled) and records member, dependency and target information for
    /// origin classification and the module tree. Uses
    /// `cargo metadata --no-deps`: no dependency resolution, no network, no
    /// `Cargo.lock` written.
    pub fn scan_workspace_root(mut self, root: impl Into<PathBuf>) -> Result<Self, KleptoError> {
        let root = root.into();
        self.scan_root = Some(root.clone());
        let manifest = root.join("Cargo.toml");
        let meta = MetadataCommand::new().manifest_path(manifest).no_deps().exec()?;

        // Dependency sources for `#[macro_export]` macros.
        // `--frozen` so cargo never writes a lockfile into the scanned tree;
        // when that fails the reason is kept, not hidden.
        match MetadataCommand::new().manifest_path(root.join("Cargo.toml")).other_options(vec!["--frozen".to_string()]).exec() {
            Ok(full) => {
                let member_ids: HashSet<_> = full.workspace_members.iter().cloned().collect();
                for p in &full.packages {
                    if member_ids.contains(&p.id) {
                        continue;
                    }
                    if let Some(dir) = p.manifest_path.parent() {
                        self.dep_sources.push((norm_crate_root(&p.name), dir.join("src").into_std_path_buf()));
                    }
                    self.dependency_features.entry(norm_crate_root(&p.name)).or_default().extend(p.features.keys().cloned());
                }
            }
            Err(e) => self.dep_macro_note = Some(format!("dependency macros not harvested: {}", e.to_string().lines().next().unwrap_or_default())),
        }

        let mut members: Vec<PathBuf> = Vec::new();

        for id in &meta.workspace_members {
            let Some(pkg) = meta.packages.iter().find(|p| &p.id == id) else {
                continue;
            };

            // Always collect workspace member roots + deps (regardless of member_filter)
            let pkg_name = norm_crate_root(&pkg.name);
            self.workspace_members.insert(pkg_name.clone());
            let manifest_path = pkg.manifest_path.clone().into_std_path_buf();
            self.packages.insert(
                pkg_name.clone(),
                PackageInfo {
                    name: pkg_name.clone(),
                    cargo_name: pkg.name.clone(),
                    version: pkg.version.to_string(),
                    root: manifest_path.parent().map(Path::to_path_buf).unwrap_or_default(),
                    manifest_path,
                    targets: pkg
                        .targets
                        .iter()
                        .flat_map(|t| {
                            t.kind.iter().map(move |k| TargetInfo { name: t.name.clone(), kind: k.clone(), src_path: t.src_path.clone().into_std_path_buf() })
                        })
                        .collect(),
                },
            );
            let deps = self.package_deps.entry(pkg_name.clone()).or_default();
            for dep in &pkg.dependencies {
                let d = norm_crate_root(&dep.name);
                self.dependency_crates.insert(d.clone());
                deps.insert(d);
            }

            // Optional filter: only scan selected members
            if let Some(f) = &self.member_filter {
                if !f.iter().any(|x| x == &pkg.name) {
                    continue;
                }
            }

            for t in &pkg.targets {
                let kind = t.kind.iter().find(|k| {
                    matches!(k.as_str(), "lib" | "rlib" | "dylib" | "cdylib" | "staticlib" | "proc-macro" | "bin")
                });
                if let Some(kind) = kind {
                    self.package_roots.push(PackageRoot {
                        package: pkg_name.clone(),
                        target: t.name.clone(),
                        kind: kind.clone(),
                        file: t.src_path.clone().into_std_path_buf(),
                    });
                } else if let Some(kind) = t.kind.iter().find(|k| matches!(k.as_str(), "test" | "example" | "bench")) {
                    self.extra_targets.push(PackageRoot {
                        package: pkg_name.clone(),
                        target: t.name.clone(),
                        kind: kind.clone(),
                        file: t.src_path.clone().into_std_path_buf(),
                    });
                }
            }

            members.push(pkg.manifest_path.clone().into_std_path_buf());
        }

        let scanned: Vec<String> = self.package_roots.iter().map(|r| r.package.clone()).collect();
        self.features.extend(crate::resolve::features::build_feature_models(&meta, &scanned));

        for m in members {
            if let Some(dir) = m.parent() {
                self.roots.push(dir.join("src"));
                self.member_dirs.push(dir.to_path_buf());
            }
        }

        Ok(self)
    }

    /// The roots read: the members' `src/`, then `tests/`, `examples/` and
    /// `benches/` as asked (before or after the scan alike).
    fn read_roots(&self) -> Vec<PathBuf> {
        let mut roots = self.roots.clone();
        for dir in &self.member_dirs {
            if self.add_tests {
                roots.push(dir.join("tests"));
            }
            if self.add_examples {
                roots.push(dir.join("examples"));
            }
            if self.add_benches {
                roots.push(dir.join("benches"));
            }
        }
        roots
    }

    /// The crate roots: lib and bin targets, and the test / example / bench
    /// targets whose directories are read.
    fn crate_root_targets(&self) -> Vec<PackageRoot> {
        let mut roots = self.package_roots.clone();
        roots.extend(self.extra_targets.iter().filter(|t| match t.kind.as_str() {
            "test" => self.add_tests,
            "example" => self.add_examples,
            "bench" => self.add_benches,
            _ => false,
        }).cloned());
        roots
    }

    /// The workspace members `scan_workspace_root` read from cargo metadata,
    /// before anything is parsed: a consumer that needs the packages alone
    /// does not parse for them.
    pub fn packages(&self) -> &BTreeMap<String, PackageInfo> {
        &self.packages
    }

    pub fn only_members(mut self, names: &[&str]) -> Self {
        self.member_filter = Some(names.iter().map(|s| s.to_string()).collect());
        self
    }

    pub fn include_glob(mut self, pat: &str) -> Result<Self, KleptoError> {
        self.include.add(glob(pat)?);
        Ok(self)
    }

    pub fn exclude_glob(mut self, pat: &str) -> Result<Self, KleptoError> {
        self.exclude.add(glob(pat)?);
        Ok(self)
    }

    /// Skips build output and the usual generated-file names
    /// (`*.pb.rs`, `*_pb.rs`, `bindings.rs`, `*_bindings.rs`).
    pub fn exclude_generated(mut self) -> Result<Self, KleptoError> {
        self = self.exclude_glob("**/target/**")?;
        self = self.exclude_glob("**/OUT_DIR/**")?;
        self = self.exclude_glob("**/*.pb.rs")?;
        self = self.exclude_glob("**/*_pb.rs")?;
        self = self.exclude_glob("**/bindings.rs")?;
        self = self.exclude_glob("**/*_bindings.rs")?;
        Ok(self)
    }

    pub fn follow_links(mut self, yes: bool) -> Self { self.follow_links = yes; self }
    pub fn max_file_size(mut self, bytes: u64) -> Self { self.max_file_size = Some(bytes); self }
    /// Skip files syn cannot parse instead of failing the whole scan; the
    /// skipped paths are reported in `Klepto::skipped_files`.
    pub fn ignore_parse_errors(mut self, yes: bool) -> Self { self.ignore_parse_errors = yes; self }
    pub fn only_newest(mut self, n: usize) -> Self { self.only_newest = Some(n); self }

    pub fn include_tests(mut self, yes: bool) -> Self { self.add_tests = yes; self }
    pub fn include_examples(mut self, yes: bool) -> Self { self.add_examples = yes; self }
    pub fn include_benches(mut self, yes: bool) -> Self { self.add_benches = yes; self }
    /// Do not read dependency sources for `#[macro_export]` macros.
    pub fn skip_dependency_macros(mut self, yes: bool) -> Self { self.no_dep_macros = yes; self }

    /// The `.rs` files `parse` would read, newest first.
    fn candidates(&self) -> Result<Vec<(PathBuf, SystemTime)>, KleptoError> {
        let include: GlobSet = self.include.build()?;
        let exclude: GlobSet = self.exclude.build()?;

        let mut candidates: Vec<(PathBuf, SystemTime)> = Vec::new();

        for root in &self.read_roots() {
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
        Ok(candidates)
    }

    /// The files this scan would read (its `.rs` files, every member's
    /// manifest, the root manifest and lockfile) with the blake3 of each,
    /// without parsing anything: equal fingerprints, equal scans.
    pub fn fingerprint(&self) -> Result<Fingerprint, KleptoError> {
        let mut paths: Vec<PathBuf> = self.candidates()?.into_iter().map(|(p, _)| p).collect();
        paths.extend(self.packages.values().map(|p| p.manifest_path.clone()));
        if let Some(root) = &self.scan_root {
            paths.extend([root.join("Cargo.toml"), root.join("Cargo.lock")].into_iter().filter(|p| p.is_file()));
        }
        paths.sort();
        paths.dedup();
        let mut files = Vec::with_capacity(paths.len());
        let mut all = blake3::Hasher::new();
        for p in paths {
            let bytes = std::fs::read(&p).map_err(|e| KleptoError::Io { path: p.clone(), source: e })?;
            let hash = blake3::hash(&bytes).to_hex().to_string();
            all.update(p.to_string_lossy().as_bytes());
            all.update(b"\0");
            all.update(hash.as_bytes());
            all.update(b"\n");
            files.push((p, hash));
        }
        Ok(Fingerprint { files, digest: all.finalize().to_hex().to_string() })
    }

    pub fn parse(self) -> Result<Klepto, KleptoError> {
        let candidates = self.candidates()?;

        let mut files = Vec::new();
        let mut skipped_files = Vec::new();
        for (path, modified) in candidates {
            match parse_one(&path, modified, self.max_file_size) {
                Ok(Some(pf)) => files.push(pf),
                Ok(None) => {}
                Err(e @ KleptoError::Parse { .. }) if self.ignore_parse_errors => {
                    skipped_files.push((path, e.to_string()));
                }
                Err(e) => return Err(e),
            }
        }

        // Expand `macro_rules!` before anything reads
        // the ASTs, so generated items, mods and calls are ordinary ones.
        let crate_pkg = norm_crate_root(&self.crate_name);
        let root_dirs: Vec<(PathBuf, String)> =
            self.package_roots.iter().filter_map(|r| r.file.parent().map(|d| (d.to_path_buf(), r.package.clone()))).collect();
        let package_of = |path: &Path| -> String {
            root_dirs
                .iter()
                .filter(|(d, _)| path.starts_with(d))
                .max_by_key(|(d, _)| d.as_os_str().len())
                .map(|(_, p)| p.clone())
                .unwrap_or_else(|| crate_pkg.clone())
        };
        let mut table = crate::resolve::expand::MacroTable::default();
        for pf in &files {
            table.harvest(&pf.ast, &package_of(&pf.path), false, &pf.path);
        }
        let mut dep_macro_note = self.dep_macro_note.clone();
        if !self.no_dep_macros {
            for (name, src) in &self.dep_sources {
                if !self.dependency_crates.contains(name) {
                    continue;
                }
                for entry in WalkDir::new(src).into_iter().filter_map(|e| e.ok()) {
                    let p = entry.path();
                    if !p.is_file() || p.extension().and_then(|e| e.to_str()) != Some("rs") {
                        continue;
                    }
                    let Ok(text) = std::fs::read_to_string(p) else { continue };
                    if !text.contains("macro_export") {
                        continue;
                    }
                    match syn::parse_file(&text) {
                        Ok(ast) => table.harvest(&ast, name, true, p),
                        Err(e) => dep_macro_note.get_or_insert_with(String::new).push_str(&format!("{}: {e}; ", p.display())),
                    }
                }
            }
        }
        // macros that expansions define (syn's `Token!` comes out of
        // another macro) are found by expanding again from the parsed files
        // until no new `macro_rules!` appears (at most four rounds).
        let originals: Vec<syn::File> = files.iter().map(|f| f.ast.clone()).collect();
        let mut expansions = Vec::new();
        for round in 0..4 {
            expansions.clear();
            for (pf, orig) in files.iter_mut().zip(&originals) {
                if round > 0 {
                    pf.ast = orig.clone();
                }
                let pkg = package_of(&pf.path);
                let deps: Vec<String> = self.package_deps.get(&pkg).map(|d| d.iter().cloned().collect()).unwrap_or_default();
                expansions.extend(crate::resolve::expand::expand_file(&mut pf.ast, &table, &pkg, deps, &pf.path));
            }
            let before = table.len();
            for pf in &files {
                table.harvest(&pf.ast, &package_of(&pf.path), false, &pf.path);
            }
            if table.len() == before {
                break;
            }
        }
        let macros_known = table.names();

        let targets = self.crate_root_targets();
        let read_roots = self.read_roots();
        let modules = crate::resolve::build_module_tree(&self.crate_name, &targets, &read_roots, &mut files);
        // a bin, test, example or bench crate root is named after its target; it resolves with
        // its package's dependencies, and a test, example or bench with its package's lib too
        let names = crate::resolve::root_names(&targets);
        let crate_roots: BTreeMap<String, String> = names.iter().cloned().collect();
        let libs: BTreeSet<&str> = self.package_roots.iter().filter(|r| r.kind != "bin").map(|r| r.package.as_str()).collect();
        let mut package_deps = self.package_deps.clone();
        for (t, (root, _)) in targets.iter().zip(&names) {
            if *root == t.package {
                continue;
            }
            let mut d = self.package_deps.get(&t.package).cloned().unwrap_or_default();
            if matches!(t.kind.as_str(), "test" | "example" | "bench") && libs.contains(t.package.as_str()) {
                d.insert(t.package.clone());
            }
            package_deps.entry(root.clone()).or_insert(d);
        }

        // extract caches
        let mut functions = Vec::new();
        let mut imports = Vec::new();
        let mut exports = Vec::new();

        let mut macros_def = Vec::new();
        let mut macros_inv = Vec::new();
        let mut paths = Vec::new();
        let mut calls = Vec::new();
        let mut attrs = Vec::new();
        let mut extern_crates = Vec::new();
        let mut pending_singles = Vec::new();
        let mut items = Vec::new();
        let mut types = Vec::new();
        let mut impls = Vec::new();
        let mut literals = Vec::new();
        let mut matches = Vec::new();

        let mut no_std_detected = false;
        let mut index = crate::syntax::index::EnclosingIndex::default();

        for pf in &files {
            if pf.is_no_std_crate_root { no_std_detected = true; }

            let (package, segs, base_cfg) = pf
                .module
                .as_deref()
                .and_then(|m| modules.get(m))
                .map(|mi| (mi.package.clone(), mi.segments.clone(), mi.cfg.clone()))
                .unwrap_or_else(|| (crate_pkg.clone(), Vec::new(), CfgPredicate::True));

            functions.extend(extract_functions(&package, &segs, &base_cfg, &pf.path, &pf.ast));
            imports.extend(extract_imports(&pf.path, &segs, &base_cfg, &pf.ast));
            exports.extend(extract_public_surface(&pf.path, &segs, &base_cfg, &pf.ast));

            let occ = extract_occurrences(&package, &segs, &base_cfg, &pf.path, &pf.ast);
            macros_def.extend(occ.macros_def);
            macros_inv.extend(occ.macros_inv);
            paths.extend(occ.paths);
            calls.extend(occ.calls);
            attrs.extend(occ.attrs);
            extern_crates.extend(occ.extern_crates);
            imports.extend(occ.body_imports);
            pending_singles.extend(occ.single_paths);
            items.extend(occ.items);
            types.extend(occ.types);
            impls.extend(occ.impls);
            literals.extend(occ.literals);
            matches.extend(occ.matches);

            index = index.merge(crate::syntax::index::EnclosingIndex::build(&package, &segs, &pf.path, &pf.ast));
        }

        classify_imports(&mut imports, &self.workspace_members, &self.dependency_crates);

        let mut k = Klepto {
            crate_name: self.crate_name,
            files,
            functions,
            imports,
            exports,
            macros_def,
            macros_inv,
            paths,
            calls,
            attrs,
            extern_crates,
            items,
            types,
            impls,
            modules,
            graph: ItemGraph::default(),
            package_deps,
            crate_roots,
            features: self.features,
            packages: self.packages,
            no_std_detected,
            expansions,
            macros_known,
            dep_macro_note,
            dependency_features: self.dependency_features,
            root: self.scan_root.clone(),
            index,
            skipped_files,
            literals,
            matches,
            pending_singles,
        };
        crate::resolve::resolve_all(&mut k);
        crate::typing::types::resolve_types(&mut k);
        crate::typing::methods::resolve_method_calls(&mut k);
        link_arguments(&mut k);
        k.graph = crate::graph::build_item_graph(&k);
        Ok(k)
    }
}

/// Copies each call's resolution onto the literals and paths written in its
/// arguments, matched by the call's location, kind and (for a method, which
/// starts where its receiver does) name.
fn link_arguments(k: &mut Klepto) {
    type Key = (PathBuf, Option<u32>, Option<u32>, bool, String);
    let key = |loc: &crate::model::FileLocation, is_method: bool, callee: &str| -> Key {
        (loc.path.clone(), loc.line, loc.column, is_method, if is_method { callee.to_string() } else { String::new() })
    };
    let mut by_call: std::collections::HashMap<Key, crate::model::Resolution> = std::collections::HashMap::new();
    for c in &k.calls {
        if let Some(r) = &c.resolution {
            by_call.entry(key(&c.location, c.is_method, &c.callee)).or_insert_with(|| r.clone());
        }
    }
    let fill = |a: &mut Option<crate::model::ArgOf>| {
        if let Some(a) = a {
            a.resolution = by_call.get(&key(&a.location, a.is_method, &a.callee)).cloned();
        }
    };
    for l in &mut k.literals {
        fill(&mut l.arg_of);
    }
    for p in &mut k.paths {
        fill(&mut p.arg_of);
    }
    // an arm's pattern path takes the resolution of the path occurrence written there
    let mut at: std::collections::HashMap<(PathBuf, Option<u32>, Option<u32>, String), crate::model::Resolution> = std::collections::HashMap::new();
    for p in &k.paths {
        if let Some(r) = &p.resolution {
            at.entry((p.location.path.clone(), p.location.line, p.location.column, p.path.clone())).or_insert_with(|| r.clone());
        }
    }
    for m in &mut k.matches {
        for arm in &mut m.arms {
            for ap in &mut arm.paths {
                ap.resolution = at.get(&(ap.location.path.clone(), ap.location.line, ap.location.column, ap.path.clone())).cloned();
            }
        }
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
        module: None,
    }))
}

fn norm_crate_root(s: &str) -> String {
    s.replace('-', "_")
}

fn classify_imports(
    imports: &mut [StolenPath],
    workspace_members: &HashSet<String>,
    dependency_crates: &HashSet<String>,
) {
    use ImportOrigin::*;

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
