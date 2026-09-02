use crate::analysis::run as r;
use crate::engine::{RunContext, run_scan};
use crate::error::{PassengerError, Result};
use crate::passenger::command::{PassengerCmd, passenger};
use crate::{commands, packs, plans};
use clap::{Parser, Subcommand};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "code-passenger")]
#[command(about = "Scan a Rust crate for feature/dependency usage and generate feature gating headers.", long_about = None)]
pub struct Cli {
    /// Path to crate root (where Cargo.toml lives)
    #[arg(long, default_value = ".")]
    pub root: PathBuf,

    /// Relative source directory to scan
    #[arg(long, default_value = "src")]
    pub src: String,

    /// Manifest path (defaults to {root}/Cargo.toml)
    #[arg(long)]
    pub manifest: Option<PathBuf>,

    #[arg(long, default_value = "rust")]
    pub lang: String,

    #[arg(long, default_value = "verbose")]
    pub plan: String,

    /// Emit JSON instead of human output
    #[arg(long)]
    pub json: bool,

    #[command(subcommand)]
    pub cmd: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Scan and print reports
    Scan,
    /// Annotate files (insert/replace header blocks)
    Annotate {
        /// Actually write changes (otherwise dry-run)
        #[arg(long)]
        write: bool,

        /// Exit non-zero if changes would be made (CI mode)
        #[arg(long)]
        check: bool,
    },
    Scaffold {
        #[arg(long)]
        kind: String, // "module" etc

        #[arg(long)]
        name: String,

        /// like "net/http"
        #[arg(long, default_value = "")]
        path: String,

        #[arg(long, default_value = "default")]
        scaffold_plan: String,

        #[arg(long)]
        write: bool,
    },
    Passenger {
        #[clap(subcommand)]
        cmd: PassengerCmd,
    },

    // ---- klepto-backed commands (GATE_RULES.md) ----
    /// Plan, prove (real `cargo check` on a scratch copy) and apply feature
    /// gates from a klepto.toml spec
    Gate {
        /// The klepto.toml spec (package → optional deps / features / default)
        #[arg(long)]
        spec: PathBuf,
        /// Package to gate (defaults to the root manifest's [package].name)
        #[arg(long)]
        package: Option<String>,
        /// Write the edits to the tree once the oracle converged
        #[arg(long)]
        write: bool,
        /// Exit 1 when there is gating to do (CI mode); never writes
        #[arg(long)]
        check: bool,
        /// Converge rounds with the cargo oracle
        #[arg(long, default_value_t = 3)]
        max_rounds: usize,
        /// Skip the cargo oracle (plan and diff only)
        #[arg(long)]
        no_cargo: bool,
    },
    /// Features, dependencies and use sites per package
    FeatureMap {
        #[arg(long)]
        package: Option<String>,
    },
    /// Use sites of a dependency, or of the dependencies a feature enables
    FeatureSites {
        /// Dependency or feature name
        target: String,
        #[arg(long)]
        package: Option<String>,
    },
    /// Review what gating a dependency or feature changes (never writes)
    FeatureProbe {
        /// Dependency or feature name
        target: String,
        #[arg(long)]
        package: Option<String>,
        /// Review against this klepto.toml instead of the current manifest
        #[arg(long)]
        spec: Option<PathBuf>,
    },
    /// Write the klepto receipt (and, with --kg, the knowledge-graph export)
    Receipt {
        /// Output file (default: <root>/receipt.json)
        #[arg(long)]
        out: Option<PathBuf>,
        /// Directory for knowledge.ndjson and challenges.ndjson
        #[arg(long)]
        kg: Option<PathBuf>,
    },
    /// Run klepto's analyses under a policy; the exit code is the verdict's
    Lint {
        /// Policy TOML ([levels] / [max]); default denies KLEP010 and KLEP011
        #[arg(long)]
        policy: Option<PathBuf>,
    },
    /// Repeated-code candidates (extract fn, parameterize, generic, trait, macro)
    Patterns {
        #[arg(long, default_value_t = 16)]
        min_tokens: usize,
        #[arg(long, default_value_t = 20)]
        top: usize,
    },
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let manifest_path = cli
        .manifest
        .clone()
        .unwrap_or_else(|| cli.root.join("Cargo.toml"));

    let ctx = RunContext {
        root: cli.root.clone(),
        src_rel: cli.src.clone(),
        manifest_path,
        lang: cli.lang.clone(),
    };

    match cli.cmd {
        Command::Scan => {
            let out = run_scan(&ctx)?;
            if cli.json {
                // println!("{}", serde_json::to_string_pretty(&out.reports).unwrap());

                let a = r(&out);
                let file = std::fs::File::create("report.json")?;
                let writer = std::io::BufWriter::new(file);

                let _ = serde_json::to_writer_pretty(writer, &a);
                println!("Saved report to report.json");
            } else {
                let top_n = 5;

                for r in &out.reports {
                    println!("== {} ==", r.document.relative_path);

                    print_set("used_deps", &r.used.packages); // or r.used.packages
                    print_set("used_mods", &r.used.modules); // or r.used.modules
                    print_set("corpus_features", &r.corpus_features);

                    // Optional: quick totals
                    println!(
                        "use_sites: external={}, internal={}",
                        r.external_use_sites.len(),
                        r.internal_use_sites.len()
                    );

                    print_counts("external_symbols", &r.external_dep_symbol_counts, top_n);
                    print_counts("internal_symbols", &r.internal_dep_symbol_counts, top_n);

                    println!();
                }
            }
        }

        Command::Annotate { write, check } => {
            let out = run_scan(&ctx)?;
            let pack = packs::get_pack(&cli.lang)?;
            let plan = plans::get_plan(&cli.plan)?;

            let mut changed_any = false;

            for r in &out.reports {
                let header = pack.render_header(plan.as_ref(), r);

                let file_path = cli.root.join(&cli.src).join(&r.document.relative_path);
                let original = fs::read_to_string(&file_path)?;
                let updated = pack.apply_header(&original, &header);

                if updated != original {
                    changed_any = true;
                    if write {
                        fs::write(&file_path, updated)?;
                    }
                }
            }

            if check && changed_any {
                return Err(PassengerError::ChangesNeeded);
            }
        }

        Command::Scaffold {
            kind,
            name,
            path,
            scaffold_plan,
            write,
        } => {
            let pack = packs::get_pack(&cli.lang)?;
            let plan = crate::scaffolds::get_scaffold_plan(&scaffold_plan)?;

            let kind = crate::model::ScaffoldKind::parse(&kind).ok_or_else(|| {
                PassengerError::Unsupported(format!("unknown scaffold kind '{kind}'"))
            })?;

            let module_path: Vec<String> = path
                .split('/')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();

            let out = pack.scaffold(
                plan.as_ref(),
                crate::model::ScaffoldRequest {
                    kind,
                    name,
                    module_path,
                },
            )?;

            if out.files.is_empty() {
                return Err(PassengerError::Unsupported(
                    "scaffold produced no files".into(),
                ));
            }

            for (rel, content) in out.files {
                let abs = cli.root.join(rel);
                if write {
                    if let Some(parent) = abs.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::write(&abs, content)?;
                } else {
                    println!("--- {}", abs.display());
                    println!("{content}");
                }
            }
        }
        Command::Passenger { cmd } => { passenger(cmd,ctx)? }

        // ---- klepto-backed commands (GATE_RULES.md) ----
        Command::Gate { spec, package, write, check, max_rounds, no_cargo } => {
            let opts = commands::gate::GateOptions { spec, package, write, check, max_rounds, no_cargo };
            let out = commands::gate::run(&cli.root, &opts)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&out.plan)?);
            } else {
                print!("{}", commands::gate::render(&out));
            }
            if out.exit_code != 0 {
                std::process::exit(out.exit_code);
            }
        }
        Command::FeatureMap { package } => {
            let m = commands::features::feature_map(&cli.root, package.as_deref())?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&m)?);
            } else {
                print!("{}", commands::features::render_map(&m));
            }
        }
        Command::FeatureSites { target, package } => {
            let s = commands::features::feature_sites(&cli.root, package.as_deref(), &target)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&s)?);
            } else {
                print!("{}", commands::features::render_sites(&s));
            }
        }
        Command::FeatureProbe { target, package, spec } => {
            let p = commands::features::feature_probe(&cli.root, package.as_deref(), &target, spec.as_deref())?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&p)?);
            } else {
                print!("{}", commands::features::render_probe(&p));
            }
        }
        Command::Receipt { out, kg } => {
            let out = out.unwrap_or_else(|| cli.root.join("receipt.json"));
            let w = commands::receipt::write(&cli.root, &out, kg.as_deref())?;
            println!("wrote {}", w.receipt.display());
            if let Some(k) = &w.knowledge {
                println!("wrote {}", k.display());
            }
            if let Some(c) = &w.challenges {
                println!("wrote {}", c.display());
            }
        }
        Command::Lint { policy } => {
            let v = commands::lint::run(&cli.root, policy.as_deref())?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                println!("{}", v.summary());
            }
            if v.exit_code != 0 {
                std::process::exit(v.exit_code);
            }
        }
        Command::Patterns { min_tokens, top } => {
            let r = commands::patterns::run(&cli.root, &klepto::MiningOptions { min_tokens, min_members: 2 })?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&r)?);
            } else {
                print!("{}", commands::patterns::render(&r, top));
            }
        }
    }

    Ok(())
}

use std::collections::BTreeMap;

fn preview_syms(syms: &BTreeMap<String, usize>, top_n: usize) -> String {
    let mut v: Vec<(&String, &usize)> = syms.iter().collect();
    v.sort_by(|(ka, ca), (kb, cb)| {
        // count desc, then name asc
        cb.cmp(ca).then_with(|| ka.cmp(kb))
    });

    v.into_iter()
        .take(top_n)
        .map(|(k, c)| format!("{k}({c})"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_counts(title: &str, counts: &BTreeMap<String, BTreeMap<String, usize>>, top_n: usize) {
    if counts.is_empty() {
        return;
    }

    println!("{title}:");
    for (dep, syms) in counts {
        if syms.is_empty() {
            continue;
        }
        let preview = preview_syms(syms, top_n);
        if preview.is_empty() {
            continue;
        }
        println!("  {dep}: {preview}");
    }
}

fn print_set<T: std::fmt::Debug>(label: &str, set: &T) {
    println!("{label}: {set:?}");
}
