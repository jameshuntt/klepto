//! Phase 4: the `cargo check` matrix as data, cargo's JSON diagnostics
//! mapped back to items, and the converge loop over an [`Oracle`] — rules
//! V1–V3, X1–X3, L1–L4 in `PHASE4_RULES.md`. klepto never runs cargo and
//! never edits a file: the oracle implementation (`code_passenger`) does.

use crate::cfg::{CfgPredicate, Implication};
use crate::features::FeatureModel;
use crate::klepto::Klepto;
use crate::model::FileLocation;
use crate::plan::{ForcedGate, GatePlan, GateReason};
use crate::resolve::norm;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// One configuration to compile.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckJob {
    pub package: String,
    /// Features passed with `--features`.
    pub features: Vec<String>,
    pub no_default: bool,
}

impl CheckJob {
    /// Rule V3: arguments after `cargo`.
    pub fn cargo_args(&self) -> Vec<String> {
        let mut a: Vec<String> = vec!["check".into(), "-p".into(), self.package.clone(), "--message-format=json".into()];
        if self.no_default {
            a.push("--no-default-features".into());
        }
        if !self.features.is_empty() {
            a.push("--features".into());
            a.push(self.features.join(","));
        }
        a
    }

    pub fn label(&self) -> String {
        let defaults = if self.no_default { "no-default" } else { "default" };
        if self.features.is_empty() {
            format!("{}[{defaults}]", self.package)
        } else {
            format!("{}[{defaults}+{}]", self.package, self.features.join(","))
        }
    }
}

/// Cap under which every feature subset is checked (rule V2).
const EXHAUSTIVE_FEATURES: usize = 4;

/// Rules V1–V2.
pub fn check_matrix(desired: &FeatureModel) -> Vec<CheckJob> {
    let declared: Vec<&String> = desired.features.keys().collect();
    let mut jobs = Vec::new();
    let mut seen: BTreeSet<BTreeSet<String>> = BTreeSet::new();
    let mut push = |features: Vec<String>, no_default: bool| {
        let requested: Vec<&str> = features.iter().map(String::as_str).collect();
        let closure = desired.active_features(&requested, !no_default);
        if seen.insert(closure) {
            jobs.push(CheckJob { package: desired.package.clone(), features, no_default });
        }
    };

    if declared.len() <= EXHAUSTIVE_FEATURES {
        let n = declared.len();
        let mut masks: Vec<u32> = (0..(1u32 << n)).collect();
        masks.sort_by_key(|m| (m.count_ones(), *m));
        for mask in masks {
            let subset: Vec<String> = declared.iter().enumerate().filter(|(i, _)| mask & (1 << i) != 0).map(|(_, f)| (*f).clone()).collect();
            push(subset.clone(), true);
            push(subset, false);
        }
    } else {
        push(Vec::new(), true);
        push(Vec::new(), false);
        for f in &declared {
            if f.as_str() != "default" {
                push(vec![(*f).clone()], true);
            }
        }
        push(declared.iter().filter(|f| f.as_str() != "default").map(|f| (*f).clone()).collect(), false);
    }
    jobs
}

/// One compiler error, from `cargo --message-format=json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Diagnostic {
    /// `E0433`, …
    pub code: Option<String>,
    pub level: String,
    pub message: String,
    /// The first backticked name in the message (`alpha`, `gated_fn`).
    pub mention: Option<String>,
    /// As cargo prints it: relative to the workspace root.
    pub file: Option<PathBuf>,
    /// 1-based.
    pub line: Option<u32>,
    /// 1-based, as cargo prints it.
    pub column: Option<u32>,
}

/// Rule X2.
fn mention_of(message: &str) -> Option<String> {
    let start = message.find('`')?;
    let rest = &message[start + 1..];
    let end = rest.find('`')?;
    let m = rest[..end].trim();
    if m.is_empty() { None } else { Some(m.to_string()) }
}

/// Rules X1–X2.
pub fn parse_cargo_json(text: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if v.get("reason").and_then(|r| r.as_str()) != Some("compiler-message") {
            continue;
        }
        let Some(msg) = v.get("message") else { continue };
        let level = msg.get("level").and_then(|l| l.as_str()).unwrap_or("").to_string();
        if level != "error" {
            continue;
        }
        let message = msg.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string();
        let code = msg.get("code").and_then(|c| c.get("code")).and_then(|c| c.as_str()).map(str::to_string);
        let primary = msg
            .get("spans")
            .and_then(|s| s.as_array())
            .and_then(|spans| spans.iter().find(|s| s.get("is_primary").and_then(|p| p.as_bool()) == Some(true)));
        let (file, line, column) = match primary {
            Some(s) => (
                s.get("file_name").and_then(|f| f.as_str()).map(PathBuf::from),
                s.get("line_start").and_then(|l| l.as_u64()).map(|l| l as u32),
                s.get("column_start").and_then(|c| c.as_u64()).map(|c| c as u32),
            ),
            None => (None, None, None),
        };
        out.push(Diagnostic { code, level, mention: mention_of(&message), message, file, line, column });
    }
    out
}

/// A diagnostic mapped back to the plan's vocabulary (rule X3).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MissedGate {
    /// The item whose declaration encloses the diagnostic.
    pub item: Option<String>,
    /// The gate that would fix it, when the mention is a dependency or a
    /// local item with a cfg.
    pub predicate: Option<CfgPredicate>,
    pub diagnostic: Diagnostic,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct OracleResult {
    pub ok: bool,
    pub diagnostics: Vec<Diagnostic>,
}

impl OracleResult {
    pub fn passed() -> Self {
        OracleResult { ok: true, diagnostics: Vec::new() }
    }

    pub fn failed(diagnostics: Vec<Diagnostic>) -> Self {
        OracleResult { ok: false, diagnostics }
    }
}

/// Rule L1: something that can compile one configuration with a plan
/// applied. `code_passenger` implements it with a scratch copy and cargo;
/// tests use [`FnOracle`].
pub trait Oracle {
    fn check(&mut self, root: &Path, job: &CheckJob, plan: &GatePlan) -> OracleResult;
}

/// An oracle backed by a closure (tests, dry runs, recorded results).
pub struct FnOracle<F>(pub F);

impl<F: FnMut(&CheckJob, &GatePlan) -> OracleResult> Oracle for FnOracle<F> {
    fn check(&mut self, _root: &Path, job: &CheckJob, plan: &GatePlan) -> OracleResult {
        (self.0)(job, plan)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConvergeStatus {
    /// Every job passed with the final plan.
    Converged,
    /// A round produced no new actionable gate, or `max_rounds` was reached.
    GaveUp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Round {
    pub plan: GatePlan,
    pub jobs: Vec<CheckJob>,
    /// Parallel to `jobs`.
    pub results: Vec<OracleResult>,
    pub missed: Vec<MissedGate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Convergence {
    pub status: ConvergeStatus,
    pub rounds: Vec<Round>,
    /// The last plan tried.
    pub plan: GatePlan,
    /// Gates learned from the compiler across all rounds.
    pub forced: Vec<ForcedGate>,
    /// Diagnostics no rule could turn into a gate: a person decides.
    pub human: Vec<MissedGate>,
}

fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

impl Klepto {
    /// Rule X3.
    pub fn map_diagnostic(&self, root: &Path, package: &str, desired: &FeatureModel, d: &Diagnostic) -> MissedGate {
        let item = d.file.as_ref().and_then(|f| {
            let target = canon(&root.join(f));
            let line = d.line?;
            // nodes of the package living in that file
            let mut in_file: Vec<_> = self
                .graph
                .nodes
                .values()
                .filter(|n| n.package.as_deref() == Some(package))
                .filter(|n| n.location.as_ref().is_some_and(|l| l.line.is_some() && canon(&l.path) == target))
                .collect();
            if in_file.is_empty() {
                return None;
            }
            // the parsed path as the index knows it
            let parsed_path = in_file[0].location.as_ref().map(|l| l.path.clone())?;
            let loc = FileLocation { path: parsed_path, line: Some(line), column: Some(d.column.unwrap_or(1).saturating_sub(1)) };
            if let Some(span) = self.index.enclosing(&loc) {
                if self.graph.nodes.contains_key(&span.fq_name) {
                    return Some(span.fq_name.clone());
                }
            }
            in_file.sort_by_key(|n| n.location.as_ref().and_then(|l| l.line));
            in_file.iter().rev().find(|n| n.location.as_ref().and_then(|l| l.line).is_some_and(|l| l <= line)).map(|n| n.id.clone())
        });

        let predicate = d.mention.as_deref().and_then(|m| {
            let last = norm(m.rsplit("::").next().unwrap_or(m));
            if desired.dep(&last).is_some() {
                let p = desired.dep_predicate(&last).simplify();
                return if p == CfgPredicate::True { None } else { Some(p) };
            }
            let by_id = m.strip_prefix("crate::").map(|rest| format!("{package}::{rest}")).unwrap_or_else(|| m.to_string());
            let node = self.graph.node(&by_id).or_else(|| {
                let named: Vec<_> = self.graph.nodes.values().filter(|n| n.package.as_deref() == Some(package) && n.name == last).collect();
                if named.len() == 1 { Some(named[0]) } else { None }
            })?;
            if node.cfg == CfgPredicate::True || node.cfg.is_unknown() { None } else { Some(node.cfg.clone().simplify()) }
        });

        MissedGate { item, predicate, diagnostic: d.clone() }
    }

    /// Rules L2–L4.
    pub fn converge(&self, root: &Path, package: &str, desired: &FeatureModel, oracle: &mut dyn Oracle, max_rounds: usize) -> Convergence {
        let jobs = check_matrix(desired);
        let mut forced: Vec<ForcedGate> = Vec::new();
        let mut human: Vec<MissedGate> = Vec::new();
        let mut rounds: Vec<Round> = Vec::new();

        for _ in 0..max_rounds.max(1) {
            let plan = self.plan_gates_with(package, desired, &forced);
            let mut results = Vec::with_capacity(jobs.len());
            let mut missed = Vec::new();
            for job in &jobs {
                let res = oracle.check(root, job, &plan);
                for d in &res.diagnostics {
                    missed.push(self.map_diagnostic(root, package, desired, d));
                }
                results.push(res);
            }
            let all_ok = results.iter().all(|r| r.ok);
            rounds.push(Round { plan: plan.clone(), jobs: jobs.clone(), results, missed: missed.clone() });
            if all_ok {
                return Convergence { status: ConvergeStatus::Converged, rounds, plan, forced, human };
            }

            let mut added = false;
            for m in missed {
                let (Some(item), Some(p)) = (&m.item, &m.predicate) else {
                    human.push(m);
                    continue;
                };
                // L4: not new if already forced, or the plan already covers it
                let already_forced = forced.iter().any(|f| &f.item == item && &f.predicate == p);
                let already_planned = plan
                    .gate_for(item)
                    .is_some_and(|g| desired.closed_implies(&g.existing_cfg.clone().and(g.predicate.clone()).simplify(), p) == Implication::Always);
                if already_forced || already_planned {
                    continue;
                }
                forced.push(ForcedGate {
                    item: item.clone(),
                    predicate: p.clone(),
                    reason: GateReason::Forced {
                        code: m.diagnostic.code.clone().unwrap_or_default(),
                        mention: m.diagnostic.mention.clone().unwrap_or_default(),
                    },
                });
                added = true;
            }
            if !added {
                return Convergence { status: ConvergeStatus::GaveUp, rounds, plan, forced, human };
            }
        }

        let plan = rounds.last().map(|r| r.plan.clone()).unwrap_or_else(|| GatePlan { package: package.to_string(), ..Default::default() });
        Convergence { status: ConvergeStatus::GaveUp, rounds, plan, forced, human }
    }
}
