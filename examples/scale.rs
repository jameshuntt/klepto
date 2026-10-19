//! Memory and time on part of a workspace: the members named after the root
//! are scanned (all of them when none are named).
//!
//!     cargo run --release --example scale -- <workspace root> [member...]

use klepto::Klepto;
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let root = args.next().unwrap_or_else(|| ".".into());
    let members: Vec<String> = args.collect();
    let t = Instant::now();
    let mut b = Klepto::new("scale").scan_workspace_root(&root).expect("cargo metadata").ignore_parse_errors(true).exclude_generated().expect("globs");
    if !members.is_empty() {
        let names: Vec<&str> = members.iter().map(String::as_str).collect();
        b = b.only_members(&names);
    }
    if std::env::var_os("SKIP_DEP_MACROS").is_some() {
        b = b.skip_dependency_macros(true);
    }
    let k = b.parse().expect("parse");
    let lines: usize = k.files.iter().map(|f| f.source.lines().count()).sum();
    println!(
        "{} files, {lines} lines, {} paths, {} calls, {} literals, {} items, {} edges, {} expansions in {:?}",
        k.files.len(),
        k.paths.len(),
        k.calls.len(),
        k.literals.len(),
        k.items.len(),
        k.graph.edges.len(),
        k.expansions.len(),
        t.elapsed()
    );
}
