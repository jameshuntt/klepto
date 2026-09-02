//! Phase 2: `#[cfg(..)]` predicates as boolean formulas.
//!
//! Rules C1–C6 and Q5 in `PHASE2_RULES.md`.

use proc_macro2::{Delimiter, TokenStream, TokenTree};
use quote::ToTokens;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A cfg predicate. `True` is the absence of any `#[cfg]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum CfgPredicate {
    #[default]
    True,
    /// `feature = "x"`
    Feature(String),
    /// `unix`, `test`, `debug_assertions`, …
    Key(String),
    /// `target_os = "linux"`, `target_pointer_width = "64"`, …
    KeyValue(String, String),
    All(Vec<CfgPredicate>),
    Any(Vec<CfgPredicate>),
    Not(Box<CfgPredicate>),
    /// Could not be parsed; treated as "conditionally present", never absent.
    Unknown(String),
}

/// An assignment of cfg variables: active features plus the keys that hold
/// (`unix`, `test`; key-values spelled `target_os=linux`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CfgEnv {
    pub features: BTreeSet<String>,
    pub keys: BTreeSet<String>,
}

impl CfgEnv {
    pub fn with_features<I, S>(features: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        CfgEnv { features: features.into_iter().map(Into::into).collect(), keys: BTreeSet::new() }
    }
}

/// Result of `a.implies(b)`: does `b` hold whenever `a` holds?
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Implication {
    Always,
    /// `a` and `b` are never true together.
    Never,
    /// `a` can hold while `b` does not; `counterexample` is one such assignment.
    Sometimes { counterexample: CfgEnv },
    /// An `Unknown` predicate is involved.
    Unknown,
}

/// Conjunction of every `#[cfg(..)]` among `attrs` (rule E1).
pub fn cfg_of_attrs(attrs: &[syn::Attribute]) -> CfgPredicate {
    let mut cfg = CfgPredicate::True;
    for a in attrs {
        if !a.path().is_ident("cfg") {
            continue;
        }
        let p = match &a.meta {
            syn::Meta::List(l) => CfgPredicate::parse_tokens(&l.tokens),
            other => CfgPredicate::Unknown(other.to_token_stream().to_string()),
        };
        cfg = cfg.and(p);
    }
    cfg
}

// ---------------------------------------------------------------- parsing (C1–C5)

fn unquote(lit: &str) -> Option<String> {
    lit.strip_prefix('"').and_then(|s| s.strip_suffix('"')).map(str::to_string)
}

/// `pred (',' pred)* ','?`
fn parse_seq(tokens: &[TokenTree]) -> Option<Vec<CfgPredicate>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let (p, next) = parse_one(tokens, i)?;
        out.push(p);
        i = next;
        if i < tokens.len() {
            match &tokens[i] {
                TokenTree::Punct(p) if p.as_char() == ',' => i += 1,
                _ => return None,
            }
        }
    }
    Some(out)
}

fn parse_one(tokens: &[TokenTree], i: usize) -> Option<(CfgPredicate, usize)> {
    let TokenTree::Ident(id) = tokens.get(i)? else { return None };
    let name = id.to_string();
    match tokens.get(i + 1) {
        Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis => {
            let inner: Vec<TokenTree> = g.stream().into_iter().collect();
            let ps = parse_seq(&inner)?;
            let p = match name.as_str() {
                "all" => if ps.is_empty() { CfgPredicate::True } else { CfgPredicate::All(ps) },
                "any" => if ps.is_empty() { CfgPredicate::Not(Box::new(CfgPredicate::True)) } else { CfgPredicate::Any(ps) },
                "not" => {
                    if ps.len() != 1 {
                        return None;
                    }
                    CfgPredicate::Not(Box::new(ps.into_iter().next()?))
                }
                _ => return None, // version(..), accessible(..), anything else
            };
            Some((p, i + 2))
        }
        Some(TokenTree::Punct(p)) if p.as_char() == '=' => {
            let TokenTree::Literal(lit) = tokens.get(i + 2)? else { return None };
            let v = unquote(&lit.to_string())?;
            let p = if name == "feature" { CfgPredicate::Feature(v) } else { CfgPredicate::KeyValue(name, v) };
            Some((p, i + 3))
        }
        _ => Some((CfgPredicate::Key(name), i + 1)),
    }
}

impl CfgPredicate {
    /// Rules C1–C5. Accepts the inside of `#[cfg(..)]`, `cfg(<pred>)` as
    /// cargo spells target-specific dependencies, or a bare target triple.
    pub fn parse_str(s: &str) -> CfgPredicate {
        let s = s.trim();
        if let Some(inner) = s.strip_prefix("cfg(").and_then(|r| r.strip_suffix(')')) {
            return CfgPredicate::parse_str(inner);
        }
        if !s.is_empty() && s.contains('-') && !s.contains(['(', ')', '=', ' ', ',']) {
            return CfgPredicate::KeyValue("target".into(), s.into());
        }
        match s.parse::<TokenStream>() {
            Ok(ts) => CfgPredicate::parse_tokens(&ts),
            Err(_) => CfgPredicate::Unknown(s.to_string()),
        }
    }

    /// Same grammar, from an attribute's argument tokens.
    pub fn parse_tokens(ts: &TokenStream) -> CfgPredicate {
        let tokens: Vec<TokenTree> = ts.clone().into_iter().collect();
        match parse_seq(&tokens) {
            Some(mut ps) if ps.len() == 1 => ps.pop().unwrap_or(CfgPredicate::True),
            _ => CfgPredicate::Unknown(ts.to_string()),
        }
    }

    /// Conjunction with `True` as identity and `All` flattened (rule C6).
    pub fn and(self, other: CfgPredicate) -> CfgPredicate {
        match (self, other) {
            (CfgPredicate::True, o) => o,
            (s, CfgPredicate::True) => s,
            (CfgPredicate::All(mut a), CfgPredicate::All(b)) => {
                a.extend(b);
                CfgPredicate::All(a)
            }
            (CfgPredicate::All(mut a), o) => {
                a.push(o);
                CfgPredicate::All(a)
            }
            (s, CfgPredicate::All(b)) => {
                let mut v = vec![s];
                v.extend(b);
                CfgPredicate::All(v)
            }
            (s, o) => CfgPredicate::All(vec![s, o]),
        }
    }

    /// Feature names mentioned anywhere in the predicate.
    pub fn features(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        self.walk(&mut |p| {
            if let CfgPredicate::Feature(f) = p {
                out.insert(f.clone());
            }
        });
        out
    }

    /// Non-feature variables, key-values spelled `key=value`.
    pub fn keys(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        self.walk(&mut |p| match p {
            CfgPredicate::Key(k) => {
                out.insert(k.clone());
            }
            CfgPredicate::KeyValue(k, v) => {
                out.insert(format!("{k}={v}"));
            }
            _ => {}
        });
        out
    }

    pub fn is_unknown(&self) -> bool {
        let mut found = false;
        self.walk(&mut |p| {
            if matches!(p, CfgPredicate::Unknown(_)) {
                found = true;
            }
        });
        found
    }

    /// Truth under `env`; `None` when an `Unknown` decides the outcome.
    pub fn eval(&self, env: &CfgEnv) -> Option<bool> {
        match self {
            CfgPredicate::True => Some(true),
            CfgPredicate::Feature(f) => Some(env.features.contains(f)),
            CfgPredicate::Key(k) => Some(env.keys.contains(k)),
            CfgPredicate::KeyValue(k, v) => Some(env.keys.contains(&format!("{k}={v}"))),
            CfgPredicate::Not(p) => p.eval(env).map(|b| !b),
            CfgPredicate::All(ps) => {
                let mut unknown = false;
                for p in ps {
                    match p.eval(env) {
                        Some(false) => return Some(false),
                        Some(true) => {}
                        None => unknown = true,
                    }
                }
                if unknown { None } else { Some(true) }
            }
            CfgPredicate::Any(ps) => {
                let mut unknown = false;
                for p in ps {
                    match p.eval(env) {
                        Some(true) => return Some(true),
                        Some(false) => {}
                        None => unknown = true,
                    }
                }
                if unknown { None } else { Some(false) }
            }
            CfgPredicate::Unknown(_) => None,
        }
    }

    /// Rule C7: the source spelling for `#[cfg(..)]`. Single-element
    /// `All`/`Any` render as their member; `True` renders as `all()`.
    pub fn render(&self) -> String {
        fn list(ps: &[CfgPredicate]) -> String {
            ps.iter().map(CfgPredicate::render).collect::<Vec<_>>().join(", ")
        }
        match self {
            CfgPredicate::True => "all()".into(),
            CfgPredicate::Feature(f) => format!("feature = \"{f}\""),
            CfgPredicate::Key(k) => k.clone(),
            CfgPredicate::KeyValue(k, v) => format!("{k} = \"{v}\""),
            CfgPredicate::All(ps) => match ps.as_slice() {
                [p] => p.render(),
                _ => format!("all({})", list(ps)),
            },
            CfgPredicate::Any(ps) => match ps.as_slice() {
                [p] => p.render(),
                _ => format!("any({})", list(ps)),
            },
            CfgPredicate::Not(p) => format!("not({})", p.render()),
            CfgPredicate::Unknown(raw) => raw.clone(),
        }
    }

    /// Rule C8: collapse single-element `All`/`Any`, flatten nested
    /// same-kind lists, drop duplicates (first occurrence kept), drop `True`
    /// from `All`, and let a `True` member absorb an `Any`.
    pub fn simplify(self) -> CfgPredicate {
        fn merge(out: &mut Vec<CfgPredicate>, q: CfgPredicate) {
            if !out.contains(&q) {
                out.push(q);
            }
        }
        match self {
            CfgPredicate::All(ps) => {
                let mut out: Vec<CfgPredicate> = Vec::new();
                for p in ps.into_iter().map(CfgPredicate::simplify) {
                    match p {
                        CfgPredicate::True => {}
                        CfgPredicate::All(inner) => inner.into_iter().for_each(|q| merge(&mut out, q)),
                        q => merge(&mut out, q),
                    }
                }
                match out.len() {
                    0 => CfgPredicate::True,
                    1 => out.pop().unwrap_or(CfgPredicate::True),
                    _ => CfgPredicate::All(out),
                }
            }
            CfgPredicate::Any(ps) => {
                let mut out: Vec<CfgPredicate> = Vec::new();
                for p in ps.into_iter().map(CfgPredicate::simplify) {
                    match p {
                        CfgPredicate::True => return CfgPredicate::True,
                        CfgPredicate::Any(inner) => inner.into_iter().for_each(|q| merge(&mut out, q)),
                        q => merge(&mut out, q),
                    }
                }
                match out.len() {
                    1 => out.pop().unwrap_or(CfgPredicate::True),
                    _ => CfgPredicate::Any(out),
                }
            }
            CfgPredicate::Not(p) => CfgPredicate::Not(Box::new(p.simplify())),
            other => other,
        }
    }

    /// Rule Q5: variables are independent booleans; `preferred` is tried
    /// first as the counterexample.
    pub fn implies(&self, other: &CfgPredicate, preferred: Option<&CfgEnv>) -> Implication {
        if self.is_unknown() || other.is_unknown() {
            return Implication::Unknown;
        }
        if let Some(env) = preferred {
            if self.eval(env) == Some(true) && other.eval(env) == Some(false) {
                return Implication::Sometimes { counterexample: env.clone() };
            }
        }
        let mut vars: Vec<(bool, String)> = Vec::new(); // (is_feature, name)
        for f in self.features().union(&other.features()) {
            vars.push((true, f.clone()));
        }
        for k in self.keys().union(&other.keys()) {
            vars.push((false, k.clone()));
        }
        if vars.len() > 16 {
            return syntactic_implication(self, other);
        }
        judge(vars.len(), |mask| env_from_mask(&vars, mask), self, other)
    }

    fn walk(&self, f: &mut dyn FnMut(&CfgPredicate)) {
        f(self);
        match self {
            CfgPredicate::All(ps) | CfgPredicate::Any(ps) => {
                for p in ps {
                    p.walk(f);
                }
            }
            CfgPredicate::Not(p) => p.walk(f),
            _ => {}
        }
    }
}

fn env_from_mask(vars: &[(bool, String)], mask: u32) -> CfgEnv {
    let mut env = CfgEnv::default();
    for (i, (is_feature, name)) in vars.iter().enumerate() {
        if mask & (1 << i) != 0 {
            if *is_feature {
                env.features.insert(name.clone());
            } else {
                env.keys.insert(name.clone());
            }
        }
    }
    env
}

/// Shared verdict logic over `2^n` assignments produced by `env_of`
/// (used by Q5 and, with closed assignments, by Q6).
pub(crate) fn judge(n: usize, env_of: impl Fn(u32) -> CfgEnv, a: &CfgPredicate, b: &CfgPredicate) -> Implication {
    let mut any_both = false;
    let mut first_fail: Option<CfgEnv> = None;
    let mut custom_fail = false;
    for mask in 0u32..(1u32 << n) {
        let env = env_of(mask);
        if a.eval(&env) != Some(true) {
            continue;
        }
        if b.eval(&env) == Some(true) {
            any_both = true;
        } else if env_sets_custom_key(&env) {
            custom_fail = true;
        } else if first_fail.is_none() {
            first_fail = Some(env);
        }
    }
    verdict(any_both, first_fail, custom_fail)
}

/// Rule Q8: a counterexample is definite only when every cfg key it sets is
/// one the compiler or cargo sets (`test`, `debug_assertions`, `target_*`,
/// …). A key only a build script or `RUSTFLAGS` could set (`no_serde_derive`,
/// `docsrs`) makes the verdict Unknown: klepto does not know who sets it.
pub(crate) fn verdict(any_both: bool, first_fail: Option<CfgEnv>, custom_fail: bool) -> Implication {
    if !any_both {
        Implication::Never
    } else if let Some(env) = first_fail {
        Implication::Sometimes { counterexample: env }
    } else if custom_fail {
        Implication::Unknown
    } else {
        Implication::Always
    }
}

/// Cfg keys the toolchain sets; anything else is a custom cfg (rule Q8).
pub fn is_builtin_cfg_key(key: &str) -> bool {
    let name = key.split('=').next().unwrap_or(key).trim();
    matches!(
        name,
        "test" | "debug_assertions" | "doc" | "doctest" | "miri" | "unix" | "windows" | "proc_macro" | "panic" | "overflow_checks" | "sanitize"
            | "ub_checks" | "target_os" | "target_arch" | "target_family" | "target_env" | "target_endian" | "target_pointer_width"
            | "target_vendor" | "target_feature" | "target_has_atomic" | "target_abi" | "target_thread_local" | "relocation_model" | "fmt_debug"
            | "clippy" | "rustfmt" | "rust_analyzer"
    )
}

pub(crate) fn env_sets_custom_key(env: &CfgEnv) -> bool {
    env.keys.iter().any(|k| !is_builtin_cfg_key(k))
}

/// Verdict over an explicit list of assignments (closed feature sets).
pub(crate) fn judge_envs(envs: &[CfgEnv], a: &CfgPredicate, b: &CfgPredicate) -> Implication {
    let mut any_both = false;
    let mut first_fail: Option<CfgEnv> = None;
    let mut custom_fail = false;
    for env in envs {
        if a.eval(env) != Some(true) {
            continue;
        }
        if b.eval(env) == Some(true) {
            any_both = true;
        } else if env_sets_custom_key(env) {
            custom_fail = true;
        } else if first_fail.is_none() {
            first_fail = Some(env.clone());
        }
    }
    verdict(any_both, first_fail, custom_fail)
}

/// Fallback past the enumeration cap: only obvious syntactic implication.
fn syntactic_implication(a: &CfgPredicate, b: &CfgPredicate) -> Implication {
    if *b == CfgPredicate::True || a == b {
        return Implication::Always;
    }
    if let CfgPredicate::All(ps) = a {
        if ps.iter().any(|p| p == b) {
            return Implication::Always;
        }
    }
    Implication::Unknown
}
