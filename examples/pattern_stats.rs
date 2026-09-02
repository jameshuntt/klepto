//! Sanity check on a real workspace: shapes, tightest clusters per
//! level, candidates per kind, impl mining, and the heaviest proposals.
//!
//!     cargo run --example pattern_stats -- <workspace root> <crate name>

use klepto::{CandidateKind, Klepto, ShapeLevel, SHAPE_LEVELS};
use std::collections::BTreeMap;

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
    let r = k.patterns();

    println!("fns {} shaped {} clusters {} tightest {}", k.functions.len(), r.shapes.len(), r.clusters.len(), r.clusters.iter().filter(|c| c.tightest).count());
    let mut per_level: BTreeMap<ShapeLevel, usize> = BTreeMap::new();
    for c in r.clusters.iter().filter(|c| c.tightest) {
        *per_level.entry(c.level).or_default() += 1;
    }
    println!("tightest per level: {:?}", SHAPE_LEVELS.iter().map(|l| (l.name(), per_level.get(l).copied().unwrap_or(0))).collect::<Vec<_>>());
    let mut per_kind: BTreeMap<String, usize> = BTreeMap::new();
    for c in &r.candidates {
        *per_kind.entry(format!("{:?}", c.kind)).or_default() += 1;
    }
    println!("candidates {}: {:?}", r.candidates.len(), per_kind);
    println!("shared method sets {} repeated impls {}", r.shared_method_sets.len(), r.repeated_impls.len());
    for s in r.shared_method_sets.iter().take(3) {
        println!("  shared {:?} by {:?}", s.methods, s.types);
    }
    for c in r.candidates.iter().take(6) {
        let params: Vec<String> = c.params.iter().map(|p| format!("{}={}", p.placeholder, p.fillers.join("|"))).collect();
        println!("  [{:?} w={}] {} :: {}{}", c.kind, c.weight, c.members.join(", "), c.note, if params.is_empty() { String::new() } else { format!(" params {}", params.join(" ")) });
        if c.kind == CandidateKind::GenericFn {
            println!("    required: {:?}", c.required_methods);
        }
    }
    let mut heavy: Vec<(&String, &klepto::FnMetrics)> = r.metrics.iter().collect();
    heavy.sort_by_key(|(_, m)| std::cmp::Reverse(m.branches));
    for (id, m) in heavy.iter().take(3) {
        println!("  branches {} lines {} depth {} calls {} fan_in {} fan_out {}: {id}", m.branches, m.lines, m.max_depth, m.calls, m.fan_in, m.fan_out);
    }
}
