//! Sanity check on a real workspace: receipt size and digest, KG
//! record counts, and the pipeline verdict under a quiet policy.
//!
//!     cargo run --example receipt_stats -- <workspace root> <crate name>

use klepto::{receipt::kg, run_pipeline, Klepto, Policy, Receipt};
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

    let r = Receipt::from_klepto(&k);
    let json = r.to_json_compact();
    println!(
        "receipt v{} digest {}.. bytes {} files {} items {} edges {} unresolved {} dep_uses {} skipped {}",
        r.version,
        &r.digest[..12],
        json.len(),
        r.files.len(),
        r.items.len(),
        r.edges.len(),
        r.unresolved.len(),
        r.dep_uses.len(),
        r.skipped_files.len()
    );
    let again = Receipt::from_klepto(&k);
    println!("deterministic: {}", again.to_json_compact() == json);

    let b = kg::export(&k);
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for rec in &b.knowledge {
        *kinds.entry(rec["kind"].as_str().unwrap_or("?").to_string()).or_default() += 1;
    }
    println!("kg records {}: {:?}", b.knowledge.len(), kinds);
    let mut classes: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for c in &b.challenges {
        let e = classes.entry(c["class"].as_str().unwrap_or("?").to_string()).or_default();
        e.0 += 1;
        if c["split"] == "heldout" {
            e.1 += 1;
        }
    }
    println!("challenges {} (class: total/heldout): {:?}", b.challenges.len(), classes);

    let policy = Policy::parse("[levels]\nKLEP001 = \"allow\"\nKLEP012 = \"allow\"\n").expect("policy");
    let v = run_pipeline(&k, &policy);
    println!("{}", v.summary());
}
