//! The cargo oracle (rules O1–O4 in `GATE_RULES.md`): proves a plan with
//! real `cargo check` on a scratch copy. The real tree is never written.

use crate::gate::{apply, relative, scratch_copy, workspace_root};
use klepto::{parse_cargo_json, CheckJob, Diagnostic, GatePlan, Oracle, OracleResult};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct CargoOracle {
    root: PathBuf,
    /// The workspace `root` belongs to (rule O1); the scratch copy is of this.
    workspace: Option<PathBuf>,
    scratch: Option<tempfile::TempDir>,
    /// Files written into the scratch copy by the previous check, relative
    /// to the workspace root (rule O1).
    touched: BTreeSet<PathBuf>,
    pub offline: bool,
}

fn internal(message: String) -> OracleResult {
    OracleResult {
        ok: false,
        diagnostics: vec![Diagnostic { code: None, level: "error".into(), message, mention: None, file: None, line: None, column: None }],
    }
}

impl CargoOracle {
    pub fn new(root: &Path) -> Self {
        Self { root: root.to_path_buf(), workspace: None, scratch: None, touched: BTreeSet::new(), offline: true }
    }

    /// The workspace the scratch copy mirrors, once a check has run.
    pub fn workspace(&self) -> Option<&Path> {
        self.workspace.as_deref()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn scratch_dir(&self) -> Option<&Path> {
        self.scratch.as_ref().map(|d| d.path())
    }

    pub fn touched(&self) -> &BTreeSet<PathBuf> {
        &self.touched
    }

    /// Rule O1: pristine copy of the whole workspace on first use;
    /// previously touched files restored. Returns the scratch directory
    /// that corresponds to `root` (cargo's working directory).
    fn prepare(&mut self, plan: &GatePlan) -> Result<PathBuf, String> {
        if self.workspace.is_none() {
            self.workspace = Some(workspace_root(&self.root).map_err(|e| e.to_string())?);
        }
        let ws = self.workspace.clone().ok_or("no workspace root")?;
        if self.scratch.is_none() {
            self.scratch = Some(scratch_copy(&ws).map_err(|e| e.to_string())?);
        }
        let scratch = self.scratch.as_ref().map(|d| d.path().to_path_buf()).ok_or("no scratch dir")?;
        for rel in std::mem::take(&mut self.touched) {
            let src = ws.join(&rel);
            let dst = scratch.join(&rel);
            if src.exists() {
                // read + write, not copy: a copied file keeps its old mtime and cargo would call the crate fresh
                let bytes = std::fs::read(&src).map_err(|e| format!("restore {}: {e}", rel.display()))?;
                std::fs::write(&dst, bytes).map_err(|e| format!("restore {}: {e}", rel.display()))?;
            } else {
                let _ = std::fs::remove_file(&dst);
            }
        }
        let edits = apply(&self.root, plan).map_err(|e| e.to_string())?;
        let written = edits.write_to(&ws, &scratch).map_err(|e| e.to_string())?;
        self.touched.extend(written);
        let inside = relative(&ws, &self.root).map_err(|e| e.to_string())?;
        Ok(scratch.join(inside))
    }
}

impl Oracle for CargoOracle {
    /// Rules O2–O3.
    fn check(&mut self, _root: &Path, job: &CheckJob, plan: &GatePlan) -> OracleResult {
        let cwd = match self.prepare(plan) {
            Ok(s) => s,
            Err(e) => return internal(format!("oracle setup failed: {e}")),
        };
        let target_dir = self.scratch.as_ref().map(|d| d.path().join("target")).unwrap_or_else(|| cwd.join("target"));
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let mut cmd = Command::new(cargo);
        cmd.args(job.cargo_args());
        if self.offline {
            cmd.arg("--offline");
        }
        cmd.current_dir(&cwd);
        cmd.env("CARGO_TARGET_DIR", target_dir);
        let output = match cmd.output() {
            Ok(o) => o,
            Err(e) => return internal(format!("could not run cargo: {e}")),
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut diagnostics = parse_cargo_json(&stdout);
        if !output.status.success() && diagnostics.is_empty() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            diagnostics.push(Diagnostic {
                code: None,
                level: "error".into(),
                message: stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("cargo failed").to_string(),
                mention: None,
                file: None,
                line: None,
                column: None,
            });
        }
        OracleResult { ok: output.status.success(), diagnostics }
    }
}
