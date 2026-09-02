//! Pattern mining — function shapes at five abstraction levels,
//! clusters of same-shaped functions, the parameters that differ inside a
//! cluster, candidate refactorings, impl-block mining and per-function
//! metrics.
//!
//! klepto proposes, never rewrites. Mining needs the `span-locations`
//! feature (default): tokens are classified by span, so without it only
//! the `Exact` level is computed.
//!
//! Out of scope: semantic equivalence beyond token shapes (reordered
//! statements, renamed fields), clones inside macro bodies (macro
//! arguments are opaque), cross-crate patterns, suggesting names.

use crate::syntax::extract::path_string;
use crate::klepto::Klepto;
use crate::model::{CapturedFn, FnKind, Resolution};
use proc_macro2::{Delimiter, Spacing, Span, TokenStream, TokenTree};
use quote::ToTokens;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use syn::spanned::Spanned;
use syn::visit::Visit;

/// Abstraction levels, strictest first.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ShapeLevel {
    /// Tokens as written, whitespace normalised.
    Exact,
    /// Parameters and locals renamed by first occurrence.
    Renamed,
    /// Renamed, plus literals replaced by slots.
    Literals,
    /// Literals, plus type names replaced by slots.
    Types,
    /// Types, plus callee names replaced by slots.
    Callees,
}

pub const SHAPE_LEVELS: [ShapeLevel; 5] = [ShapeLevel::Exact, ShapeLevel::Renamed, ShapeLevel::Literals, ShapeLevel::Types, ShapeLevel::Callees];

impl ShapeLevel {
    pub fn name(self) -> &'static str {
        match self {
            ShapeLevel::Exact => "exact",
            ShapeLevel::Renamed => "renamed",
            ShapeLevel::Literals => "literals",
            ShapeLevel::Types => "types",
            ShapeLevel::Callees => "callees",
        }
    }

    fn index(self) -> usize {
        match self {
            ShapeLevel::Exact => 0,
            ShapeLevel::Renamed => 1,
            ShapeLevel::Literals => 2,
            ShapeLevel::Types => 3,
            ShapeLevel::Callees => 4,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MiningOptions {
    /// Functions with fewer significant tokens in signature + body are not
    /// shaped.
    pub min_tokens: usize,
    /// Smallest cluster reported.
    pub min_members: usize,
}

impl Default for MiningOptions {
    fn default() -> Self {
        Self { min_tokens: 16, min_members: 2 }
    }
}

/// One placeholder and what it stands for in one function.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Slot {
    /// `$v0`, `$l0`, `$t0`, `$f0`.
    pub placeholder: String,
    /// The concrete token(s) it replaced.
    pub filler: String,
}

/// A function's shapes at every level.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FnShape {
    pub id: String,
    /// Significant tokens of signature + body.
    pub tokens: usize,
    pub shapes: BTreeMap<ShapeLevel, String>,
    /// blake3 hex of each shape.
    pub hashes: BTreeMap<ShapeLevel, String>,
    /// Slots introduced at each level, in first-occurrence order.
    pub slots: BTreeMap<ShapeLevel, Vec<Slot>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ParamKind {
    Literal,
    Type,
    Callee,
}

/// A slot whose fillers differ across a cluster's members.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Param {
    pub kind: ParamKind,
    pub placeholder: String,
    /// One filler per member, in `Cluster::members` order.
    pub fillers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cluster {
    pub level: ShapeLevel,
    pub hash: String,
    /// Sorted item ids.
    pub members: Vec<String>,
    /// The first member.
    pub exemplar: String,
    /// No stricter level joins all these members.
    pub tightest: bool,
    pub params: Vec<Param>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    /// Same code: keep one, call it from the others.
    ExtractFn,
    /// Same code modulo literals: pass them as arguments or const generics.
    Parameterize,
    /// Same code modulo types: one generic fn with the listed bounds.
    GenericFn,
    /// Same-named inherent methods on several types: a trait.
    Trait,
    /// Same code modulo callees: `macro_rules!` with `$f:path` parameters.
    MacroRules,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub kind: CandidateKind,
    pub level: ShapeLevel,
    pub members: Vec<String>,
    pub params: Vec<Param>,
    /// Type placeholder → method names called on values of that type
    /// (from the receiver types), sorted.
    pub required_methods: BTreeMap<String, Vec<String>>,
    /// Trait candidates: the shared method name.
    pub method: Option<String>,
    /// Trait candidates: the self types, sorted.
    pub types: Vec<String>,
    /// `members × exemplar tokens`.
    pub weight: usize,
    pub note: String,
}

/// Types whose inherent impls all provide the same method names.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SharedMethodSet {
    pub methods: Vec<String>,
    pub types: Vec<String>,
}

/// One trait implemented for several types with the same bodies.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepeatedImpl {
    pub trait_id: String,
    pub types: Vec<String>,
    pub impls: Vec<String>,
    pub level: ShapeLevel,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FnMetrics {
    pub tokens: usize,
    pub lines: usize,
    pub branches: usize,
    pub calls: usize,
    pub max_depth: usize,
    pub fan_in: usize,
    pub fan_out: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternReport {
    pub options: MiningOptions,
    pub shapes: Vec<FnShape>,
    pub clusters: Vec<Cluster>,
    pub candidates: Vec<Candidate>,
    pub shared_method_sets: Vec<SharedMethodSet>,
    pub repeated_impls: Vec<RepeatedImpl>,
    pub metrics: BTreeMap<String, FnMetrics>,
}

impl PatternReport {
    pub fn shape(&self, id: &str) -> Option<&FnShape> {
        self.shapes.iter().find(|s| s.id == id)
    }

    /// Every cluster `id` belongs to, strictest level first.
    pub fn clusters_of(&self, id: &str) -> Vec<&Cluster> {
        self.clusters.iter().filter(|c| c.members.iter().any(|m| m == id)).collect()
    }

    pub fn to_json_string(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// spans
// ---------------------------------------------------------------------------

type Pos = (usize, usize);

#[cfg(feature = "span-locations")]
const HAVE_SPANS: bool = true;
#[cfg(not(feature = "span-locations"))]
const HAVE_SPANS: bool = false;

#[cfg(feature = "span-locations")]
fn pos(span: Span) -> Option<Pos> {
    let s = span.start();
    Some((s.line, s.column))
}

#[cfg(not(feature = "span-locations"))]
fn pos(_span: Span) -> Option<Pos> {
    None
}

#[cfg(feature = "span-locations")]
fn line_range(span: Span) -> (usize, usize) {
    (span.start().line, span.end().line)
}

#[cfg(not(feature = "span-locations"))]
fn line_range(_span: Span) -> (usize, usize) {
    (0, 0)
}

const PRIMITIVES: &[&str] = &[
    "bool", "char", "str", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32", "f64",
];

fn is_upper(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}

// ---------------------------------------------------------------------------
// Token classes gathered by span
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Classes {
    /// Names bound by patterns (parameters, lets, closure params); never
    /// uppercase-initial ones (unit variants and consts look like bindings).
    locals: HashSet<String>,
    literal: HashSet<Pos>,
    ty: HashSet<Pos>,
    /// Callee ident span → spelling (full path for path calls).
    callee: HashMap<Pos, String>,
    field: HashSet<Pos>,
}

impl<'ast> Visit<'ast> for Classes {
    fn visit_pat_ident(&mut self, p: &'ast syn::PatIdent) {
        let name = p.ident.to_string();
        if !is_upper(&name) {
            self.locals.insert(name);
        }
        syn::visit::visit_pat_ident(self, p);
    }

    fn visit_lit(&mut self, l: &'ast syn::Lit) {
        if let Some(p) = pos(l.span()) {
            self.literal.insert(p);
        }
    }

    fn visit_type_path(&mut self, tp: &'ast syn::TypePath) {
        for seg in &tp.path.segments {
            if let Some(p) = pos(seg.ident.span()) {
                self.ty.insert(p);
            }
        }
        syn::visit::visit_type_path(self, tp);
    }

    fn visit_expr_call(&mut self, c: &'ast syn::ExprCall) {
        if let syn::Expr::Path(p) = &*c.func {
            if p.qself.is_none() {
                if let Some(seg) = p.path.segments.last() {
                    if let Some(pp) = pos(seg.ident.span()) {
                        self.callee.insert(pp, path_string(&p.path));
                    }
                }
            }
        }
        syn::visit::visit_expr_call(self, c);
    }

    fn visit_expr_method_call(&mut self, m: &'ast syn::ExprMethodCall) {
        if let Some(p) = pos(m.method.span()) {
            self.callee.insert(p, m.method.to_string());
        }
        syn::visit::visit_expr_method_call(self, m);
    }

    fn visit_macro(&mut self, m: &'ast syn::Macro) {
        if let Some(seg) = m.path.segments.last() {
            if let Some(p) = pos(seg.ident.span()) {
                self.callee.insert(p, path_string(&m.path));
            }
        }
        syn::visit::visit_macro(self, m);
    }

    fn visit_member(&mut self, m: &'ast syn::Member) {
        let span = match m {
            syn::Member::Named(i) => i.span(),
            syn::Member::Unnamed(i) => i.span,
        };
        if let Some(p) = pos(span) {
            self.field.insert(p);
        }
    }
}

// ---------------------------------------------------------------------------
// The shaper
// ---------------------------------------------------------------------------

#[derive(Default)]
struct SlotMap {
    prefix: &'static str,
    index: HashMap<String, String>,
    slots: Vec<Slot>,
}

impl SlotMap {
    fn new(prefix: &'static str) -> Self {
        Self { prefix, ..Default::default() }
    }

    fn get(&mut self, filler: &str) -> String {
        if let Some(p) = self.index.get(filler) {
            return p.clone();
        }
        let p = format!("{}{}", self.prefix, self.slots.len());
        self.index.insert(filler.to_string(), p.clone());
        self.slots.push(Slot { placeholder: p.clone(), filler: filler.to_string() });
        p
    }
}

struct Shaper<'c> {
    classes: &'c Classes,
    texts: [Vec<String>; 5],
    tokens: usize,
    v: SlotMap,
    l: SlotMap,
    t: SlotMap,
    f: SlotMap,
}

impl<'c> Shaper<'c> {
    fn new(classes: &'c Classes) -> Self {
        Self {
            classes,
            texts: Default::default(),
            tokens: 0,
            v: SlotMap::new("$v"),
            l: SlotMap::new("$l"),
            t: SlotMap::new("$t"),
            f: SlotMap::new("$f"),
        }
    }

    fn push_all(&mut self, s: &str) {
        for t in self.texts.iter_mut() {
            t.push(s.to_string());
        }
        self.tokens += 1;
    }

    fn push(&mut self, per_level: [String; 5]) {
        for (t, s) in self.texts.iter_mut().zip(per_level) {
            t.push(s);
        }
        self.tokens += 1;
    }

    fn ident(&mut self, name: &str, span: Span, opaque: bool) {
        if opaque {
            return self.push_all(name);
        }
        let p = pos(span);
        let is_field = p.is_some_and(|p| self.classes.field.contains(&p));
        let callee = if is_field { None } else { p.and_then(|p| self.classes.callee.get(&p).cloned()) };
        let is_type = !is_field
            && callee.is_none()
            && (p.is_some_and(|p| self.classes.ty.contains(&p)) || (name != "Self" && is_upper(name)) || PRIMITIVES.contains(&name));
        let is_local = !is_field && callee.is_none() && !is_type && self.classes.locals.contains(name);
        let exact = name.to_string();
        let renamed = if is_local { self.v.get(name) } else { exact.clone() };
        let literals = renamed.clone();
        let types = if is_type { self.t.get(name) } else { literals.clone() };
        let callees = match &callee {
            Some(spelling) => self.f.get(spelling),
            None => types.clone(),
        };
        self.push([exact, renamed, literals, types, callees]);
    }

    fn literal(&mut self, text: &str, opaque: bool) {
        if opaque {
            return self.push_all(text);
        }
        let l = self.l.get(text);
        self.push([text.to_string(), text.to_string(), l.clone(), l.clone(), l]);
    }

    fn group(&mut self, g: &proc_macro2::Group, opaque: bool) {
        let (open, close) = match g.delimiter() {
            Delimiter::Parenthesis => ("(", ")"),
            Delimiter::Brace => ("{", "}"),
            Delimiter::Bracket => ("[", "]"),
            Delimiter::None => return self.emit(g.stream(), opaque),
        };
        self.push_all(open);
        self.emit(g.stream(), opaque);
        self.push_all(close);
    }

    /// Joint punctuation stays one token (`::`, `->`, `+=`, `&&`);
    /// `name ! ( .. )` is a macro invocation whose arguments are opaque.
    fn emit(&mut self, ts: TokenStream, opaque: bool) {
        let trees: Vec<TokenTree> = ts.into_iter().collect();
        let mut pending = String::new();
        let mut i = 0;
        while i < trees.len() {
            if let TokenTree::Punct(p) = &trees[i] {
                pending.push(p.as_char());
                if p.spacing() == Spacing::Alone {
                    let s = std::mem::take(&mut pending);
                    self.push_all(&s);
                }
                i += 1;
                continue;
            }
            if !pending.is_empty() {
                let s = std::mem::take(&mut pending);
                self.push_all(&s);
            }
            match &trees[i] {
                TokenTree::Ident(id) => {
                    let name = id.to_string();
                    let is_macro = matches!(
                        (trees.get(i + 1), trees.get(i + 2)),
                        (Some(TokenTree::Punct(p)), Some(TokenTree::Group(_))) if p.as_char() == '!'
                    );
                    self.ident(&name, id.span(), opaque);
                    if is_macro {
                        self.push_all("!");
                        if let Some(TokenTree::Group(g)) = trees.get(i + 2) {
                            self.group(g, true);
                        }
                        i += 3;
                        continue;
                    }
                }
                TokenTree::Literal(l) => self.literal(&l.to_string(), opaque),
                TokenTree::Group(g) => self.group(g, opaque),
                TokenTree::Punct(_) => unreachable!(),
            }
            i += 1;
        }
        if !pending.is_empty() {
            let s = std::mem::take(&mut pending);
            self.push_all(&s);
        }
    }
}

fn hash(s: &str) -> String {
    blake3::hash(s.as_bytes()).to_hex().to_string()
}

/// One function's shapes at every level, its slots and its token count.
fn shape_fn(id: &str, sig: &syn::Signature, block: Option<&syn::Block>) -> FnShape {
    let mut classes = Classes::default();
    classes.visit_signature(sig);
    if let Some(b) = block {
        classes.visit_block(b);
    }
    let mut ts = TokenStream::new();
    let inputs = &sig.inputs;
    ts.extend(quote::quote! { ( #inputs ) });
    ts.extend(sig.output.to_token_stream());
    if let Some(b) = block {
        ts.extend(b.to_token_stream());
    }
    let mut sh = Shaper::new(&classes);
    sh.emit(ts, false);

    let mut shapes = BTreeMap::new();
    let mut hashes = BTreeMap::new();
    let mut slots = BTreeMap::new();
    let levels: &[ShapeLevel] = if HAVE_SPANS { &SHAPE_LEVELS } else { &SHAPE_LEVELS[..1] };
    for level in levels {
        let text = sh.texts[level.index()].join(" ");
        hashes.insert(*level, hash(&text));
        shapes.insert(*level, text);
        let s = match level {
            ShapeLevel::Exact => Vec::new(),
            ShapeLevel::Renamed => sh.v.slots.clone(),
            ShapeLevel::Literals => sh.l.slots.clone(),
            ShapeLevel::Types => sh.t.slots.clone(),
            ShapeLevel::Callees => sh.f.slots.clone(),
        };
        slots.insert(*level, s);
    }
    FnShape { id: id.to_string(), tokens: sh.tokens, shapes, hashes, slots }
}

// ---------------------------------------------------------------------------
// Metrics over a body
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Counts {
    branches: usize,
    calls: usize,
    depth: usize,
    max_depth: usize,
}

impl<'ast> Visit<'ast> for Counts {
    fn visit_expr_if(&mut self, e: &'ast syn::ExprIf) {
        self.branches += 1;
        syn::visit::visit_expr_if(self, e);
    }
    fn visit_expr_while(&mut self, e: &'ast syn::ExprWhile) {
        self.branches += 1;
        syn::visit::visit_expr_while(self, e);
    }
    fn visit_expr_loop(&mut self, e: &'ast syn::ExprLoop) {
        self.branches += 1;
        syn::visit::visit_expr_loop(self, e);
    }
    fn visit_expr_for_loop(&mut self, e: &'ast syn::ExprForLoop) {
        self.branches += 1;
        syn::visit::visit_expr_for_loop(self, e);
    }
    fn visit_expr_match(&mut self, e: &'ast syn::ExprMatch) {
        self.branches += e.arms.len().saturating_sub(1);
        syn::visit::visit_expr_match(self, e);
    }
    fn visit_expr_try(&mut self, e: &'ast syn::ExprTry) {
        self.branches += 1;
        syn::visit::visit_expr_try(self, e);
    }
    fn visit_expr_binary(&mut self, e: &'ast syn::ExprBinary) {
        if matches!(e.op, syn::BinOp::And(_) | syn::BinOp::Or(_)) {
            self.branches += 1;
        }
        syn::visit::visit_expr_binary(self, e);
    }
    fn visit_expr_call(&mut self, e: &'ast syn::ExprCall) {
        self.calls += 1;
        syn::visit::visit_expr_call(self, e);
    }
    fn visit_expr_method_call(&mut self, e: &'ast syn::ExprMethodCall) {
        self.calls += 1;
        syn::visit::visit_expr_method_call(self, e);
    }
    fn visit_expr_macro(&mut self, e: &'ast syn::ExprMacro) {
        self.calls += 1;
        syn::visit::visit_expr_macro(self, e);
    }
    fn visit_stmt_macro(&mut self, e: &'ast syn::StmtMacro) {
        self.calls += 1;
        syn::visit::visit_stmt_macro(self, e);
    }
    fn visit_block(&mut self, b: &'ast syn::Block) {
        self.depth += 1;
        self.max_depth = self.max_depth.max(self.depth);
        syn::visit::visit_block(self, b);
        self.depth -= 1;
    }
}

// ---------------------------------------------------------------------------
// finding every function in the ASTs
// ---------------------------------------------------------------------------

struct Found<'ast> {
    at: Pos,
    sig: &'ast syn::Signature,
    block: Option<&'ast syn::Block>,
    lines: (usize, usize),
}

#[derive(Default)]
struct Collector<'ast> {
    found: Vec<Found<'ast>>,
}

impl<'ast> Visit<'ast> for Collector<'ast> {
    fn visit_item_fn(&mut self, f: &'ast syn::ItemFn) {
        if let Some(at) = pos(f.span()) {
            self.found.push(Found { at, sig: &f.sig, block: Some(&f.block), lines: line_range(f.span()) });
        }
        syn::visit::visit_item_fn(self, f);
    }
    fn visit_impl_item_fn(&mut self, f: &'ast syn::ImplItemFn) {
        if let Some(at) = pos(f.span()) {
            self.found.push(Found { at, sig: &f.sig, block: Some(&f.block), lines: line_range(f.span()) });
        }
        syn::visit::visit_impl_item_fn(self, f);
    }
    fn visit_trait_item_fn(&mut self, f: &'ast syn::TraitItemFn) {
        if let Some(at) = pos(f.span()) {
            self.found.push(Found { at, sig: &f.sig, block: f.default.as_ref(), lines: line_range(f.span()) });
        }
        syn::visit::visit_trait_item_fn(self, f);
    }
}

// ---------------------------------------------------------------------------
// mining
// ---------------------------------------------------------------------------

fn last_segment(id: &str) -> &str {
    id.rsplit("::").next().unwrap_or(id)
}

fn all_equal<T: PartialEq>(items: &[T]) -> bool {
    items.windows(2).all(|w| w[0] == w[1])
}

/// Mines the whole scan: shapes and metrics per function, clusters per level,
/// their parameters, candidate refactorings, shared method sets and repeated
/// impls.
pub fn mine(k: &Klepto, options: &MiningOptions) -> PatternReport {
    // every CapturedFn by its recorded location
    let by_loc: HashMap<(PathBuf, u32, u32), &CapturedFn> = k
        .functions
        .iter()
        .filter_map(|f| Some(((f.location.path.clone(), f.location.line?, f.location.column?), f)))
        .collect();
    let fns: BTreeMap<&str, &CapturedFn> = k.functions.iter().map(|f| (f.fq_name.as_str(), f)).collect();

    // shapes and metrics over every function (threshold applied later)
    let mut all: BTreeMap<String, FnShape> = BTreeMap::new();
    let mut counts: BTreeMap<String, (Counts, usize, bool)> = BTreeMap::new();
    for pf in &k.files {
        let mut c = Collector::default();
        c.visit_file(&pf.ast);
        for f in c.found {
            let key = (pf.path.clone(), f.at.0 as u32, f.at.1 as u32);
            let Some(cf) = by_loc.get(&key) else { continue };
            let id = cf.fq_name.clone();
            let shape = shape_fn(&id, f.sig, f.block);
            let mut cnt = Counts::default();
            let mut empty = true;
            if let Some(b) = f.block {
                empty = b.stmts.is_empty();
                for s in &b.stmts {
                    cnt.visit_stmt(s);
                }
            }
            let lines = if f.lines == (0, 0) { 0 } else { f.lines.1 - f.lines.0 + 1 };
            counts.insert(id.clone(), (cnt, lines, empty));
            all.insert(id, shape);
        }
    }
    let mut metrics: BTreeMap<String, FnMetrics> = BTreeMap::new();
    for f in &k.functions {
        let id = f.fq_name.clone();
        let tokens = all.get(&id).map(|s| s.tokens).unwrap_or(0);
        let (branches, calls, max_depth, lines) = counts
            .get(&id)
            .map(|(c, lines, _)| (c.branches, c.calls, c.max_depth, *lines))
            .unwrap_or((0, 0, 0, 0));
        metrics.insert(
            id.clone(),
            FnMetrics {
                tokens,
                lines,
                branches,
                calls,
                max_depth,
                fan_in: k.graph.dependents_of(&id).len(),
                fan_out: k.graph.dependencies_of(&id).len(),
            },
        );
    }

    // Shaped = big enough and not empty
    let shaped: Vec<&FnShape> = all
        .values()
        .filter(|s| s.tokens >= options.min_tokens && !counts.get(&s.id).is_some_and(|(_, _, empty)| *empty))
        .collect();
    let shape_of: HashMap<&str, &FnShape> = shaped.iter().map(|s| (s.id.as_str(), *s)).collect();
    let mut clusters: Vec<Cluster> = Vec::new();
    let levels: &[ShapeLevel] = if HAVE_SPANS { &SHAPE_LEVELS } else { &SHAPE_LEVELS[..1] };
    for level in levels {
        let mut by: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for s in &shaped {
            by.entry(s.hashes[level].as_str()).or_default().push(s.id.clone());
        }
        for (h, mut members) in by {
            if members.len() < options.min_members {
                continue;
            }
            members.sort();
            let tightest = !levels.iter().take_while(|l| *l < level).any(|l| {
                let hs: Vec<&str> = members.iter().filter_map(|m| shape_of.get(m.as_str())).map(|s| s.hashes[l].as_str()).collect();
                hs.len() == members.len() && all_equal(&hs)
            });
            let params = cluster_params(*level, &members, &shape_of);
            clusters.push(Cluster { level: *level, hash: h.to_string(), exemplar: members[0].clone(), members, tightest, params });
        }
    }
    clusters.sort_by(|a, b| {
        (a.level, std::cmp::Reverse(a.members.len()), &a.hash).cmp(&(b.level, std::cmp::Reverse(b.members.len()), &b.hash))
    });

    // method id → self type id, from the impl records
    let mut self_type_of: HashMap<&str, (&str, bool)> = HashMap::new(); // (type id, is trait impl)
    for i in &k.impls {
        if let Some(Resolution::Local(t)) = i.self_ty.mentions.first() {
            for m in &i.methods {
                self_type_of.insert(m.as_str(), (t.as_str(), i.trait_.is_some()));
            }
        }
    }
    let mut candidates: Vec<Candidate> = Vec::new();
    for c in clusters.iter().filter(|c| c.tightest) {
        let members: Vec<&CapturedFn> = c.members.iter().filter_map(|m| fns.get(m.as_str()).copied()).collect();
        let n = c.members.len();
        let tokens = shape_of.get(c.exemplar.as_str()).map(|s| s.tokens).unwrap_or(0);
        let weight = n * tokens;
        let inherent_same_name = members.len() == n
            && members.iter().all(|f| matches!(&f.kind, FnKind::ImplMethod { trait_ty: None, .. }))
            && all_equal(&members.iter().map(|f| f.name.as_str()).collect::<Vec<_>>());
        let types: BTreeSet<String> = c
            .members
            .iter()
            .filter_map(|m| self_type_of.get(m.as_str()).map(|(t, _)| t.to_string()))
            .collect();
        let (kind, note, method, types) = if inherent_same_name && types.len() >= 2 {
            let method = members[0].name.clone();
            (
                CandidateKind::Trait,
                format!(
                    "Inherent method `{method}` has the same shape on {} types: a trait with `{method}`{}.",
                    types.len(),
                    if c.level <= ShapeLevel::Renamed { " (a default body is possible: the bodies are identical)" } else { "" }
                ),
                Some(method),
                types.into_iter().collect::<Vec<_>>(),
            )
        } else {
            let (kind, note) = match c.level {
                ShapeLevel::Exact | ShapeLevel::Renamed => {
                    (CandidateKind::ExtractFn, format!("Same code in {n} functions: keep one and call it from the others."))
                }
                ShapeLevel::Literals => (
                    CandidateKind::Parameterize,
                    format!("Same code in {n} functions except {} literal(s): pass them as arguments or const generics.", c.params.len()),
                ),
                ShapeLevel::Types => (
                    CandidateKind::GenericFn,
                    format!(
                        "Same code in {n} functions except the type(s) {}: one generic function.",
                        c.params.iter().filter(|p| p.kind == ParamKind::Type).map(|p| p.fillers.join("/")).collect::<Vec<_>>().join(", ")
                    ),
                ),
                ShapeLevel::Callees => (
                    CandidateKind::MacroRules,
                    format!(
                        "Same code in {n} functions except the callee(s) {}: a macro_rules! taking them as $f:path, or a function taking a closure.",
                        c.params.iter().filter(|p| p.kind == ParamKind::Callee).map(|p| p.fillers.join("/")).collect::<Vec<_>>().join(", ")
                    ),
                ),
            };
            (kind, note, None, Vec::new())
        };
        let mut required_methods: BTreeMap<String, Vec<String>> = BTreeMap::new();
        if kind == CandidateKind::GenericFn {
            for p in c.params.iter().filter(|p| p.kind == ParamKind::Type) {
                let mut names: BTreeSet<String> = BTreeSet::new();
                for (i, member) in c.members.iter().enumerate() {
                    let filler = &p.fillers[i];
                    for call in k.calls.iter().filter(|x| x.is_method && x.enclosing_fn.as_deref() == Some(member.as_str())) {
                        let Some(Resolution::Local(t)) = &call.receiver_type else { continue };
                        let matches = k.type_decl(t).map(|d| d.name == *filler).unwrap_or(false) || last_segment(t) == filler;
                        if matches {
                            names.insert(call.callee.clone());
                        }
                    }
                }
                required_methods.insert(p.placeholder.clone(), names.into_iter().collect());
            }
        }
        candidates.push(Candidate {
            kind,
            level: c.level,
            members: c.members.clone(),
            params: c.params.clone(),
            required_methods,
            method,
            types,
            weight,
            note,
        });
    }
    candidates.sort_by(|a, b| (std::cmp::Reverse(a.weight), &a.members[0]).cmp(&(std::cmp::Reverse(b.weight), &b.members[0])));
    let mut methods_by_type: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for i in &k.impls {
        if i.trait_.is_some() {
            continue;
        }
        if let Some(Resolution::Local(t)) = i.self_ty.mentions.first() {
            let e = methods_by_type.entry(t.clone()).or_default();
            e.extend(i.methods.iter().map(|m| last_segment(m).to_string()));
        }
    }
    let shared_method_sets = shared_method_sets(&methods_by_type);
    let repeated_impls = repeated_impls(k, &all, levels);

    PatternReport {
        options: options.clone(),
        shapes: shaped.into_iter().cloned().collect(),
        clusters,
        candidates,
        shared_method_sets,
        repeated_impls,
        metrics,
    }
}

/// The parameters that differ between a cluster's members, one placeholder
/// family per level.
fn cluster_params(level: ShapeLevel, members: &[String], shape_of: &HashMap<&str, &FnShape>) -> Vec<Param> {
    let families: &[(ShapeLevel, ParamKind)] = match level {
        ShapeLevel::Exact | ShapeLevel::Renamed => &[],
        ShapeLevel::Literals => &[(ShapeLevel::Literals, ParamKind::Literal)],
        ShapeLevel::Types => &[(ShapeLevel::Literals, ParamKind::Literal), (ShapeLevel::Types, ParamKind::Type)],
        ShapeLevel::Callees => &[
            (ShapeLevel::Literals, ParamKind::Literal),
            (ShapeLevel::Types, ParamKind::Type),
            (ShapeLevel::Callees, ParamKind::Callee),
        ],
    };
    let shapes: Vec<&FnShape> = members.iter().filter_map(|m| shape_of.get(m.as_str()).copied()).collect();
    if shapes.len() != members.len() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (family, kind) in families {
        let Some(first) = shapes[0].slots.get(family) else { continue };
        for (j, slot) in first.iter().enumerate() {
            let fillers: Vec<String> = shapes.iter().filter_map(|s| s.slots.get(family).and_then(|v| v.get(j)).map(|x| x.filler.clone())).collect();
            if fillers.len() != shapes.len() || all_equal(&fillers) {
                continue;
            }
            out.push(Param { kind: *kind, placeholder: slot.placeholder.clone(), fillers });
        }
    }
    out
}

/// Closed (methods, types) pairs, seeded by pairwise intersections
/// and closed under further intersection.
fn shared_method_sets(methods_by_type: &BTreeMap<String, BTreeSet<String>>) -> Vec<SharedMethodSet> {
    let types: Vec<(&String, &BTreeSet<String>)> = methods_by_type.iter().collect();
    let mut seeds: BTreeSet<BTreeSet<String>> = BTreeSet::new();
    for (i, (_, a)) in types.iter().enumerate() {
        for (_, b) in types.iter().skip(i + 1) {
            let s: BTreeSet<String> = a.intersection(b).cloned().collect();
            if s.len() >= 2 {
                seeds.insert(s);
            }
        }
    }
    loop {
        let mut grew = false;
        for s in seeds.clone() {
            for (_, m) in &types {
                let t: BTreeSet<String> = s.intersection(m).cloned().collect();
                if t.len() >= 2 && seeds.insert(t) {
                    grew = true;
                }
            }
        }
        if !grew {
            break;
        }
    }
    let mut out: BTreeSet<(Vec<String>, Vec<String>)> = BTreeSet::new();
    for s in seeds {
        let group: Vec<String> = types.iter().filter(|(_, m)| s.is_subset(m)).map(|(t, _)| (*t).clone()).collect();
        if group.len() < 2 {
            continue;
        }
        let closed: BTreeSet<String> = group
            .iter()
            .filter_map(|t| methods_by_type.get(t))
            .fold(None::<BTreeSet<String>>, |acc, m| Some(match acc {
                None => m.clone(),
                Some(a) => a.intersection(m).cloned().collect(),
            }))
            .unwrap_or_default();
        if closed == s {
            out.insert((s.into_iter().collect(), group));
        }
    }
    let mut v: Vec<SharedMethodSet> = out.into_iter().map(|(methods, types)| SharedMethodSet { methods, types }).collect();
    v.sort_by(|a, b| {
        (std::cmp::Reverse(a.types.len()), std::cmp::Reverse(a.methods.len()), &a.types).cmp(&(
            std::cmp::Reverse(b.types.len()),
            std::cmp::Reverse(b.methods.len()),
            &b.types,
        ))
    });
    v
}

/// One trait implemented for several types with same-shaped bodies.
fn repeated_impls(k: &Klepto, all: &BTreeMap<String, FnShape>, levels: &[ShapeLevel]) -> Vec<RepeatedImpl> {
    let mut by_trait: BTreeMap<String, Vec<&crate::typing::types::ImplDecl>> = BTreeMap::new();
    for i in &k.impls {
        let Some(tr) = i.trait_.as_ref().and_then(|t| t.mentions.first()) else { continue };
        let trait_id = match tr {
            Resolution::Local(s) | Resolution::External(s) => s.clone(),
            _ => continue,
        };
        if i.methods.is_empty() {
            continue;
        }
        by_trait.entry(trait_id).or_default().push(i);
    }
    let self_type = |i: &crate::typing::types::ImplDecl| -> String {
        match i.self_ty.mentions.first() {
            Some(Resolution::Local(t)) | Some(Resolution::External(t)) => t.clone(),
            _ => i.self_ty.raw.clone(),
        }
    };
    let mut out = Vec::new();
    for (trait_id, impls) in by_trait {
        if impls.len() < 2 {
            continue;
        }
        let mut seen: BTreeSet<Vec<String>> = BTreeSet::new();
        for level in levels {
            let mut parts: BTreeMap<Vec<(String, String)>, Vec<&crate::typing::types::ImplDecl>> = BTreeMap::new();
            for i in &impls {
                let mut sig: Vec<(String, String)> = i
                    .methods
                    .iter()
                    .map(|m| (last_segment(m).to_string(), all.get(m).and_then(|s| s.hashes.get(level)).cloned().unwrap_or_default()))
                    .collect();
                sig.sort();
                parts.entry(sig).or_default().push(i);
            }
            for group in parts.values() {
                if group.len() < 2 {
                    continue;
                }
                let mut ids: Vec<String> = group.iter().map(|i| i.id.clone()).collect();
                ids.sort();
                if !seen.insert(ids.clone()) {
                    continue;
                }
                let mut types: Vec<String> = group.iter().map(|i| self_type(i)).collect();
                types.sort();
                types.dedup();
                out.push(RepeatedImpl { trait_id: trait_id.clone(), types, impls: ids, level: *level });
            }
        }
    }
    out.sort_by(|a, b| (&a.trait_id, a.level, &a.impls).cmp(&(&b.trait_id, b.level, &b.impls)));
    out
}

impl Klepto {
    /// `mine` with default options.
    pub fn patterns(&self) -> PatternReport {
        mine(self, &MiningOptions::default())
    }
}
