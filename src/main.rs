use klepto::{Klepto, findings_to_table};

#[allow(unused)]
fn main_v1() -> Result<(), Box<dyn std::error::Error>> {
    let k = Klepto::new("crate")
        .scan_workspace_root(".")?
        .exclude_generated()?
        .only_newest(200)
        .parse()?;

    // Ergonomic queries
    let api = k.public_api()
        .returns("Result")
        .collect();

    let std_uses = k.imports()
        .root("std")
        .collect();

    // Finders
    let printlns = k.find_macro_invocations("println");
    let arc_paths = k.find_paths("std::sync::Arc");

    // Snapshot + diff
    let snap = k.snapshot();
    std::fs::write("klepto_snapshot.json", snap.to_json_string())?;

    // Rules + report
    let findings = k.rules().with_default_rules().run();
    println!("{}", findings_to_table(&findings));

    println!("public api fns returning Result: {}", api.len());
    println!("std imports: {}", std_uses.len());
    println!("println! invocations: {}", printlns.len());
    println!("Arc paths: {}", arc_paths.len());

    let k2 = Klepto::new("crate")
        .scan_workspace_root(".")?
        .exclude_generated()?
        .parse()?;

    let findings2 = k2.rules().with_default_rules().run();
    println!("{}", klepto::findings_to_table(&findings2));

    Ok(())
}


fn main() -> Result<(), Box<dyn std::error::Error>> {
    let k = Klepto::new("crate")
        .scan_workspace_root(".")?
        .exclude_generated()?
        .only_newest(200)
        .parse()?;

    // Ergonomic queries
    let api = k.public_api()
        .returns("Result")
        .collect();

    let std_uses = k.imports()
        .root("std")
        .collect();

    // Finders
    let printlns = k.find_macro_invocations("println");
    let arc_paths = k.find_paths("std::sync::Arc");

    // Snapshot + diff
    let snap = k.snapshot();
    std::fs::write("klepto_snapshot.json", snap.to_json_string())?;

    // Rules + report
    let findings = k.rules().with_default_rules().run();
    println!("{}", findings_to_table(&findings));

    println!("public api fns returning Result: {}", api.len());
    println!("std imports: {}", std_uses.len());
    println!("println! invocations: {}", printlns.len());
    println!("Arc paths: {}", arc_paths.len());

    Ok(())
}
