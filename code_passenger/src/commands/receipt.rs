//! `receipt` (rule R1 in `GATE_RULES.md`).

use crate::commands::{crate_name_guess, klepto_for};
use crate::error::Result;
use klepto::{kg, Receipt};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Written {
    pub receipt: PathBuf,
    pub knowledge: Option<PathBuf>,
    pub challenges: Option<PathBuf>,
}

/// Rule R1.
pub fn write(root: &Path, out: &Path, kg_dir: Option<&Path>) -> Result<Written> {
    let k = klepto_for(root, &crate_name_guess(root))?;
    let receipt = Receipt::from_klepto(&k);
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(out, receipt.to_json_string())?;
    let mut written = Written { receipt: out.to_path_buf(), knowledge: None, challenges: None };
    if let Some(dir) = kg_dir {
        std::fs::create_dir_all(dir)?;
        let bundle = kg::export(&k);
        let knowledge = dir.join("knowledge.ndjson");
        let challenges = dir.join("challenges.ndjson");
        std::fs::write(&knowledge, kg::to_ndjson(&bundle.knowledge))?;
        std::fs::write(&challenges, kg::to_ndjson(&bundle.challenges))?;
        written.knowledge = Some(knowledge);
        written.challenges = Some(challenges);
    }
    Ok(written)
}
