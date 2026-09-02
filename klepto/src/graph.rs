//! Phase 3: the item graph — one node per item, one edge per resolved
//! reference — and the closure queries on it.
//!
//! Rules N1–N7, D1–D9, Q1–Q7 in `PHASE3_RULES.md`.

use crate::cfg::{CfgPredicate, Implication};
use crate::features::{dep_lost_without, requires_feature};
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
    /// Effective cfg (Phase 2); `True` for externals.
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
    /// Rule R14: the target is one of cfg-twin bindings that together cover
    /// every build; not judged by `cfg_edge_violations`.
    #[serde(default)]
    pub covered: bool,
}

/// An occurrence that could not become an edge (rule D5).
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
    /// Ids declared more than once (rule N6); the first declaration is kept.
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

    /// Rule D4: the exact node, or — for a path with at least two segments —
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

    /// Q1: what `id` uses directly (Reference, Import, ReExport edges).
    pub fn dependencies_of(&self, id: &str) -> BTreeSet<String> {
        self.edges
            .iter()
            .filter(|e| e.from == id && DEPENDENCY_KINDS.contains(&e.kind))
            .map(|e| e.to.clone())
            .collect()
    }

    /// Q2: what uses `id` directly (Reference and ReExport edges).
    pub fn dependents_of(&self, id: &str) -> BTreeSet<String> {
        self.edges
            .iter()
            .filter(|e| e.to == id && DEPENDENT_KINDS.contains(&e.kind))
            .map(|e| e.from.clone())
            .collect()
    }

    /// Q3: everything that transitively uses any of `seeds` (seeds excluded).
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

    /// Q4: everything `id` transitively uses (`id` excluded).
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
    /// external nodes of that crate (rule Q5).
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
    /// Items whose own cfg cannot hold without the feature (Q6 only).
    pub gated: Vec<String>,
    /// Items (not in `gated`) that directly reference a dependency that goes away.
    pub uses_lost_dep: Vec<String>,
    /// Everything else reached through Reference / ReExport edges.
    pub transitive: Vec<String>,
}

/// Rule Q7: an item references another item that is not compiled in some
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
// builder (rules N1–N7, D1–D9)
// ---------------------------------------------------------------------------

struct Builder<'k> {
    k: &'k Klepto,
    g: ItemGraph,
    seen_edges: HashSet<(String, String, EdgeKind, FileLocation)>,
    /// Rule R14: whether the occurrence being turned into edges is covered.
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

    /// Rule N3.
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
            return; // D6
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

    /// Rules D1/D2/D4/D5 for one resolved occurrence.
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

        // N1: module nodes, D3: module containment
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

        // N1/N4/N6: item nodes
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
        // N6 amendment: an item declared several times under cfgs that
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
        // D3: containment of items
        for (d, is_first) in k.items.iter().zip(first) {
            if !is_first {
                continue;
            }
            let parent = d.parent.clone().unwrap_or_else(|| d.module.clone());
            self.add_edge(&parent, &d.id, EdgeKind::Contains, &d.location, &d.cfg);
        }

        // D1: references
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
        // D1 amendment (scale run): a built-in attribute's own name (`cfg`,
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
        // D5 / Phase 5 G1: a method call is an edge only when its receiver was typed
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
        // D2: imports and re-exports
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

/// Rules N1–N7 and D1–D9.
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

    /// Q5: items lost when dependency `dep` goes away.
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

    /// Q6: items lost when `feature` of `package` is off.
    pub fn affected_by_feature(&self, package: &str, feature: &str) -> Affected {
        let Some(model) = self.features.get(package) else {
            return Affected::default();
        };
        let g = &self.graph;

        // the same few cfg predicates repeat across hundreds of items:
        // judge each distinct one once (scale-run fix: 2 s → ms per feature)
        let mut memo: HashMap<String, bool> = HashMap::new();
        let gated: BTreeSet<String> = g
            .nodes
            .values()
            .filter(|n| {
                n.package.as_deref() == Some(package)
                    && *memo.entry(n.cfg.render()).or_insert_with(|| requires_feature(model, &n.cfg, feature))
            })
            .map(|n| n.id.clone())
            .collect();

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

    /// Q7: item-to-item references whose cfg does not guarantee the target
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
