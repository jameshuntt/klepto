//! The workspace as cargo resolved it, read from one `cargo metadata` (its JSON): every package cargo read with its
//! edition, its `rust-version`, its source, the requirements it declares (as cargo wrote them) and its feature model;
//! the resolve's edges, with the features cargo turned on for each package. Nothing here resolves anything: every
//! field is cargo's, or it is absent (`--no-deps` has no resolve).

use crate::klepto::KleptoError;
use crate::resolve::features::{dep_info_of, feature_model_of, DepInfo, DepKind, FeatureModel};
use cargo_metadata::semver::{Version, VersionReq};
use cargo_metadata::MetadataCommand;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// A dependency a package declares, as `cargo metadata` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclaredDependency {
    /// Its name in source, its package, kind, target, optional, default features and features.
    pub dep: DepInfo,
    /// The version requirement as cargo wrote it (`^1.0.200`, `*` for a path dependency with no version).
    pub req: String,
    /// The `[target.'..'.dependencies]` key as written (`cfg(unix)`, a triple).
    pub target: Option<String>,
    /// `registry+https://..`, `git+https://..`; `None` for a path dependency.
    pub source: Option<String>,
    /// A path dependency's directory.
    pub path: Option<PathBuf>,
    /// An alternative registry's name.
    pub registry: Option<String>,
}

/// One package cargo read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CargoPackage {
    /// Cargo's package id.
    pub id: String,
    /// The name as published (`-` kept).
    pub name: String,
    pub version: String,
    /// `2015`, `2018`, `2021`, `2024`.
    pub edition: String,
    /// The `rust-version` it declares, as cargo reports it (`1.89.0`); `None` when it declares none.
    pub rust_version: Option<String>,
    pub manifest_path: PathBuf,
    /// `None` for a path package (a member, a path dependency).
    pub source: Option<String>,
    pub dependencies: Vec<DeclaredDependency>,
    /// klepto's feature model of it (the four entry forms, cargo's implicit features).
    pub features: FeatureModel,
}

/// One edge of cargo's resolve.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedEdge {
    /// The dependency's name in source (a rename's name).
    pub name: String,
    /// The package id it resolved to.
    pub package: String,
    /// Each kind it is depended on as, with its target when it has one.
    pub kinds: Vec<(DepKind, Option<String>)>,
}

/// One package in cargo's resolve.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ResolvedNode {
    /// The features cargo turned on for it, unified over the workspace.
    pub features: Vec<String>,
    pub deps: Vec<ResolvedEdge>,
}

/// One `cargo metadata`, read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CargoWorkspace {
    pub workspace_root: PathBuf,
    /// The members' package ids, in cargo's order.
    pub members: Vec<String>,
    /// Every package cargo read, by id.
    pub packages: BTreeMap<String, CargoPackage>,
    /// The resolve, by package id; `None` from `--no-deps`.
    pub resolve: Option<BTreeMap<String, ResolvedNode>>,
}

impl CargoWorkspace {
    /// The workspace from `cargo metadata --format-version 1` output (with or without `--no-deps`).
    pub fn from_json(json: &str) -> Result<CargoWorkspace, KleptoError> {
        let meta: cargo_metadata::Metadata = serde_json::from_str(json).map_err(|e| KleptoError::CargoMeta(cargo_metadata::Error::Json(e)))?;
        Ok(Self::from_metadata(&meta))
    }

    /// `cargo metadata` of the workspace `manifest` belongs to, run here: `--frozen` (no lock written, no network),
    /// or `--no-deps` (members only, nothing resolved).
    pub fn read(manifest: &Path, resolve: bool) -> Result<CargoWorkspace, KleptoError> {
        let mut cmd = MetadataCommand::new();
        cmd.manifest_path(manifest);
        if resolve {
            cmd.other_options(vec!["--frozen".to_string()]);
        } else {
            cmd.no_deps();
        }
        Ok(Self::from_metadata(&cmd.exec()?))
    }

    pub(crate) fn from_metadata(meta: &cargo_metadata::Metadata) -> CargoWorkspace {
        let packages = meta
            .packages
            .iter()
            .map(|p| {
                let dependencies = p
                    .dependencies
                    .iter()
                    .map(|d| DeclaredDependency {
                        dep: dep_info_of(d),
                        req: d.req.to_string(),
                        target: d.target.as_ref().map(|t| t.to_string()),
                        source: d.source.clone(),
                        path: d.path.clone().map(|p| p.into_std_path_buf()),
                        registry: d.registry.clone(),
                    })
                    .collect();
                let pkg = CargoPackage {
                    id: p.id.repr.clone(),
                    name: p.name.clone(),
                    version: p.version.to_string(),
                    edition: p.edition.as_str().to_string(),
                    rust_version: p.rust_version.as_ref().map(|v| v.to_string()),
                    manifest_path: p.manifest_path.clone().into_std_path_buf(),
                    source: p.source.as_ref().map(|s| s.repr.clone()),
                    dependencies,
                    features: feature_model_of(p),
                };
                (pkg.id.clone(), pkg)
            })
            .collect();
        let resolve = meta.resolve.as_ref().map(|r| {
            r.nodes
                .iter()
                .map(|n| {
                    let deps = n
                        .deps
                        .iter()
                        .map(|d| ResolvedEdge {
                            name: d.name.clone(),
                            package: d.pkg.repr.clone(),
                            kinds: d
                                .dep_kinds
                                .iter()
                                .map(|k| {
                                    let kind = match k.kind {
                                        cargo_metadata::DependencyKind::Development => DepKind::Dev,
                                        cargo_metadata::DependencyKind::Build => DepKind::Build,
                                        _ => DepKind::Normal,
                                    };
                                    (kind, k.target.as_ref().map(|t| t.to_string()))
                                })
                                .collect(),
                        })
                        .collect();
                    (n.id.repr.clone(), ResolvedNode { features: n.features.clone(), deps })
                })
                .collect()
        });
        CargoWorkspace {
            workspace_root: meta.workspace_root.clone().into_std_path_buf(),
            members: meta.workspace_members.iter().map(|m| m.repr.clone()).collect(),
            packages,
            resolve,
        }
    }

    /// The members, in cargo's order.
    pub fn member_packages(&self) -> Vec<&CargoPackage> {
        self.members.iter().filter_map(|id| self.packages.get(id)).collect()
    }

    /// Whether `id` is a member.
    pub fn is_member(&self, id: &str) -> bool {
        self.members.iter().any(|m| m == id)
    }

    /// Every package `id` reaches through cargo's resolve (itself left out); `kinds` limits the first step to edges
    /// of those kinds (a member's dev-dependencies are its own; cargo lists none for a dependency), every step after
    /// it follows normal and build edges. Empty without a resolve.
    pub fn closure(&self, id: &str, kinds: &[DepKind]) -> BTreeSet<String> {
        let Some(resolve) = &self.resolve else { return BTreeSet::new() };
        let mut seen = BTreeSet::new();
        let mut work: Vec<String> = resolve
            .get(id)
            .map(|n| n.deps.iter().filter(|e| e.kinds.iter().any(|(k, _)| kinds.contains(k))).map(|e| e.package.clone()).collect())
            .unwrap_or_default();
        while let Some(p) = work.pop() {
            if p == id || !seen.insert(p.clone()) {
                continue;
            }
            if let Some(n) = resolve.get(&p) {
                work.extend(n.deps.iter().filter(|e| e.kinds.iter().any(|(k, _)| *k != DepKind::Dev)).map(|e| e.package.clone()));
            }
        }
        seen
    }

    /// Every package whose resolve reaches `id` (its reverse dependents, transitive), with the direct ones apart.
    pub fn dependents(&self, id: &str) -> (BTreeSet<String>, BTreeSet<String>) {
        let Some(resolve) = &self.resolve else { return Default::default() };
        let direct: BTreeSet<String> = resolve.iter().filter(|(_, n)| n.deps.iter().any(|e| e.package == id)).map(|(p, _)| p.clone()).collect();
        let mut all = BTreeSet::new();
        let mut work: Vec<String> = direct.iter().cloned().collect();
        while let Some(p) = work.pop() {
            if p == id || !all.insert(p.clone()) {
                continue;
            }
            work.extend(resolve.iter().filter(|(_, n)| n.deps.iter().any(|e| e.package == p)).map(|(q, _)| q.clone()));
        }
        (direct, all)
    }

    /// The packages of a name (`-` and `_` alike), in version order.
    pub fn by_name(&self, name: &str) -> Vec<&CargoPackage> {
        let n = name.replace('-', "_");
        let mut v: Vec<&CargoPackage> = self.packages.values().filter(|p| p.name.replace('-', "_") == n).collect();
        v.sort_by(|a, b| match (Version::parse(&a.version), Version::parse(&b.version)) {
            (Ok(x), Ok(y)) => x.cmp(&y),
            _ => a.version.cmp(&b.version),
        });
        v
    }
}

/// Whether `version` meets the requirement `req` as cargo writes them (`^1.2`, `>=0.4, <0.6`, `*`): semver's answer,
/// `None` when either does not parse. A query of one pair, not a resolution.
pub fn version_meets(req: &str, version: &str) -> Option<bool> {
    let r = VersionReq::parse(req.trim()).ok()?;
    let v = Version::parse(version.trim()).ok()?;
    Some(r.matches(&v))
}
