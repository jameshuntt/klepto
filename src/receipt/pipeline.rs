//! The analysis frame. Every
//! analysis is a `Rule` emitting `Finding`s; a checking pipeline is a
//! `Policy` — a level per rule code and optional count limits — applied to
//! the findings, giving a `Verdict` with an exit code.

use crate::klepto::Klepto;
use crate::model::{Finding, Severity};
use crate::rules::{all_codes, RuleRunner};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// Dropped from the verdict.
    Allow,
    /// Reported, never fails.
    Warn,
    /// Reported; any occurrence fails.
    Deny,
}

/// Per-code levels and count limits, e.g.
///
/// ```toml
/// [levels]
/// KLEP012 = "allow"
/// KLEP013 = "warn"
/// [max]
/// KLEP011 = 0
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    #[serde(default)]
    pub levels: BTreeMap<String, Level>,
    /// Fails when a code's count exceeds the limit, whatever its level.
    #[serde(default)]
    pub max: BTreeMap<String, usize>,
}

#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    #[error("policy: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("policy names unknown rule code `{0}`")]
    UnknownCode(String),
}

impl Policy {
    pub fn parse(s: &str) -> Result<Policy, PolicyError> {
        let p: Policy = toml::from_str(s)?;
        let known: Vec<&str> = all_codes().into_iter().map(|(c, _)| c).collect();
        for code in p.levels.keys().chain(p.max.keys()) {
            if !known.contains(&code.as_str()) {
                return Err(PolicyError::UnknownCode(code.clone()));
            }
        }
        Ok(p)
    }

    /// Every code allowed: the verdict is always exit 0.
    pub fn permissive() -> Policy {
        Policy { levels: all_codes().into_iter().map(|(c, _)| (c.to_string(), Level::Allow)).collect(), max: BTreeMap::new() }
    }

    /// The policy's level, else the finding's severity — Deny → deny,
    /// Warn → warn, Info → allow.
    pub fn level_for(&self, code: &str, severity: &Severity) -> Level {
        self.levels.get(code).copied().unwrap_or(match severity {
            Severity::Deny => Level::Deny,
            Severity::Warn => Level::Warn,
            Severity::Info => Level::Allow,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Violation {
    pub code: String,
    pub count: usize,
    /// The exceeded `max`, when that is what failed.
    pub limit: Option<usize>,
    pub level: Level,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verdict {
    /// Findings at Warn or Deny level, in stable order.
    pub findings: Vec<Finding>,
    /// Per code, over `findings`.
    pub counts: BTreeMap<String, usize>,
    pub violations: Vec<Violation>,
    /// 0 when `violations` is empty, else 1.
    pub exit_code: i32,
}

impl Verdict {
    /// One line per code — `CODE name: count (level)` — then `exit N`.
    pub fn summary(&self) -> String {
        let names: BTreeMap<&str, &str> = all_codes().into_iter().collect();
        let mut out = String::new();
        for (code, count) in &self.counts {
            let level = self
                .violations
                .iter()
                .find(|v| &v.code == code)
                .map(|v| v.level)
                .unwrap_or(Level::Warn);
            let level = match level {
                Level::Deny => "deny",
                Level::Warn => "warn",
                Level::Allow => "allow",
            };
            out.push_str(&format!("{code} {}: {count} ({level})\n", names.get(code.as_str()).unwrap_or(&"?")));
        }
        out.push_str(&format!("exit {}", self.exit_code));
        out
    }
}

/// Runs every built-in rule and applies `policy` to the findings.
pub fn run_pipeline(k: &Klepto, policy: &Policy) -> Verdict {
    let all = RuleRunner::new(k).with_all_rules().run();
    let mut findings = Vec::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut levels: BTreeMap<String, Level> = BTreeMap::new();
    for f in all {
        let level = policy.level_for(&f.code, &f.severity);
        if level == Level::Allow {
            continue;
        }
        *counts.entry(f.code.clone()).or_default() += 1;
        levels.insert(f.code.clone(), level);
        findings.push(f);
    }
    let mut violations = Vec::new();
    for (code, count) in &counts {
        let level = levels.get(code).copied().unwrap_or(Level::Warn);
        let limit = policy.max.get(code).copied();
        if level == Level::Deny {
            violations.push(Violation { code: code.clone(), count: *count, limit: None, level });
        } else if limit.is_some_and(|m| *count > m) {
            violations.push(Violation { code: code.clone(), count: *count, limit, level });
        }
    }
    let exit_code = if violations.is_empty() { 0 } else { 1 };
    Verdict { findings, counts, violations, exit_code }
}
