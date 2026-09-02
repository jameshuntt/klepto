use crate::{error::Result, passenger::{CheckpointOptions, PassengerStore}};



#[derive(clap::Subcommand, Debug)]
pub enum PassengerCmd {
    Init,

    Branch {
        #[clap(subcommand)]
        cmd: BranchCmd,
    },

    Checkout {
        name: String,
    },

    Checkpoint {
        #[clap(long)]
        note: Option<String>,

        #[clap(long)]
        branch: Option<String>,

        #[clap(long)]
        no_artifacts: bool,
    },

    Log {
        #[clap(long, default_value_t = 25)]
        n: usize,
    },
}

#[derive(clap::Subcommand, Debug)]
pub enum BranchCmd {
    Create {
        name: String,
        #[clap(long)]
        from: Option<String>,
    },
}

pub fn passenger(cmd:PassengerCmd, ctx:crate::engine::RunContext) -> Result<()> {
    match cmd {
        PassengerCmd::Init => {
            PassengerStore::init(".")?;
            println!("initialized .passenger/");
        }

        PassengerCmd::Branch { cmd } => match cmd {
            BranchCmd::Create { name, from } => {
                let s = PassengerStore::open(".")?;
                s.create_branch(&name, from.as_deref())?;
                println!("created branch {name}");
            }
        },

        PassengerCmd::Checkout { name } => {
            let s = PassengerStore::open(".")?;
            s.checkout_branch(&name)?;
            println!("checked out {name}");
        }

        PassengerCmd::Checkpoint {
            note,
            branch,
            no_artifacts,
        } => {
            let s = PassengerStore::open(".")?;

            // run scan
            let out = crate::engine::run_scan(&ctx)?; // (use your ctx builder)
            // run analysis (you can return a serde_json::Value, or serialize AnalysisState)
            let analysis = crate::analysis::run(&out);
            let analysis_json = serde_json::to_value(&analysis)?; // if AnalysisState is Serialize

            let commit = s.checkpoint(
                Some(&out),
                Some(&analysis_json),
                CheckpointOptions {
                    note,
                    branch,
                    include_artifacts: !no_artifacts,
                    track_roots: None,
                },
            )?;

            println!("checkpoint {} on {}", commit.id, commit.branch);
        }

        PassengerCmd::Log { n } => {
            let s = PassengerStore::open(".")?;
            let head = s.resolve_head()?;
            let mut cur = head.head_commit;

            println!("== log ({}) ==", head.branch);
            for _ in 0..n {
                let Some(id) = cur.take() else {
                    break;
                };
                let c = s.read_commit(&head.passenger_version, &id)?;
                println!(
                    "{}  {}  changed_files={} +{} -{}  {}",
                    c.id,
                    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(c.ts_ms)
                        .unwrap(),
                    c.stats.changed_files,
                    c.stats.added_lines,
                    c.stats.removed_lines,
                    c.note.clone().unwrap_or_default()
                );
                cur = c.parents.first().cloned();
            }
        }
    }

    Ok(())
}
