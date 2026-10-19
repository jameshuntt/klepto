//! The item graph — one node per item, one edge per resolved
//! reference — and the closure queries on it.

use crate::resolve::cfg::{CfgPredicate, Implication};
use crate::resolve::features::{dep_lost_without, feature_closure, feature_steps, requires_feature, FeatureEdge, FeatureKey, FeatureModel};
use crate::klepto::Klepto;
use crate::model::{FileLocation, Resolution, UseKind};
use crate::resolve::{file_packages, join_key, norm};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NodeKind {
    Fn,
    Method,
    Struct,
    Enum,
    Union,
    Trait,
    TypeAlias,
    Const,
    Static,
    Macro,
    Module,
    Impl,
    /// An item of another crate, created on demand when referenced.
    External,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EdgeKind {
    /// A path, macro or attribute in the source item names the target.
    Reference,
    /// A `use` item (module-level from the module, body-level from the fn).
    Import,
    /// A `pub use` re-export from the module.
    ReExport,
    /// Module → child module / item, impl → method, trait → method, fn → nested item.
    Contains,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ItemNode {
    /// Canonical id: `app::store::Engine::new`, `app::<impl Config>`, `dep_x::x`.
    pub id: String,
    pub kind: NodeKind,
    /// `None` for external nodes.
    pub package: Option<String>,
    /// Module key (`app::store`); for externals, the crate root name.
    pub module: String,
    pub name: String,
    /// `None` for external nodes.
    pub location: Option<FileLocation>,
    /// Effective cfg; `True` for externals.
    pub cfg: CfgPredicate,
    pub is_public: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ItemEdge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
    pub location: FileLocation,
    /// Effective cfg at the referencing site (already includes `from`'s).
    pub cfg: CfgPredicate,
    /// The target is one of cfg-twin bindings that together cover
    /// every build; not judged by `cfg_edge_violations`.
    #[serde(default)]
    pub covered: bool,
}

/// An occurrence that could not become an edge.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnresolvedReference {
    /// Enclosing item id, or the module key at module level.
    pub from: String,
    pub raw: String,
    pub resolution: Resolution,
    pub location: FileLocation,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ItemGraph {
    pub nodes: BTreeMap<String, ItemNode>,
    pub edges: Vec<ItemEdge>,
    pub unresolved: Vec<UnresolvedReference>,
    /// Ids declared more than once; the first declaration is kept.
    pub duplicates: Vec<String>,
}

const DEPENDENCY_KINDS: [EdgeKind; 3] = [EdgeKind::Reference, EdgeKind::Import, EdgeKind::ReExport];
const DEPENDENT_KINDS: [EdgeKind; 2] = [EdgeKind::Reference, EdgeKind::ReExport];

fn crate_root(s: &str) -> &str {
    s.split("::").next().unwrap_or("")
}

fn last_segment(s: &str) -> &str {
    s.rsplit("::").next().unwrap_or(s)
}

impl ItemGraph {
    pub fn node(&self, id: &str) -> Option<&ItemNode> {
        self.nodes.get(id)
    }

    /// The exact node, or — for a path with at least two segments —
    /// the node for the path minus its last segment when that node is neither
    /// a module nor external (an associated item, variant or unknown method).
    pub fn resolve_id(&self, canonical: &str) -> Option<&str> {
        if let Some((k, _)) = self.nodes.get_key_value(canonical) {
            return Some(k.as_str());
        }
        let (prefix, _) = canonical.rsplit_once("::")?;
        match self.nodes.get_key_value(prefix) {
            Some((k, n)) if !matches!(n.kind, NodeKind::Module | NodeKind::External) => Some(k.as_str()),
            _ => None,
        }
    }

    pub fn edges_from(&self, id: &str) -> Vec<&ItemEdge> {
        self.edges.iter().filter(|e| e.from == id).collect()
    }

    pub fn edges_to(&self, id: &str) -> Vec<&ItemEdge> {
        self.edges.iter().filter(|e| e.to == id).collect()
    }

    pub fn has_edge(&self, from: &str, to: &str, kind: EdgeKind) -> bool {
        self.edges.iter().any(|e| e.from == from && e.to == to && e.kind == kind)
    }

    /// What `id` uses directly (Reference, Import, ReExport edges).
    pub fn dependencies_of(&self, id: &str) -> BTreeSet<String> {
        self.edges
            .iter()
            .filter(|e| e.from == id && DEPENDENCY_KINDS.contains(&e.kind))
            .map(|e| e.to.clone())
            .collect()
    }

    /// What uses `id` directly (Reference and ReExport edges).
    pub fn dependents_of(&self, id: &str) -> BTreeSet<String> {
        self.edges
            .iter()
            .filter(|e| e.to == id && DEPENDENT_KINDS.contains(&e.kind))
            .map(|e| e.from.clone())
            .collect()
    }

    /// Everything that transitively uses any of `seeds` (seeds excluded).
    pub fn transitive_dependents<'a>(&self, seeds: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
        let seeds: BTreeSet<String> = seeds.into_iter().map(str::to_string).collect();
        let mut seen = seeds.clone();
        let mut work: Vec<String> = seeds.iter().cloned().collect();
        while let Some(id) = work.pop() {
            for d in self.dependents_of(&id) {
                if seen.insert(d.clone()) {
                    work.push(d);
                }
            }
        }
        seen.difference(&seeds).cloned().collect()
    }

    /// Everything `id` transitively uses (`id` excluded).
    pub fn transitive_dependencies(&self, id: &str) -> BTreeSet<String> {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut work = vec![id.to_string()];
        while let Some(cur) = work.pop() {
            for d in self.dependencies_of(&cur) {
                if seen.insert(d.clone()) {
                    work.push(d);
                }
            }
        }
        seen.remove(id);
        seen
    }

    /// External node ids grouped by crate.
    pub fn external_roots(&self) -> BTreeMap<String, BTreeSet<String>> {
        let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for n in self.nodes.values().filter(|n| n.kind == NodeKind::External) {
            out.entry(n.module.clone()).or_default().insert(n.id.clone());
        }
        out
    }

    /// Node ids of a dependency: a scanned member's own nodes, or the
    /// external nodes of that crate.
    fn targets_of(&self, dep: &str) -> HashSet<String> {
        self.nodes
            .values()
            .filter(|n| n.package.as_deref() == Some(dep) || (n.kind == NodeKind::External && n.module == dep))
            .map(|n| n.id.clone())
            .collect()
    }

    fn is_local(&self, id: &str) -> bool {
        self.nodes.get(id).is_some_and(|n| n.package.is_some())
    }
}

/// Result of `affected_by_*`: ids sorted, the three lists disjoint.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Affected {
    /// Items whose own cfg cannot hold without the feature.
    pub gated: Vec<String>,
    /// Items (not in `gated`) that directly reference a dependency that goes away.
    pub uses_lost_dep: Vec<String>,
    /// Everything else reached through Reference / ReExport edges.
    pub transitive: Vec<String>,
}

/// One node of a feature's closure and the items it brings: for a feature
/// of a crate, the crate's items whose own cfg cannot hold without it (as
/// `affected_by_feature(..).gated` computes them); for a crate itself (an
/// optional dependency turned on), its items no feature of it gates.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeatureItems {
    pub key: FeatureKey,
    /// Sorted ids.
    pub items: Vec<String>,
    /// The edge that brought it (the shortest chain's last); `None` for
    /// the feature asked about.
    pub enabled_by: Option<FeatureEdge>,
    /// Whether the crate was scanned: one that was not has no model and
    /// no items, and its features are not followed.
    pub scanned: bool,
}

/// `feature_items(package, feature)`: everything the feature brings, in
/// its crate and in its dependencies, crate by crate.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeatureItemMap {
    pub package: String,
    pub feature: String,
    /// The closure, breadth first from the feature asked about.
    pub entries: Vec<FeatureItems>,
    /// `name?/feat` entries whose dependency nothing in the closure turns
    /// on: they bring `feat` only when something else turns `name` on.
    pub weak_pending: Vec<FeatureEdge>,
}

impl FeatureItemMap {
    pub fn get(&self, package: &str, feature: Option<&str>) -> Option<&FeatureItems> {
        self.entries.iter().find(|e| e.key.package == package && e.key.feature.as_deref() == feature)
    }

    /// Crate → its entries.
    pub fn by_crate(&self) -> BTreeMap<&str, Vec<&FeatureItems>> {
        let mut out: BTreeMap<&str, Vec<&FeatureItems>> = BTreeMap::new();
        for e in &self.entries {
            out.entry(e.key.package.as_str()).or_default().push(e);
        }
        out
    }

    /// Every item the feature brings in one crate.
    pub fn items_of(&self, package: &str) -> BTreeSet<&str> {
        self.entries.iter().filter(|e| e.key.package == package).flat_map(|e| e.items.iter().map(String::as_str)).collect()
    }
}

/// `features_bringing(package, item)`: one feature that brings the item,
/// with the chain of entries from it to the feature (or crate) of the
/// item's own crate that gates the item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeatureChain {
    pub package: String,
    pub feature: String,
    /// The edges in order, from `package:feature` on (the shortest chain);
    /// empty when the feature is the item's own gate.
    pub edges: Vec<FeatureEdge>,
    /// The node of the item's own crate the chain ends on: the feature
    /// gating the item, or the crate itself for an item no feature gates.
    /// An item that needs several features has a chain to each.
    pub gate: FeatureKey,
}

impl FeatureChain {
    /// The chain has a `name?/feat` entry: it brings the item only when
    /// that dependency is on otherwise.
    pub fn weak(&self) -> bool {
        self.edges.iter().any(|e| e.weak)
    }
}

/// An item references another item that is not compiled in some
/// configuration where the reference is.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CfgEdgeViolation {
    pub package: String,
    pub from: String,
    pub to: String,
    pub location: FileLocation,
    pub edge_cfg: CfgPredicate,
    pub to_cfg: CfgPredicate,
    pub verdict: Implication,
}

// ---------------------------------------------------------------------------
// builder
// ---------------------------------------------------------------------------

struct Builder<'k> {
    k: &'k Klepto,
    g: ItemGraph,
    seen_edges: HashSet<(String, String, EdgeKind, FileLocation)>,
    /// Whether the occurrence being turned into edges is covered.
    next_covered: bool,
    file_pkg: HashMap<PathBuf, String>,
    crate_pkg: String,
    /// Package names that were scanned (their items are local nodes).
    scanned: HashSet<String>,
}

impl<'k> Builder<'k> {
    fn module_key_of(&self, path: &Path, module_path: &[String]) -> String {
        let pkg = self.file_pkg.get(path).cloned().unwrap_or_else(|| self.crate_pkg.clone());
        join_key(&pkg, module_path)
    }

    /// The node for a canonical path outside the scanned packages, created once.
    fn ensure_external(&mut self, canonical: &str) {
        if self.g.nodes.contains_key(canonical) {
            return;
        }
        self.g.nodes.insert(
            canonical.to_string(),
            ItemNode {
                id: canonical.to_string(),
                kind: NodeKind::External,
                package: None,
                module: crate_root(canonical).to_string(),
                name: last_segment(canonical).to_string(),
                location: None,
                cfg: CfgPredicate::True,
                is_public: true,
            },
        );
    }

    fn add_edge(&mut self, from: &str, to: &str, kind: EdgeKind, location: &FileLocation, cfg: &CfgPredicate) {
        if from == to {
            return;
        }
        let key = (from.to_string(), to.to_string(), kind, location.clone());
        if self.seen_edges.insert(key) {
            self.g.edges.push(ItemEdge { from: from.to_string(), to: to.to_string(), kind, location: location.clone(), cfg: cfg.clone(), covered: self.next_covered });
        }
    }

    fn unresolved(&mut self, from: &str, raw: &str, res: &Resolution, location: &FileLocation) {
        self.g.unresolved.push(UnresolvedReference {
            from: from.to_string(),
            raw: raw.to_string(),
            resolution: res.clone(),
            location: location.clone(),
        });
    }

    /// The edge, or the unresolved record, for one resolved occurrence.
    fn add_ref(&mut self, from: &str, res: &Resolution, kind: EdgeKind, location: &FileLocation, cfg: &CfgPredicate) {
        match res {
            Resolution::Local(s) => match self.g.resolve_id(s).map(str::to_string) {
                Some(to) => self.add_edge(from, &to, kind, location, cfg),
                None => self.unresolved(from, s, res, location),
            },
            Resolution::External(e) => {
                if self.scanned.contains(crate_root(e)) {
                    // a workspace member: land on its own local node
                    match self.g.resolve_id(e).map(str::to_string) {
                        Some(to) => self.add_edge(from, &to, kind, location, cfg),
                        None => self.unresolved(from, e, res, location),
                    }
                } else {
                    self.ensure_external(e);
                    self.add_edge(from, e, kind, location, cfg);
                }
            }
            Resolution::Unresolved { raw, .. } => self.unresolved(from, raw, res, location),
        }
    }

    fn build(mut self) -> ItemGraph {
        let k = self.k;

        // module nodes and module containment
        for (key, mi) in &k.modules.modules {
            self.g.nodes.insert(
                key.clone(),
                ItemNode {
                    id: key.clone(),
                    kind: NodeKind::Module,
                    package: Some(mi.package.clone()),
                    module: key.clone(),
                    name: mi.segments.last().cloned().unwrap_or_else(|| mi.package.clone()),
                    location: Some(FileLocation { path: mi.file.clone(), line: None, column: None }),
                    cfg: mi.cfg.clone(),
                    is_public: mi.is_pub,
                },
            );
        }
        for (key, mi) in &k.modules.modules {
            if mi.segments.is_empty() {
                continue;
            }
            let parent = join_key(&mi.package, &mi.segments[..mi.segments.len() - 1]);
            let loc = FileLocation { path: mi.file.clone(), line: None, column: None };
            self.add_edge(&parent, key, EdgeKind::Contains, &loc, &mi.cfg);
        }

        // Item nodes
        let mut first: Vec<bool> = Vec::with_capacity(k.items.len());
        let mut dup_cfgs: HashMap<String, Vec<CfgPredicate>> = HashMap::new();
        for d in &k.items {
            if self.g.nodes.contains_key(&d.id) {
                self.g.duplicates.push(d.id.clone());
                first.push(false);
                let entry = dup_cfgs.entry(d.id.clone()).or_default();
                if entry.is_empty() {
                    if let Some(n) = self.g.nodes.get(&d.id) {
                        entry.push(n.cfg.clone());
                    }
                }
                entry.push(d.cfg.clone());
                continue;
            }
            self.g.nodes.insert(
                d.id.clone(),
                ItemNode {
                    id: d.id.clone(),
                    kind: d.kind,
                    package: Some(d.package.clone()),
                    module: d.module.clone(),
                    name: d.name.clone(),
                    location: Some(d.location.clone()),
                    cfg: d.cfg.clone(),
                    is_public: d.is_public,
                },
            );
            first.push(true);
        }
        // An item declared several times under cfgs that
        // together cover every configuration (`cfg(feature = "x")` and
        // `cfg(not(feature = "x"))`) is always present — its node's cfg is
        // `True`, not the first declaration's.
        // When the twins do not provably cover everything (`fast_arithmetic =
        // "32"` / `"64"`, set by a build script; `unix` / `windows`), klepto
        // cannot say which declaration applies: the node's cfg is Unknown —
        // never the first twin's cfg, which would be a claim.
        for (id, cfgs) in dup_cfgs {
            let together = CfgPredicate::Any(cfgs).simplify();
            let covered = matches!(CfgPredicate::True.implies(&together, None), Implication::Always);
            if let Some(n) = self.g.nodes.get_mut(&id) {
                n.cfg = if covered { CfgPredicate::True } else { CfgPredicate::Unknown(format!("twins: {}", together.render())) };
            }
        }
        // Containment of items
        for (d, is_first) in k.items.iter().zip(first) {
            if !is_first {
                continue;
            }
            let parent = d.parent.clone().unwrap_or_else(|| d.module.clone());
            self.add_edge(&parent, &d.id, EdgeKind::Contains, &d.location, &d.cfg);
        }

        // References
        for p in &k.paths {
            self.next_covered = p.covered;
            let from = p.enclosing_item.clone().unwrap_or_else(|| self.module_key_of(&p.location.path, &p.module_path));
            if let Some(res) = &p.resolution {
                self.add_ref(&from, res, EdgeKind::Reference, &p.location, &p.cfg);
            }
        }
        for m in &k.macros_inv {
            self.next_covered = m.covered;
            let from = m.enclosing_item.clone().unwrap_or_else(|| self.module_key_of(&m.location.path, &m.module_path));
            if let Some(res) = &m.resolution {
                self.add_ref(&from, res, EdgeKind::Reference, &m.location, &m.cfg);
            }
        }
        self.next_covered = false;
        // A built-in attribute's own name (`cfg`,
        // `inline`, `allow`) is not a reference, and attribute arguments are
        // references only for `derive` or when they are real paths (two or
        // more segments); `feature = "x"`, `rename = "y"`, `any(..)` are
        // attribute syntax, not names to resolve.
        for a in &k.attrs {
            let from = a.enclosing_item.clone().unwrap_or_else(|| self.module_key_of(&a.location.path, &a.module_path));
            let own_is_builtin = !a.path.contains("::") && crate::resolve::is_builtin_attr(&a.path);
            if let Some(res) = &a.resolution {
                if !own_is_builtin {
                    self.add_ref(&from, res, EdgeKind::Reference, &a.location, &a.cfg);
                }
            }
            let is_derive = a.path == "derive";
            for (arg, res) in a.arg_paths.iter().zip(&a.arg_resolutions) {
                if is_derive || arg.contains("::") {
                    self.add_ref(&from, res, EdgeKind::Reference, &a.location, &a.cfg);
                }
            }
        }
        // a method call is an edge only when its receiver was typed
        for c in k.calls.iter().filter(|c| c.is_method) {
            self.next_covered = c.covered;
            let from = c.enclosing_item.clone().unwrap_or_else(|| self.module_key_of(&c.location.path, &c.module_path));
            match &c.resolution {
                Some(res @ (Resolution::Local(_) | Resolution::External(_))) => {
                    self.add_ref(&from, res, EdgeKind::Reference, &c.location, &c.cfg);
                }
                Some(res) => self.unresolved(&from, &c.callee, res, &c.location),
                None => {}
            }
        }
        self.next_covered = false;
        // Imports and re-exports
        for imp in &k.imports {
            let from = imp
                .enclosing_item
                .clone()
                .or_else(|| imp.enclosing_fn.clone())
                .unwrap_or_else(|| self.module_key_of(&imp.location.path, &imp.module_path));
            let kind = if imp.is_public_use { EdgeKind::ReExport } else { EdgeKind::Import };
            if let Some(res) = &imp.resolution {
                let _ = matches!(imp.kind, UseKind::Glob); // a glob targets its module / crate node like any other path
                self.add_ref(&from, res, kind, &imp.location, &imp.cfg);
            }
        }

        self.g
    }
}

/// Builds the item graph from the resolved facts: nodes for modules, items
/// and externals; edges for containment, references, imports and re-exports.
pub(crate) fn build_item_graph(k: &Klepto) -> ItemGraph {
    let scanned: HashSet<String> = k.modules.modules.values().map(|m| m.package.clone()).collect();
    Builder {
        k,
        g: ItemGraph::default(),
        seen_edges: HashSet::new(),
        next_covered: false,
        file_pkg: file_packages(k),
        crate_pkg: norm(&k.crate_name),
        scanned,
    }
    .build()
}

impl Klepto {
    pub fn item_graph(&self) -> &ItemGraph {
        &self.graph
    }

    /// Items lost when dependency `dep` goes away.
    pub fn affected_by_dep(&self, dep: &str) -> Affected {
        let dep = norm(dep);
        let g = &self.graph;
        let targets = g.targets_of(&dep);
        if targets.is_empty() {
            return Affected::default();
        }
        let direct: BTreeSet<String> = g
            .edges
            .iter()
            .filter(|e| DEPENDENT_KINDS.contains(&e.kind) && targets.contains(&e.to))
            .map(|e| e.from.clone())
            .filter(|f| g.is_local(f) && !targets.contains(f))
            .collect();
        let transitive: Vec<String> = g
            .transitive_dependents(direct.iter().map(String::as_str))
            .into_iter()
            .filter(|id| g.is_local(id) && !targets.contains(id))
            .collect();
        Affected { gated: Vec::new(), uses_lost_dep: direct.into_iter().collect(), transitive }
    }

    /// Items lost when `feature` of `package` is off.
    pub fn affected_by_feature(&self, package: &str, feature: &str) -> Affected {
        let Some(model) = self.features.get(package) else {
            return Affected::default();
        };
        let g = &self.graph;

        let gated = gated_items(model, feature, g.nodes.values().filter(|n| n.package.as_deref() == Some(package)));

        let lost_targets: HashSet<String> = model
            .dependencies
            .iter()
            .filter(|d| dep_lost_without(model, &d.name, feature))
            .flat_map(|d| g.targets_of(&d.name))
            .collect();

        let uses: BTreeSet<String> = g
            .edges
            .iter()
            .filter(|e| DEPENDENT_KINDS.contains(&e.kind) && lost_targets.contains(&e.to))
            .map(|e| e.from.clone())
            .filter(|f| g.node(f).is_some_and(|n| n.package.as_deref() == Some(package)) && !gated.contains(f))
            .collect();

        let seeds: BTreeSet<String> = gated.union(&uses).cloned().collect();
        let transitive: Vec<String> = g
            .transitive_dependents(seeds.iter().map(String::as_str))
            .into_iter()
            .filter(|id| g.is_local(id) && !seeds.contains(id))
            .collect();

        Affected { gated: gated.into_iter().collect(), uses_lost_dep: uses.into_iter().collect(), transitive }
    }

    /// What `feature` of `package` brings, crate by crate: the closure of
    /// features it turns on (its own, `name/feat` into each dependency's
    /// features and on down, `dep:name` turning the dependency itself on
    /// with its default and declared features, `name?/feat` once `name` is
    /// on) and, for each, the items it gates, with the edge that brought
    /// it. Each (crate, feature)'s items are computed once. Empty for a
    /// package without a model.
    pub fn feature_items(&self, package: &str, feature: &str) -> FeatureItemMap {
        if !self.features.contains_key(package) {
            return FeatureItemMap { package: package.to_string(), feature: feature.to_string(), ..Default::default() };
        }
        let (closure, weak_pending) = feature_closure(&self.features, package, feature);
        let mut items = KeyItems::new(self);
        let entries = closure
            .into_iter()
            .map(|(key, enabled_by)| FeatureItems { items: items.of(&key), scanned: self.features.contains_key(&key.package), key, enabled_by })
            .collect();
        FeatureItemMap { package: package.to_string(), feature: feature.to_string(), entries, weak_pending }
    }

    /// The reverse of `feature_items`: every (package, feature) whose
    /// closure reaches a node of `item`'s crate that brings `item`, each
    /// with its shortest chain. Weak entries are followed (`FeatureChain::weak`
    /// says so). Empty when `item` is not a node of `package`.
    pub fn features_bringing(&self, package: &str, item: &str) -> Vec<FeatureChain> {
        if self.graph.node(item).and_then(|n| n.package.as_deref()) != Some(package) {
            return Vec::new();
        }
        let mut items = KeyItems::new(self);
        let mut gates: Vec<FeatureKey> = Vec::new();
        if let Some(model) = self.features.get(package) {
            for f in model.features.keys() {
                let key = FeatureKey::new(package, Some(f));
                if items.of(&key).binary_search_by(|x| x.as_str().cmp(item)).is_ok() {
                    gates.push(key);
                }
            }
        }
        if gates.is_empty() {
            gates.push(FeatureKey::new(package, None));
        }
        // every step of every model, reversed
        let mut into: HashMap<FeatureKey, Vec<FeatureEdge>> = HashMap::new();
        for (p, model) in &self.features {
            for f in model.features.keys() {
                for step in feature_steps(&self.features, p, f) {
                    into.entry(step.to).or_default().push(step.edge);
                }
            }
        }
        // breadth first back from the gates: each node's next edge toward
        // one, the node that edge reaches, and the gate it leads to
        type Next = (Option<(FeatureEdge, FeatureKey)>, FeatureKey);
        let mut next: BTreeMap<FeatureKey, Next> = BTreeMap::new();
        let mut queue: std::collections::VecDeque<FeatureKey> = std::collections::VecDeque::new();
        for gate in &gates {
            next.insert(gate.clone(), (None, gate.clone()));
            queue.push_back(gate.clone());
        }
        while let Some(key) = queue.pop_front() {
            let gate = next[&key].1.clone();
            let mut edges: Vec<&FeatureEdge> = into.get(&key).map(|v| v.iter().collect()).unwrap_or_default();
            edges.sort();
            for e in edges {
                let from = e.from();
                if !next.contains_key(&from) {
                    next.insert(from.clone(), (Some((e.clone(), key.clone())), gate.clone()));
                    queue.push_back(from);
                }
            }
        }
        let mut out = Vec::new();
        for (key, (_, gate)) in &next {
            let Some(feature) = &key.feature else { continue };
            let mut edges = Vec::new();
            let mut cur = key;
            while let Some((Some((e, to)), _)) = next.get(cur) {
                edges.push(e.clone());
                cur = to;
            }
            out.push(FeatureChain { package: key.package.clone(), feature: feature.clone(), edges, gate: gate.clone() });
        }
        out
    }

    /// Item-to-item references whose cfg does not guarantee the target
    /// is compiled.
    pub fn cfg_edge_violations(&self) -> Vec<CfgEdgeViolation> {
        let g = &self.graph;
        let mut out = Vec::new();
        // distinct (edge cfg, target cfg) pairs are few: judge each once
        let mut memo: HashMap<(String, String, String), Implication> = HashMap::new();
        for e in g.edges.iter().filter(|e| DEPENDENT_KINDS.contains(&e.kind)) {
            let (Some(from), Some(to)) = (g.node(&e.from), g.node(&e.to)) else { continue };
            let (Some(pf), Some(pt)) = (&from.package, &to.package) else { continue };
            if e.covered || pf != pt || to.cfg == CfgPredicate::True {
                continue;
            }
            let Some(model) = self.features.get(pf) else { continue };
            let key = (pf.clone(), e.cfg.render(), to.cfg.render());
            let verdict = memo.entry(key).or_insert_with(|| model.closed_implies(&e.cfg, &to.cfg)).clone();
            if verdict != Implication::Always {
                out.push(CfgEdgeViolation {
                    package: pf.clone(),
                    from: e.from.clone(),
                    to: e.to.clone(),
                    location: e.location.clone(),
                    edge_cfg: e.cfg.clone(),
                    to_cfg: to.cfg.clone(),
                    verdict,
                });
            }
        }
        out
    }
}

/// The items of `nodes` whose own cfg cannot hold without `feature`. The
/// same few cfg predicates repeat across hundreds of items: each distinct
/// one is judged once (scale-run fix: 2 s → ms per feature).
fn gated_items<'n>(model: &FeatureModel, feature: &str, nodes: impl IntoIterator<Item = &'n ItemNode>) -> BTreeSet<String> {
    let mut memo: HashMap<String, bool> = HashMap::new();
    nodes
        .into_iter()
        .filter(|n| *memo.entry(n.cfg.render()).or_insert_with(|| requires_feature(model, &n.cfg, feature)))
        .map(|n| n.id.clone())
        .collect()
}

/// The items of each closure node, computed once per (crate, feature), on
/// one index of the graph's nodes by crate.
struct KeyItems<'k> {
    k: &'k Klepto,
    by_package: HashMap<&'k str, Vec<&'k ItemNode>>,
    memo: HashMap<FeatureKey, Vec<String>>,
}

impl<'k> KeyItems<'k> {
    fn new(k: &'k Klepto) -> Self {
        let mut by_package: HashMap<&str, Vec<&ItemNode>> = HashMap::new();
        for n in k.graph.nodes.values() {
            if let Some(p) = &n.package {
                by_package.entry(p.as_str()).or_default().push(n);
            }
        }
        KeyItems { k, by_package, memo: HashMap::new() }
    }

    /// Sorted ids: a feature's gated items; for the crate itself, the
    /// items no feature of it gates. Empty for a crate without a model.
    fn of(&mut self, key: &FeatureKey) -> Vec<String> {
        if let Some(hit) = self.memo.get(key) {
            return hit.clone();
        }
        let k = self.k;
        let Some(model) = k.features.get(&key.package) else { return Vec::new() };
        let items: Vec<String> = match &key.feature {
            Some(f) => {
                let nodes = self.by_package.get(key.package.as_str()).map(Vec::as_slice).unwrap_or_default();
                gated_items(model, f, nodes.iter().copied()).into_iter().collect()
            }
            None => {
                let mut gated: BTreeSet<String> = BTreeSet::new();
                for f in model.features.keys() {
                    gated.extend(self.of(&FeatureKey::new(&key.package, Some(f))));
                }
                let nodes = self.by_package.get(key.package.as_str()).map(Vec::as_slice).unwrap_or_default();
                let mut ids: Vec<String> = nodes.iter().filter(|n| !gated.contains(&n.id)).map(|n| n.id.clone()).collect();
                ids.sort();
                ids
            }
        };
        self.memo.insert(key.clone(), items.clone());
        items
    }
}
