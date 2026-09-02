//! `gate` (rules G1–G7, X in `GATE_RULES.md`).

use crate::commands::{crate_name_guess, klepto_for, resolve_package};
use crate::error::{PassengerError, Result};
use crate::gate::{apply, relative, EditSet};
use crate::oracle::CargoOracle;
use klepto::{ConvergeStatus, Convergence, FeatureModel, FeatureSpec, GatePlan, GateReason, Klepto, ManifestEdit};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct GateOptions {
    pub spec: PathBuf,
    pub package: Option<String>,
    pub write: bool,
    pub check: bool,
    pub max_rounds: usize,
    pub no_cargo: bool,
}

/// Rule G2.
#[derive(Debug, Clone)]
pub struct Planned {
    pub package: String,
    pub current: FeatureModel,
    pub desired: FeatureModel,
    pub plan: GatePlan,
}

#[derive(Debug)]
pub struct GateOutcome {
    pub root: PathBuf,
    pub package: String,
    pub plan: GatePlan,
    pub edits: EditSet,
    pub convergence: Option<Convergence>,
    pub written: bool,
    /// Rule X.
    pub exit_code: i32,
}

/// Rule G2.
pub fn plan(root: &Path, package: Option<&str>, spec: &Path) -> Result<(Klepto, Planned)> {
    let spec = FeatureSpec::from_path(spec).map_err(|e| PassengerError::Unsupported(e.to_string()))?;
    let k = klepto_for(root, &crate_name_guess(root))?;
    let package = resolve_package(&k, root, package)?;
    let current = k
        .feature_model(&package)
        .cloned()
        .ok_or_else(|| PassengerError::Unsupported(format!("no feature model for package `{package}` (is it a workspace member?)")))?;
    let pspec = spec
        .package(&package)
        .cloned()
        .ok_or_else(|| PassengerError::Unsupported(format!("the spec has no [{package}] table")))?;
    let desired = current.apply_spec(&pspec);
    let plan = k.plan_gates(&package, &desired);
    Ok((k, Planned { package, current, desired, plan }))
}

/// Rules G3–G7.
pub fn run(root: &Path, opts: &GateOptions) -> Result<GateOutcome> {
    let (k, planned) = plan(root, opts.package.as_deref(), &opts.spec)?;
    let mut plan = planned.plan.clone();
    let mut convergence = None;
    if !opts.no_cargo && !plan.is_empty() {
        let mut oracle = CargoOracle::new(root);
        let conv = k.converge(root, &planned.package, &planned.desired, &mut oracle, opts.max_rounds);
        plan = conv.plan.clone();
        convergence = Some(conv);
    }
    let edits = apply(root, &plan)?;

    let gave_up = convergence.as_ref().is_some_and(|c| c.status == ConvergeStatus::GaveUp);
    let human = convergence.as_ref().is_some_and(|c| !c.human.is_empty());
    let unknown = !plan.unknown.is_empty();
    let exit_code = if opts.check {
        if plan.is_empty() { 0 } else { 1 }
    } else if unknown || human {
        2
    } else if gave_up {
        1
    } else {
        0
    };
    let can_write = opts.write && !opts.check && !plan.is_empty() && exit_code == 0;
    let written = if can_write {
        edits.write()?;
        true
    } else {
        false
    };
    Ok(GateOutcome { root: root.to_path_buf(), package: planned.package, plan, edits, convergence, written, exit_code })
}

fn reason_text(r: &GateReason) -> String {
    match r {
        GateReason::UngatedDep { dep } => format!("ungated dep {dep}"),
        GateReason::ExistingViolation { to } => format!("references gated {to}"),
        GateReason::Cascade { from } => format!("cascade from {from}"),
        GateReason::Forced { code, mention } => format!("forced by {code} on {mention}"),
    }
}

fn manifest_text(e: &ManifestEdit) -> String {
    match e {
        ManifestEdit::MakeOptional { dep } => format!("optional {dep}"),
        ManifestEdit::SetFeature { name, enables } => format!("feature {name} = {enables:?}"),
        ManifestEdit::SetDefault { features } => format!("default = {features:?}"),
    }
}

/// Rule G3: the human rendering.
pub fn render(out: &GateOutcome) -> String {
    let rel = |p: &Path| relative(&out.root, p).map(|r| r.to_string_lossy().into_owned()).unwrap_or_else(|_| p.display().to_string());
    let mut s = String::new();
    s.push_str("CODE PASSENGER :: GATE\n──────────────────────\n");
    s.push_str(&format!("root: {}\npackage: {}\n", out.root.display(), out.package));
    if out.plan.is_empty() {
        s.push_str("plan: nothing to do\n");
    } else {
        s.push_str("manifest:\n");
        for e in &out.plan.manifest {
            s.push_str(&format!("  {}\n", manifest_text(e)));
        }
        s.push_str("gates:\n");
        for g in &out.plan.gates {
            let line = g.location.line.map(|l| l.to_string()).unwrap_or_else(|| "?".into());
            let reasons: Vec<String> = g.reasons.iter().map(reason_text).collect();
            s.push_str(&format!("  {}:{}  {}  {}  ({})\n", rel(&g.location.path), line, g.attribute_text(), g.item, reasons.join("; ")));
        }
        if !out.plan.reaches_public.is_empty() {
            s.push_str("reaches_public:\n");
            for p in &out.plan.reaches_public {
                s.push_str(&format!("  {p}\n"));
            }
        }
        if !out.plan.unknown.is_empty() {
            s.push_str("unknown (a person decides):\n");
            for u in &out.plan.unknown {
                let line = u.location.line.map(|l| l.to_string()).unwrap_or_else(|| "?".into());
                s.push_str(&format!("  {}:{}  {}  {}\n", rel(&u.location.path), line, u.item, u.why));
            }
        }
    }
    match &out.convergence {
        None => s.push_str("oracle: skipped\n"),
        Some(c) => {
            let status = match c.status {
                ConvergeStatus::Converged => "converged",
                ConvergeStatus::GaveUp => "gave up",
            };
            s.push_str(&format!("oracle: {status} after {} round(s), {} forced gate(s)\n", c.rounds.len(), c.forced.len()));
            if !c.human.is_empty() {
                s.push_str("human:\n");
                for h in &c.human {
                    s.push_str(&format!(
                        "  {} {} {}\n",
                        h.diagnostic.code.as_deref().unwrap_or("-"),
                        h.item.as_deref().unwrap_or("?"),
                        h.diagnostic.message
                    ));
                }
            }
        }
    }
    if !out.edits.is_empty() {
        s.push_str("diff:\n");
        s.push_str(&out.edits.diff_from(&out.root));
    }
    s.push_str(&format!("written: {}\nexit: {}\n", if out.written { "yes" } else { "no" }, out.exit_code));
    s
}
