//! Sanity check on a real workspace: how many type / impl records
//! were collected and how many method calls got a receiver type.
//!
//!     cargo run --example type_stats -- <workspace root> <crate name>

use klepto::{Klepto, Resolution};

fn main() {
    let mut args = std::env::args().skip(1);
    let root = args.next().unwrap_or_else(|| ".".into());
    let name = args.next().unwrap_or_else(|| "crate".into());
    let k = Klepto::new(&name)
        .scan_workspace_root(&root)
        .expect("cargo metadata")
        .parse()
        .expect("parse");

    println!("files {} fns {} types {} impls {}", k.files.len(), k.functions.len(), k.types.len(), k.impls.len());

    let methods: Vec<_> = k.calls.iter().filter(|c| c.is_method).collect();
    let typed = methods.iter().filter(|c| c.receiver_type.is_some()).count();
    let local = methods.iter().filter(|c| matches!(c.resolution, Some(Resolution::Local(_)))).count();
    let external = methods.iter().filter(|c| matches!(c.resolution, Some(Resolution::External(_)))).count();
    println!("method calls {} typed {} resolved local {} external {}", methods.len(), typed, local, external);

    let by_kind = |kind: klepto::TypeKind| k.types.iter().filter(|t| t.kind == kind).count();
    println!(
        "structs {} enums {} unions {} traits {} aliases {}",
        by_kind(klepto::TypeKind::Struct),
        by_kind(klepto::TypeKind::Enum),
        by_kind(klepto::TypeKind::Union),
        by_kind(klepto::TypeKind::Trait),
        by_kind(klepto::TypeKind::TypeAlias)
    );

    let mut sample: Vec<_> = methods.iter().filter(|c| matches!(c.resolution, Some(Resolution::Local(_)))).take(8).collect();
    sample.sort_by_key(|c| (c.location.path.clone(), c.location.line));
    for c in sample {
        if let Some(Resolution::Local(to)) = &c.resolution {
            println!("  {} -> {}", c.enclosing_fn.as_deref().unwrap_or("<module>"), to);
        }
    }
}
