//! Where the time goes on a real crate: per-feature `affected_by_feature`,
//! `cfg_edge_violations`, `ungated_dep_uses`, `dependents_of` over every
//! node, plus a few model facts (dependency predicates, orphan modules).
//!
//!     cargo run --release --example hotspots -- <crate root> <crate name>

use klepto::Klepto;
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let root = args.next().unwrap_or_else(|| ".".into());
    let name = args.next().unwrap_or_else(|| "crate".into());
    let k = Klepto::new(&name)
        .scan_workspace_root(&root)
        .expect("cargo metadata")
        .ignore_parse_errors(true)
        .parse()
        .expect("parse");
    let pkg = name.replace('-', "_");

    // optional third argument: explain one node (cfg, location, module cfg)
    if let Some(id) = args.next() {
        match k.graph.node(&id) {
            Some(n) => {
                println!("node {id}: kind {:?} cfg {} at {:?}", n.kind, n.cfg.render(), n.location.as_ref().map(|l| (l.path.display().to_string(), l.line)));
                if let Some(m) = k.modules.get(&n.module) {
                    println!("  module {} cfg {} file {}", m.path, m.cfg.render(), m.file.display());
                }
                for d in k.items.iter().filter(|d| d.id == id) {
                    println!("  declared {:?} parent {:?} cfg {}", d.location.line, d.parent, d.cfg.render());
                }
            }
            None => println!("no node {id}"),
        }
    }

    if let Some(model) = k.feature_model(&pkg) {
        println!("features: {:?}", model.features.keys().collect::<Vec<_>>());
        for d in &model.dependencies {
            println!(
                "  dep {} kind={:?} optional={} target={:?} -> predicate {}",
                d.name,
                d.kind,
                d.optional,
                d.target.as_ref().map(|t| t.render()),
                model.dep_predicate(&d.name).render()
            );
        }
        for f in model.features.keys() {
            let t = Instant::now();
            let a = k.affected_by_feature(&pkg, f);
            println!(
                "  affected_by_feature({f}): gated {} lost_dep {} transitive {} in {:.2}s",
                a.gated.len(),
                a.uses_lost_dep.len(),
                a.transitive.len(),
                t.elapsed().as_secs_f64()
            );
        }
    }

    let t = Instant::now();
    let v = k.cfg_edge_violations();
    println!("cfg_edge_violations: {} in {:.2}s", v.len(), t.elapsed().as_secs_f64());
    for x in v.iter().take(5) {
        println!("  {} -> {} edge {} target {}", x.from, x.to, x.edge_cfg.render(), x.to_cfg.render());
    }

    let t = Instant::now();
    let u = k.ungated_dep_uses();
    println!("ungated_dep_uses: {} in {:.2}s", u.len(), t.elapsed().as_secs_f64());
    for x in u.iter().take(5) {
        println!("  {} {} site {} needs {}", x.site.dep, x.site.scope, x.site_cfg.render(), x.dep_predicate.render());
    }

    let t = Instant::now();
    let mut total = 0usize;
    for id in k.graph.nodes.keys() {
        total += k.graph.dependents_of(id).len();
    }
    println!("dependents_of over {} nodes: {} edges counted in {:.2}s", k.graph.nodes.len(), total, t.elapsed().as_secs_f64());

    let orphans: Vec<&str> = k.modules.modules.values().filter(|m| m.orphan).map(|m| m.path.as_str()).collect();
    println!("orphan modules {}: {:?}", orphans.len(), orphans.iter().take(12).collect::<Vec<_>>());
    let mut unknown: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for u in &k.graph.unresolved {
        if let klepto::Resolution::Unresolved { raw, reason: klepto::UnresolvedReason::UnknownName, .. } = &u.resolution {
            let head = raw.split("::").next().unwrap_or(raw).to_string();
            *unknown.entry(head).or_default() += 1;
        }
    }
    let mut top: Vec<(String, usize)> = unknown.into_iter().collect();
    top.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    println!("top UnknownName heads: {:?}", top.iter().take(15).collect::<Vec<_>>());
    let distinct_cfgs: std::collections::BTreeSet<String> = k.graph.nodes.values().map(|n| n.cfg.render()).collect();
    println!("distinct node cfgs: {}", distinct_cfgs.len());
}
