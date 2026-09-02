//! Knowledge-graph export in langbrain's record vocabulary: one JSON object
//! per record with a `kind`,
//! an `id`, a plain-language `description`, `from`/`to` on edges, and
//! challenges with an exact `expected` answer, a SHA-256 `digest` and a
//! deterministic train/heldout `split`. klepto never reads langbrain; it
//! only speaks the same shape so both feed one world model.

use crate::resolve::features::{DepKind, FeatureEnable, FeatureModel};
use crate::graph::{EdgeKind, NodeKind};
use crate::klepto::Klepto;
use crate::model::Resolution;
use crate::receipt::{scanned_packages, Relocator, RECEIPT_VERSION};
use crate::resolve::{join_key, CtxParts};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// Challenge classes klepto emits (`same_shape` comes from pattern mining).
pub const CHALLENGE_CLASSES: &[&str] = &["dependents_of", "resolve_path", "lost_without_feature", "receiver_type", "same_shape"];

/// Node kinds and edge kinds (`pattern` and `realizes` come from pattern mining).
pub const NODE_KINDS: &[&str] = &["package", "module", "item", "feature", "crate", "constant", "pattern"];
pub const EDGE_KINDS: &[&str] = &["contains", "references", "imports", "reexports", "implements", "impl_of", "depends_on", "enables", "gated_by", "realizes"];

/// Largest `dependents_of` answer a challenge asks for.
const MAX_DEPENDENTS: usize = 20;

#[derive(Debug, Clone, Default)]
pub struct KgBundle {
    pub knowledge: Vec<Value>,
    pub challenges: Vec<Value>,
}

/// SHA-256 hex, the world-model digest convention.
pub fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Deterministic 25% heldout split keyed on the digest's first nibble
/// (langbrain's rule, reproduced so the splits line up).
pub fn split_for(digest: &str) -> &'static str {
    match u8::from_str_radix(&digest[0..1], 16).unwrap_or(0) {
        0..=3 => "heldout",
        _ => "train",
    }
}

/// One record per line, no trailing whitespace.
pub fn to_ndjson(records: &[Value]) -> String {
    let mut out = String::new();
    for r in records {
        out.push_str(&serde_json::to_string(r).unwrap_or_default());
        out.push('\n');
    }
    out
}

fn bare(id: &str) -> &str {
    id.split_once('/').map(|(_, r)| r).unwrap_or(id)
}

fn crate_root(id: &str) -> &str {
    id.split("::").next().unwrap_or(id)
}

fn last_segment(id: &str) -> &str {
    id.rsplit("::").next().unwrap_or(id)
}

fn kind_name(k: NodeKind) -> &'static str {
    match k {
        NodeKind::Fn => "fn",
        NodeKind::Method => "method",
        NodeKind::Struct => "struct",
        NodeKind::Enum => "enum",
        NodeKind::Union => "union",
        NodeKind::Trait => "trait",
        NodeKind::TypeAlias => "type_alias",
        NodeKind::Const => "const",
        NodeKind::Static => "static",
        NodeKind::Macro => "macro",
        NodeKind::Module => "module",
        NodeKind::Impl => "impl",
        NodeKind::External => "external",
    }
}

fn dep_kind_name(k: DepKind) -> &'static str {
    match k {
        DepKind::Normal => "normal",
        DepKind::Dev => "dev",
        DepKind::Build => "build",
    }
}

/// `default` and everything it enables transitively.
fn default_features(model: &FeatureModel) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut stack = vec!["default".to_string()];
    while let Some(f) = stack.pop() {
        if !out.insert(f.clone()) {
            continue;
        }
        if let Some(info) = model.features.get(&f) {
            for e in &info.enables {
                if let FeatureEnable::Feature(g) = e {
                    stack.push(g.clone());
                }
            }
        }
    }
    out
}

/// One challenge record.
fn challenge(class: &str, package: &str, prompt: String, expected: String, extra: Value) -> Value {
    let digest = sha256_hex(&format!("klepto|{class}|{package}|{prompt}|{expected}"));
    let mut v = json!({
        "kind": "challenge",
        "class": class,
        "id": format!("chal/{class}/{package}/{}", &digest[0..8]),
        "package": package,
        "language": "rust",
        "prompt": prompt,
        "expected": expected,
        "split": split_for(&digest),
        "frontier": "available",
        "description": format!("klepto challenge ({class}) on package '{package}': {prompt}"),
        "digest": digest,
    });
    if let Value::Object(m) = extra {
        for (k, x) in m {
            v[k] = x;
        }
    }
    v
}

struct Kg<'k> {
    k: &'k Klepto,
    rl: Relocator,
    scanned: BTreeSet<String>,
    nodes: BTreeMap<String, Value>,
    edges: BTreeMap<(String, String, String), Value>,
    /// patterns (default options).
    report: crate::patterns::PatternReport,
}

/// What a level means, for prompts.
fn level_meaning(level: crate::patterns::ShapeLevel) -> &'static str {
    use crate::patterns::ShapeLevel::*;
    match level {
        Exact => "the same tokens",
        Renamed => "the same tokens once locals and parameters are renamed",
        Literals => "the same tokens once locals are renamed and literals abstracted",
        Types => "the same tokens once locals are renamed and literals and type names abstracted",
        Callees => "the same tokens once locals are renamed and literals, type names and callees abstracted",
    }
}

/// `pattern/<level>/<hash16>`.
fn pattern_id(c: &crate::patterns::Cluster) -> String {
    format!("pattern/{}/{}", c.level.name(), &c.hash[..16.min(c.hash.len())])
}

impl<'k> Kg<'k> {
    fn node(&mut self, id: &str, v: Value) {
        self.nodes.entry(id.to_string()).or_insert(v);
    }

    /// Edges fold by `(kind, from, to)`; both ends must exist.
    #[allow(clippy::too_many_arguments)]
    fn edge(&mut self, kind: &str, from: &str, to: &str, file: Option<String>, line: Option<u32>, description: String, extra: Value) {
        debug_assert!(self.nodes.contains_key(from), "K5: {from}");
        debug_assert!(self.nodes.contains_key(to), "K5: {to}");
        let key = (kind.to_string(), from.to_string(), to.to_string());
        if let Some(e) = self.edges.get_mut(&key) {
            let n = e["occurrences"].as_u64().unwrap_or(1) + 1;
            e["occurrences"] = json!(n);
            return;
        }
        let mut v = json!({
            "kind": kind,
            "id": format!("{kind}/{}/{}", bare(from), bare(to)),
            "from": from,
            "to": to,
            "description": description,
            "occurrences": 1,
        });
        if let Some(f) = file {
            v["file"] = json!(f);
        }
        if let Some(l) = line {
            v["line"] = json!(l);
        }
        if let Value::Object(m) = extra {
            for (k, x) in m {
                v[k] = x;
            }
        }
        self.edges.insert(key, v);
    }

    fn ensure_crate(&mut self, name: &str) -> String {
        let id = format!("crate/{name}");
        self.node(
            &id,
            json!({
                "kind": "crate",
                "id": id,
                "name": name,
                "description": format!("External crate '{name}': not scanned; its items appear only where referenced."),
            }),
        );
        id
    }

    /// The item node for an external id, created once under its crate node.
    fn ensure_external_item(&mut self, id: &str) -> String {
        let nid = format!("item/{id}");
        if self.nodes.contains_key(&nid) {
            return nid;
        }
        let root = crate_root(id).to_string();
        let crate_id = self.ensure_crate(&root);
        self.node(
            &nid,
            json!({
                "kind": "item",
                "id": nid,
                "item_kind": "external",
                "name": last_segment(id),
                "package": root,
                "module": root,
                "description": format!("External item '{id}' of crate '{root}' (not scanned)."),
            }),
        );
        self.edge("contains", &crate_id, &nid, None, None, format!("'{root}' contains '{id}'."), json!({}));
        nid
    }

    fn ensure_feature(&mut self, package: &str, name: &str, declared: bool, default: bool, implicit: bool, enables: &[FeatureEnable]) -> String {
        let id = format!("feat/{package}/{name}");
        if self.nodes.contains_key(&id) {
            return id;
        }
        let description = if declared {
            let list: Vec<String> = enables.iter().map(crate::resolve::features::render_enable).collect();
            format!(
                "Feature '{name}' of package '{package}' enables [{}]; {}{}.",
                list.join(", "),
                if default { "on by default" } else { "not on by default" },
                if implicit { "; implicit feature of an optional dependency" } else { "" }
            )
        } else {
            format!("Feature '{name}' is named by a #[cfg] in package '{package}' but not declared in its manifest.")
        };
        self.node(
            &id,
            json!({
                "kind": "feature",
                "id": id,
                "package": package,
                "name": name,
                "declared": declared,
                "default": default,
                "implicit": implicit,
                "description": description,
            }),
        );
        id
    }

    /// `pkg/<name>` for a scanned package, else `crate/<name>`.
    fn dep_target(&mut self, name: &str) -> String {
        if self.scanned.contains(name) {
            format!("pkg/{name}")
        } else {
            self.ensure_crate(name)
        }
    }

    /// The node id of a graph node: `mod/` for modules, `item/` otherwise.
    fn nid(&self, id: &str) -> String {
        match self.k.graph.nodes.get(id) {
            Some(n) if n.kind == NodeKind::Module => format!("mod/{id}"),
            _ => format!("item/{id}"),
        }
    }

    fn build(&mut self) {
        let k = self.k;
        let g = &k.graph;

        // packages
        for name in self.scanned.clone() {
            let modules = k.modules.modules.values().filter(|m| m.package == name).count();
            let items = g.nodes.values().filter(|n| n.package.as_deref() == Some(name.as_str()) && n.kind != NodeKind::Module).count();
            let id = format!("pkg/{name}");
            self.node(
                &id,
                json!({
                    "kind": "package",
                    "id": id,
                    "name": name,
                    "language": "rust",
                    "modules": modules,
                    "items": items,
                    "description": format!("Package '{name}' (Rust): {modules} modules, {items} items."),
                }),
            );
        }

        // modules
        for (key, m) in &k.modules.modules {
            let file = self.rl.rel(&m.file);
            let id = format!("mod/{key}");
            let mut notes = Vec::new();
            if m.inline {
                notes.push("inline".to_string());
            }
            if m.orphan {
                notes.push("orphan (no `mod` declaration reaches it)".to_string());
            }
            let notes = if notes.is_empty() { String::new() } else { format!("; {}", notes.join(", ")) };
            self.node(
                &id,
                json!({
                    "kind": "module",
                    "id": id,
                    "package": m.package,
                    "file": file,
                    "inline": m.inline,
                    "orphan": m.orphan,
                    "public": m.is_pub,
                    "cfg": m.cfg.render(),
                    "description": format!("Module '{key}' of package '{}' in {file}{notes}; cfg {}.", m.package, m.cfg.render()),
                }),
            );
        }

        // items
        for (id, n) in &g.nodes {
            if n.kind == NodeKind::Module {
                continue;
            }
            if n.kind == NodeKind::External {
                self.ensure_external_item(id);
                continue;
            }
            let nid = format!("item/{id}");
            let file = n.location.as_ref().map(|l| self.rl.rel(&l.path));
            let line = n.location.as_ref().and_then(|l| l.line);
            let kind = kind_name(n.kind);
            let package = n.package.clone().unwrap_or_default();
            let at = match (&file, line) {
                (Some(f), Some(l)) => format!(" at {f}:{l}"),
                (Some(f), None) => format!(" in {f}"),
                _ => String::new(),
            };
            self.node(
                &nid,
                json!({
                    "kind": "item",
                    "id": nid,
                    "item_kind": kind,
                    "name": n.name,
                    "package": package,
                    "module": n.module,
                    "file": file,
                    "line": line,
                    "cfg": n.cfg.render(),
                    "public": n.is_public,
                    "description": format!(
                        "{kind} '{}' — item '{id}' in module '{}'{at}; {}; cfg {}.",
                        n.name, n.module, if n.is_public { "public" } else { "private" }, n.cfg.render()
                    ),
                }),
            );
        }

        // features and crates, depends_on / enables
        for (pkg, model) in &k.features {
            let defaults = default_features(model);
            for (name, info) in &model.features {
                self.ensure_feature(pkg, name, true, defaults.contains(name), info.implicit, &info.enables);
            }
            for d in &model.dependencies {
                let to = self.dep_target(&d.name);
                let from = format!("pkg/{pkg}");
                let target = d.target.as_ref().map(|t| format!(", target cfg {}", t.render())).unwrap_or_default();
                self.edge(
                    "depends_on",
                    &from,
                    &to,
                    None,
                    None,
                    format!(
                        "'{pkg}' depends on '{}' ({}{}{target}).",
                        d.name,
                        dep_kind_name(d.kind),
                        if d.optional { ", optional" } else { "" }
                    ),
                    json!({ "optional": d.optional, "dep_kind": dep_kind_name(d.kind), "package_name": d.package }),
                );
            }
            for (name, info) in &model.features {
                let from = format!("feat/{pkg}/{name}");
                for e in &info.enables {
                    match e {
                        FeatureEnable::Feature(f) => {
                            let to = self.ensure_feature(pkg, f, false, false, false, &[]);
                            self.edge("enables", &from, &to, None, None, format!("Feature '{pkg}/{name}' enables feature '{f}'."), json!({}));
                        }
                        FeatureEnable::Dep(d) => {
                            let to = self.dep_target(d);
                            self.edge("enables", &from, &to, None, None, format!("Feature '{pkg}/{name}' enables dependency '{d}'."), json!({}));
                        }
                        FeatureEnable::DepFeature { dep, feature, weak } => {
                            let to = self.dep_target(dep);
                            self.edge(
                                "enables",
                                &from,
                                &to,
                                None,
                                None,
                                format!(
                                    "Feature '{pkg}/{name}' enables feature '{feature}' of dependency '{dep}'{}.",
                                    if *weak { " (weak: only if the dependency is already on)" } else { "" }
                                ),
                                json!({ "feature": feature, "weak": weak }),
                            );
                        }
                    }
                }
            }
        }
        // folder mode: declared deps without a feature model
        for (pkg, deps) in &k.package_deps {
            if k.features.contains_key(pkg) {
                continue;
            }
            for d in deps {
                let to = self.dep_target(d);
                let from = format!("pkg/{pkg}");
                self.edge("depends_on", &from, &to, None, None, format!("'{pkg}' depends on '{d}'."), json!({}));
            }
        }

        // contains: package → root module, then the graph's own edges
        for name in self.scanned.clone() {
            let root = format!("mod/{name}");
            if self.nodes.contains_key(&root) {
                let from = format!("pkg/{name}");
                self.edge("contains", &from, &root, None, None, format!("'{name}' contains its root module '{name}'."), json!({}));
            }
        }
        for e in &g.edges {
            let from = self.nid(&e.from);
            let to = self.nid(&e.to);
            let file = self.rl.rel(&e.location.path);
            let (kind, verb) = match e.kind {
                EdgeKind::Contains => ("contains", "contains"),
                EdgeKind::Reference => ("references", "references"),
                EdgeKind::Import => ("imports", "imports"),
                EdgeKind::ReExport => ("reexports", "re-exports"),
            };
            let at = match e.location.line {
                Some(l) => format!(" at {file}:{l}"),
                None => format!(" in {file}"),
            };
            let cfg = if e.cfg == crate::resolve::cfg::CfgPredicate::True { String::new() } else { format!(" (cfg {})", e.cfg.render()) };
            self.edge(
                kind,
                &from,
                &to,
                Some(file.clone()),
                e.location.line,
                format!("'{}' {verb} '{}'{at}{cfg}.", e.from, e.to),
                json!({ "cfg": e.cfg.render() }),
            );
        }

        // implements / impl_of
        for i in &k.impls {
            let from = format!("item/{}", i.id);
            if !self.nodes.contains_key(&from) {
                continue;
            }
            if let Some(tr) = i.trait_.as_ref().and_then(|t| t.mentions.first()) {
                let to = match tr {
                    Resolution::Local(t) | Resolution::External(t) => {
                        let local = format!("item/{t}");
                        if self.nodes.contains_key(&local) { local } else { self.ensure_external_item(t) }
                    }
                    _ => continue,
                };
                self.edge("implements", &from, &to, None, None, format!("'{}' implements trait '{}'.", i.id, bare(&to)), json!({}));
            }
            if let Some(Resolution::Local(t)) = i.self_ty.mentions.first() {
                let to = format!("item/{t}");
                if self.nodes.contains_key(&to) {
                    self.edge("impl_of", &from, &to, None, None, format!("'{}' is an impl block of '{t}'.", i.id), json!({}));
                }
            }
        }

        // gated_by
        for (id, n) in &g.nodes {
            if n.kind == NodeKind::Module || n.kind == NodeKind::External {
                continue;
            }
            let Some(pkg) = n.package.clone() else { continue };
            for f in n.cfg.features() {
                let from = format!("item/{id}");
                let to = self.ensure_feature(&pkg, &f, false, false, false, &[]);
                self.edge(
                    "gated_by",
                    &from,
                    &to,
                    None,
                    None,
                    format!("'{id}' is gated by cfg `{}`, which names feature '{f}'.", n.cfg.render()),
                    json!({ "cfg": n.cfg.render() }),
                );
            }
        }

        // patterns and realizes
        let report = self.report.clone();
        for c in report.clusters.iter().filter(|c| c.tightest) {
            let id = pattern_id(c);
            let signature = report.shape(&c.exemplar).map(|s| s.shapes[&c.level].clone()).unwrap_or_default();
            let signature: String = signature.chars().take(500).collect();
            self.node(
                &id,
                json!({
                    "kind": "pattern",
                    "id": id,
                    "level": c.level.name(),
                    "hash": c.hash,
                    "signature": signature,
                    "exemplar": c.exemplar,
                    "support": c.members.len(),
                    "tightest": true,
                    "description": format!(
                        "Function shape shared by {} functions at the '{}' level ({}); exemplar '{}'.",
                        c.members.len(), c.level.name(), level_meaning(c.level), c.exemplar
                    ),
                }),
            );
            for m in &c.members {
                let from = format!("item/{m}");
                if !self.nodes.contains_key(&from) {
                    continue;
                }
                self.edge("realizes", &from, &id, None, None, format!("'{m}' realizes the '{}' pattern '{}'.", c.level.name(), bare(&id)), json!({}));
            }
        }

        // constants
        self.node(
            "const/klepto/receipt_version",
            json!({
                "kind": "constant",
                "id": "const/klepto/receipt_version",
                "name": "receipt_version",
                "value": RECEIPT_VERSION,
                "description": format!("klepto receipt format version {RECEIPT_VERSION}."),
            }),
        );
        let module_pkg: BTreeMap<&str, &str> = k.modules.modules.values().map(|m| (m.path.as_str(), m.package.as_str())).collect();
        let pkg_of_from = |from: &str| -> Option<String> {
            g.nodes.get(from).and_then(|n| n.package.clone()).or_else(|| module_pkg.get(from).map(|s| s.to_string()))
        };
        for name in self.scanned.clone() {
            let items = g.nodes.values().filter(|n| n.package.as_deref() == Some(name.as_str()) && n.kind != NodeKind::Module).count();
            let edges = g.edges.iter().filter(|e| pkg_of_from(&e.from).as_deref() == Some(name.as_str())).count();
            let unresolved = g.unresolved.iter().filter(|u| pkg_of_from(&u.from).as_deref() == Some(name.as_str())).count();
            let parts = CtxParts::new(k);
            let files = k.files.iter().filter(|f| parts.package_of(&f.path) == name).count();
            for (what, value) in [("items", items), ("edges", edges), ("unresolved", unresolved), ("files", files)] {
                let id = format!("const/{name}/{what}");
                self.node(
                    &id,
                    json!({
                        "kind": "constant",
                        "id": id,
                        "package": name,
                        "name": what,
                        "value": value,
                        "description": format!("Package '{name}' has {value} {what}."),
                    }),
                );
            }
        }
    }

    /// The challenge records: questions with an exact expected answer,
    /// digested and split.
    fn challenges(&self) -> Vec<Value> {
        let k = self.k;
        let g = &k.graph;
        let parts = CtxParts::new(k);
        let mut out: BTreeMap<String, Value> = BTreeMap::new();
        let mut push = |v: Value| {
            let id = v["id"].as_str().unwrap_or_default().to_string();
            out.entry(id).or_insert(v);
        };

        // one reverse index instead of a scan of every edge per node
        // (scale run: syn's 4.7k nodes × 21k edges took 15 s)
        let mut dependents: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
        for e in g.edges.iter().filter(|e| matches!(e.kind, EdgeKind::Reference | EdgeKind::ReExport)) {
            if e.from != e.to {
                dependents.entry(e.to.as_str()).or_default().insert(e.from.clone());
            }
        }
        for (id, n) in &g.nodes {
            if n.kind == NodeKind::Module || n.kind == NodeKind::External {
                continue;
            }
            let Some(pkg) = n.package.as_deref() else { continue };
            let deps: Vec<String> = dependents.get(id.as_str()).map(|s| s.iter().cloned().collect()).unwrap_or_default();
            if deps.is_empty() || deps.len() > MAX_DEPENDENTS {
                continue;
            }
            let prompt = format!(
                "In package '{pkg}', which items directly reference '{id}' (by reference or re-export)? Answer with the sorted item ids as a JSON array."
            );
            let expected = serde_json::to_string(&deps).unwrap_or_default();
            push(challenge("dependents_of", pkg, prompt, expected, json!({ "item": id })));
        }

        for imp in &k.imports {
            if imp.enclosing_fn.is_some() {
                continue;
            }
            let canonical = match &imp.resolution {
                Some(Resolution::Local(s)) | Some(Resolution::External(s)) => s.clone(),
                _ => continue,
            };
            let (pkg, module) = parts.parts(&imp.location.path, &imp.module_path);
            let prompt = format!(
                "In module '{module}' of package '{pkg}', what canonical item does `use {}` name? Answer with the canonical path.",
                imp.full_path
            );
            push(challenge("resolve_path", &pkg, prompt, canonical, json!({ "module": module, "path": imp.full_path })));
        }

        for (pkg, model) in &k.features {
            for name in model.features.keys() {
                let a = k.affected_by_feature(pkg, name);
                let mut lost: BTreeSet<String> = BTreeSet::new();
                lost.extend(a.gated.iter().cloned());
                lost.extend(a.uses_lost_dep.iter().cloned());
                lost.extend(a.transitive.iter().cloned());
                if lost.is_empty() {
                    continue;
                }
                let lost: Vec<String> = lost.into_iter().collect();
                let prompt = format!(
                    "In package '{pkg}', which items become unavailable when feature '{name}' is off (every other declared feature free to be on or off)? Answer with the sorted item ids as a JSON array."
                );
                let expected = serde_json::to_string(&lost).unwrap_or_default();
                push(challenge("lost_without_feature", pkg, prompt, expected, json!({ "feature": name })));
            }
        }

        for c in &k.calls {
            if !c.is_method {
                continue;
            }
            let Some(Resolution::Local(t)) = &c.receiver_type else { continue };
            let Some(line) = c.location.line else { continue };
            let (pkg, module) = parts.parts(&c.location.path, &c.module_path);
            let function = c.enclosing_fn.clone().unwrap_or_else(|| module.clone());
            let file = self.rl.rel(&c.location.path);
            let prompt = format!(
                "In function '{function}' ({file}:{line}), the receiver of the call `.{}()` has which type? Answer with the type's item id.",
                c.callee
            );
            push(challenge(
                "receiver_type",
                &pkg,
                prompt,
                t.clone(),
                json!({ "function": function, "method": c.callee, "file": file, "line": line }),
            ));
        }

        // same_shape
        for c in self.report.clusters.iter().filter(|c| c.tightest) {
            let Some(pkg) = g.nodes.get(&c.exemplar).and_then(|n| n.package.clone()) else { continue };
            let others: Vec<String> = c.members.iter().filter(|m| **m != c.exemplar).cloned().collect();
            let prompt = format!(
                "In package '{pkg}', which other functions have the same shape as '{}' at the '{}' level ({})? Answer with the sorted item ids as a JSON array.",
                c.exemplar,
                c.level.name(),
                level_meaning(c.level)
            );
            let expected = serde_json::to_string(&others).unwrap_or_default();
            push(challenge("same_shape", &pkg, prompt, expected, json!({ "item": c.exemplar, "level": c.level.name() })));
        }

        out.into_values().collect()
    }
}

/// Exports the scan as a knowledge-graph bundle: nodes, edges and
/// challenges, sorted by id.
pub fn export(k: &Klepto) -> KgBundle {
    let mut kg = Kg {
        k,
        rl: Relocator::new(k.root.as_deref()),
        scanned: scanned_packages(k),
        nodes: BTreeMap::new(),
        edges: BTreeMap::new(),
        report: k.patterns(),
    };
    kg.build();
    let challenges = kg.challenges();
    // Sorted by id
    let mut knowledge: Vec<Value> = kg.nodes.into_values().chain(kg.edges.into_values()).collect();
    knowledge.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    let _ = join_key; // module keys come from the graph already
    KgBundle { knowledge, challenges }
}
