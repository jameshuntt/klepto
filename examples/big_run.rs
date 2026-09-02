//! Scale check on a real crate: counts, unresolved reasons, receiver-typing
//! rate, receipt and KG sizes, pattern counts, pipeline verdict, timings.
//!
//!     cargo run --release --example big_run -- <crate root> <crate name>

use klepto::{receipt::kg, run_pipeline, Klepto, Policy, Receipt, Resolution};
use std::collections::BTreeMap;
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let root = args.next().unwrap_or_else(|| ".".into());
    let name = args.next().unwrap_or_else(|| "crate".into());

    let t = Instant::now();
    let k = Klepto::new(&name)
        .scan_workspace_root(&root)
        .expect("cargo metadata")
        .ignore_parse_errors(true)
        .exclude_generated()
        .expect("globs")
        .parse()
        .expect("parse");
    let t_parse = t.elapsed();

    let mut by_reason: BTreeMap<String, usize> = BTreeMap::new();
    for u in &k.graph.unresolved {
        let key = match &u.resolution {
            Resolution::Unresolved { reason, .. } => format!("{reason:?}"),
            Resolution::Local(_) => "UnplacedLocal".into(),
            Resolution::External(_) => "UnplacedExternal".into(),
        };
        *by_reason.entry(key).or_default() += 1;
    }
    let methods = k.calls.iter().filter(|c| c.is_method).count();
    let typed = k.calls.iter().filter(|c| c.is_method && c.receiver_type.is_some()).count();
    let local_paths = k.paths.iter().filter(|p| matches!(p.resolution, Some(Resolution::Local(_)))).count();
    let ext_paths = k.paths.iter().filter(|p| matches!(p.resolution, Some(Resolution::External(_)))).count();
    let unres_paths = k.paths.len() - local_paths - ext_paths;

    // The expansion ledger
    let mut by_outcome: BTreeMap<String, usize> = BTreeMap::new();
    for e in &k.expansions {
        let key = match &e.outcome {
            klepto::Outcome::Expanded { .. } => "Expanded".to_string(),
            klepto::Outcome::ParseError(_) => "ParseError".to_string(),
            other => format!("{other:?}"),
        };
        *by_outcome.entry(key).or_default() += 1;
    }
    let generated = k.items.iter().filter(|i| i.generated.is_some()).count();
    let top = |want: &str| -> Vec<(usize, String)> {
        let mut m: BTreeMap<String, usize> = BTreeMap::new();
        for e in &k.expansions {
            let key = match &e.outcome {
                klepto::Outcome::Expanded { .. } => "Expanded".to_string(),
                klepto::Outcome::ParseError(_) => "ParseError".to_string(),
                other => format!("{other:?}"),
            };
            if key == want {
                *m.entry(e.macro_path.clone()).or_default() += 1;
            }
        }
        let mut v: Vec<(usize, String)> = m.into_iter().map(|(p, n)| (n, p)).collect();
        v.sort_by(|a, b| b.cmp(a));
        v.truncate(5);
        v
    };
    println!("   not expanded: unmatched {:?} parse-error {:?} unknown {:?} depth {:?}", top("Unmatched"), top("ParseError"), top("ProcMacroOrUnknown"), top("DepthExceeded"));
    println!("   most expanded: {:?}", top("Expanded"));
    if let Ok(m) = std::env::var("KLEPTO_TRACE_MACRO") {
        for e in k.expansions.iter().filter(|e| e.macro_path == m || e.macro_path.ends_with(&format!("::{m}"))).take(4) {
            println!("   trace {m}: {:?} at {:?}:{:?}", e.outcome, e.location.path.file_name(), e.location.line);
        }
        println!("   known {m}: {:?}", k.macros_known.iter().filter(|(n, _)| n == &m).collect::<Vec<_>>());
        println!("   items named {m}: {:?}", k.items.iter().filter(|i| i.name == m).map(|i| (&i.id, &i.generated)).collect::<Vec<_>>());
    }
    let findings = klepto::RuleRunner::new(&k).with_all_rules().run();
    for f in findings.iter().filter(|f| f.code == "KLEP011").take(4) {
        println!("   KLEP011 sample: {}", format!("{f:?}").chars().take(330).collect::<String>());
    }
    println!(
        "== {name}: expansions {:?} generated items {generated} macros known {} dep note {:?}",
        by_outcome,
        k.macros_known.len(),
        k.dep_macro_note.as_deref().map(|s| s.chars().take(80).collect::<String>())
    );

    let t = Instant::now();
    let receipt = Receipt::from_klepto(&k);
    let json = receipt.to_json_compact();
    let t_receipt = t.elapsed();

    let t = Instant::now();
    let bundle = kg::export(&k);
    let t_kg = t.elapsed();

    let t = Instant::now();
    let patterns = k.patterns();
    let t_patterns = t.elapsed();

    let t = Instant::now();
    let policy = Policy::parse("[levels]\nKLEP001 = \"allow\"\nKLEP012 = \"allow\"\n").expect("policy");
    let verdict = run_pipeline(&k, &policy);
    let t_pipeline = t.elapsed();

    println!(
        "== {name}: files {} (skipped {}) fns {} items {} types {} impls {} edges {} unresolved {}",
        k.files.len(),
        k.skipped_files.len(),
        k.functions.len(),
        k.items.len(),
        k.types.len(),
        k.impls.len(),
        k.graph.edges.len(),
        k.graph.unresolved.len()
    );
    println!("   paths: local {local_paths} external {ext_paths} unresolved {unres_paths}");
    println!("   unresolved by reason: {by_reason:?}");
    println!(
        "   method calls {methods}, typed {typed} ({:.0}%)",
        if methods == 0 { 0.0 } else { 100.0 * typed as f64 / methods as f64 }
    );
    println!("   receipt {:.1} MB, digest ok {}", json.len() as f64 / 1e6, receipt.verify_digest());
    println!("   kg records {} challenges {}", bundle.knowledge.len(), bundle.challenges.len());
    println!(
        "   patterns: shaped {} tightest {} candidates {} shared sets {} repeated impls {}",
        patterns.shapes.len(),
        patterns.clusters.iter().filter(|c| c.tightest).count(),
        patterns.candidates.len(),
        patterns.shared_method_sets.len(),
        patterns.repeated_impls.len()
    );
    println!("   pipeline: {}", verdict.summary().replace('\n', " | "));
    println!(
        "   timings: parse {:.2}s receipt {:.2}s kg {:.2}s patterns {:.2}s pipeline {:.2}s",
        t_parse.as_secs_f64(),
        t_receipt.as_secs_f64(),
        t_kg.as_secs_f64(),
        t_patterns.as_secs_f64(),
        t_pipeline.as_secs_f64()
    );
    for (p, e) in k.skipped_files.iter().take(5) {
        println!("   skipped {}: {}", p.display(), e.lines().next().unwrap_or(""));
    }
}
