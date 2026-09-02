use klepto::{Klepto, findings_to_table};

/// `klepto [WORKSPACE_ROOT] [CRATE_NAME]` — run the default rules over a
/// workspace and print the findings table plus a few counts.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let root = args.next().unwrap_or_else(|| ".".into());
    let crate_name = args.next().unwrap_or_else(|| "crate".into());

    let k = Klepto::new(crate_name)
        .scan_workspace_root(&root)?
        .exclude_generated()?
        .parse()?;

    let findings = k.rules().with_default_rules().run();
    println!("{}", findings_to_table(&findings));

    println!(
        "files: {}  functions: {}  public fns: {}  imports: {}  findings: {}",
        k.files.len(),
        k.functions.len(),
        k.public_api().collect().len(),
        k.imports.len(),
        findings.len()
    );

    Ok(())
}
