//! Macro expansion — items, impl items, statements and expressions
//! produced by `macro_rules!` exist for klepto.
//!
//! What is expanded: `macro_rules!` defined anywhere in a scanned package
//! (textual scope, crate-wide), the `cfg_if!` idiom (built in, exact), and
//! `#[macro_export]` `macro_rules!` of dependencies whose sources are on
//! disk (harvested from the cargo registry when `cargo metadata --offline`
//! can resolve them). What cannot be expanded: procedural macros — derive,
//! attribute and function-like proc macros run code, and klepto never runs
//! code. Their invocations stay as they are and are recorded as
//! `Outcome::ProcMacroOrUnknown`.
//!
//! Every expansion is recorded (`Klepto::expansions`) with its outcome, so
//! nothing is silently dropped: a rule that does not match, an expansion
//! that does not parse, a depth overrun — all listed, the invocation left
//! in place.

use proc_macro2::{Delimiter, Group, Ident, Span, TokenStream, TokenTree};
use quote::quote;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use syn::parse::{Parse, ParseStream, Parser};
use syn::visit_mut::VisitMut;

use crate::model::FileLocation;

/// Marker doc attribute put on every generated node.
pub const GENERATED_MARK: &str = "klepto:generated:";
/// Marker doc attribute put on the first node of an expansion, so the
/// invocation stays a macro occurrence.
pub const INVOKED_MARK: &str = "klepto:invoked:";

// ---------------------------------------------------------------------------
// definitions
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum MatchNode {
    /// A literal token (ident, punct, literal) to compare by spelling.
    Token(String),
    Group { delim: Delimiter, inner: Vec<MatchNode> },
    Frag { name: String, kind: String },
    Rep { inner: Vec<MatchNode>, sep: Option<String>, op: char },
}

#[derive(Clone, Debug)]
enum TransNode {
    Token(TokenTree),
    Group { delim: Delimiter, inner: Vec<TransNode> },
    Var(String),
    Rep { inner: Vec<TransNode>, sep: Option<TokenTree> },
    Crate,
}

#[derive(Clone, Debug)]
struct Rule {
    matcher: Vec<MatchNode>,
    transcriber: Vec<TransNode>,
}

/// One `macro_rules!` definition.
#[derive(Clone, Debug)]
pub struct MacroDef {
    pub name: String,
    /// Package (or dependency crate) that defines it, `-` → `_`.
    pub crate_name: String,
    pub exported: bool,
    /// A definition inside an inline module without `#[macro_use]`
    /// is visible only in that module (file and inline-module chain); `None`
    /// is crate-wide.
    pub scope: Option<(std::path::PathBuf, Vec<String>)>,
    /// The `#[cfg]`s on the definition and its enclosing inline modules: what
    /// the macro produces exists only under them (syn's
    /// `full!` has a twin under `not(feature = "full")`).
    cfg_attrs: Vec<syn::Attribute>,
    rules: Vec<Rule>,
}

/// Every definition klepto can expand, local ones first.
#[derive(Default, Clone, Debug)]
pub struct MacroTable {
    by_name: HashMap<String, Vec<MacroDef>>,
}

impl MacroTable {
    /// Adds a definition; a second definition of the same name in the same
    /// crate is ignored (the first one wins, as in textual scope).
    pub fn add(&mut self, def: MacroDef) {
        let defs = self.by_name.entry(def.name.clone()).or_default();
        if defs.iter().any(|d| d.crate_name == def.crate_name && d.scope == def.scope) {
            return;
        }
        defs.push(def);
    }

    pub fn len(&self) -> usize {
        self.by_name.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// Names and defining crates, sorted (for reports).
    pub fn names(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = self.by_name.values().flatten().map(|d| (d.name.clone(), d.crate_name.clone())).collect();
        v.sort();
        v
    }

    /// A bare name prefers the current package's definition, then a
    /// declared dependency's, then any; `crate_name::name` picks that crate.
    fn find(&self, path: &syn::Path, package: &str, deps: &[String], file: &std::path::Path, chain: &[String]) -> Option<&MacroDef> {
        let name = path.segments.last()?.ident.to_string();
        let all = self.by_name.get(&name)?;
        // scoped definitions apply only inside their inline module
        let mut defs: Vec<&MacroDef> = all.iter().filter(|d| d.scope.as_ref().is_none_or(|(f, c)| f == file && chain.starts_with(c))).collect();
        // the innermost scope first
        defs.sort_by_key(|d| std::cmp::Reverse(d.scope.as_ref().map(|(_, c)| c.len()).unwrap_or(0)));
        if path.segments.len() >= 2 {
            let first = path.segments[0].ident.to_string().replace('-', "_");
            let want = if first == "crate" || first == "self" || first == "super" { package.to_string() } else { first };
            // a qualified name means that crate and no other (hyper's `trace!` wraps
            // `tracing::trace!`; falling back to the local one would loop)
            return defs.iter().find(|d| d.crate_name == want).copied();
        }
        defs.iter()
            .find(|d| d.crate_name == package)
            .or_else(|| defs.iter().find(|d| deps.contains(&d.crate_name)))
            .or_else(|| defs.first())
            .copied()
    }

    /// Collects `macro_rules!` definitions from a file (recursing into inline
    /// modules); `only_exported` keeps `#[macro_export]` ones (dependencies).
    pub fn harvest(&mut self, ast: &syn::File, crate_name: &str, only_exported: bool, file: &std::path::Path) {
        fn walk(items: &[syn::Item], crate_name: &str, only_exported: bool, file: &std::path::Path, chain: &[String], cfgs: &[syn::Attribute], table: &mut MacroTable) {
            for it in items {
                match it {
                    syn::Item::Macro(m) if m.mac.path.is_ident("macro_rules") => {
                        let Some(ident) = &m.ident else { continue };
                        let exported = m.attrs.iter().any(|a| a.path().is_ident("macro_export"));
                        if only_exported && !exported {
                            continue;
                        }
                        if let Some(rules) = parse_rules(&m.mac.tokens) {
                            let scope = if chain.is_empty() || exported { None } else { Some((file.to_path_buf(), chain.to_vec())) };
                            let mut cfg_attrs = cfgs.to_vec();
                            cfg_attrs.extend(m.attrs.iter().filter(|a| a.path().is_ident("cfg")).cloned());
                            table.add(MacroDef { name: ident.to_string(), crate_name: crate_name.to_string(), exported, scope, cfg_attrs, rules });
                        }
                    }
                    syn::Item::Mod(md) => {
                        if let Some((_, inner)) = &md.content {
                            // `#[macro_use]` lifts the module's macros into the parent
                            let lifted = md.attrs.iter().any(|a| a.path().is_ident("macro_use"));
                            let mut c = chain.to_vec();
                            if !lifted {
                                c.push(md.ident.to_string());
                            }
                            let mut mod_cfgs = cfgs.to_vec();
                            mod_cfgs.extend(md.attrs.iter().filter(|a| a.path().is_ident("cfg")).cloned());
                            walk(inner, crate_name, only_exported, file, &c, &mod_cfgs, table);
                        }
                    }
                    _ => {}
                }
            }
        }
        walk(&ast.items, crate_name, only_exported, file, &[], &[], self);
    }
}

fn trees(ts: &TokenStream) -> Vec<TokenTree> {
    ts.clone().into_iter().collect()
}

/// `( pattern ) => { body } ;` sequences.
fn parse_rules(ts: &TokenStream) -> Option<Vec<Rule>> {
    let t = trees(ts);
    let mut rules = Vec::new();
    let mut i = 0;
    while i < t.len() {
        let TokenTree::Group(pat) = &t[i] else { return None };
        let is_arrow = matches!((t.get(i + 1), t.get(i + 2)), (Some(TokenTree::Punct(a)), Some(TokenTree::Punct(b))) if a.as_char() == '=' && b.as_char() == '>');
        if !is_arrow {
            return None;
        }
        let Some(TokenTree::Group(body)) = t.get(i + 3) else { return None };
        let matcher = parse_matcher(&trees(&pat.stream()))?;
        let transcriber = parse_transcriber(&trees(&body.stream()));
        rules.push(Rule { matcher, transcriber });
        i += 4;
        if let Some(TokenTree::Punct(p)) = t.get(i) {
            if p.as_char() == ';' {
                i += 1;
            }
        }
    }
    Some(rules)
}

fn is_dollar(t: &TokenTree) -> bool {
    matches!(t, TokenTree::Punct(p) if p.as_char() == '$')
}

fn parse_matcher(t: &[TokenTree]) -> Option<Vec<MatchNode>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < t.len() {
        if is_dollar(&t[i]) {
            match t.get(i + 1) {
                Some(TokenTree::Ident(name)) => {
                    let colon = matches!(t.get(i + 2), Some(TokenTree::Punct(p)) if p.as_char() == ':');
                    let Some(TokenTree::Ident(kind)) = t.get(i + 3) else { return None };
                    if !colon {
                        return None;
                    }
                    out.push(MatchNode::Frag { name: name.to_string(), kind: kind.to_string() });
                    i += 4;
                }
                Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis => {
                    let inner = parse_matcher(&trees(&g.stream()))?;
                    let (sep, op, used) = rep_suffix(t, i + 2)?;
                    out.push(MatchNode::Rep { inner, sep, op });
                    i += 2 + used;
                }
                // a bare `$` is a literal token (syn's `Token![$]`)
                _ => {
                    out.push(MatchNode::Token("$".into()));
                    i += 1;
                }
            }
            continue;
        }
        match &t[i] {
            TokenTree::Group(g) => out.push(MatchNode::Group { delim: g.delimiter(), inner: parse_matcher(&trees(&g.stream()))? }),
            other => out.push(MatchNode::Token(other.to_string())),
        }
        i += 1;
    }
    Some(out)
}

/// After `$( ... )`: an optional separator token then `*`, `+` or `?`.
fn rep_suffix(t: &[TokenTree], at: usize) -> Option<(Option<String>, char, usize)> {
    match t.get(at) {
        Some(TokenTree::Punct(p)) if matches!(p.as_char(), '*' | '+' | '?') => Some((None, p.as_char(), 1)),
        Some(sep) => match t.get(at + 1) {
            Some(TokenTree::Punct(p)) if matches!(p.as_char(), '*' | '+' | '?') => Some((Some(sep.to_string()), p.as_char(), 2)),
            _ => None,
        },
        None => None,
    }
}

fn parse_transcriber(t: &[TokenTree]) -> Vec<TransNode> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < t.len() {
        if is_dollar(&t[i]) {
            match t.get(i + 1) {
                Some(TokenTree::Ident(name)) if name == "crate" => {
                    out.push(TransNode::Crate);
                    i += 2;
                    continue;
                }
                Some(TokenTree::Ident(name)) => {
                    out.push(TransNode::Var(name.to_string()));
                    i += 2;
                    continue;
                }
                Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis => {
                    let inner = parse_transcriber(&trees(&g.stream()));
                    let (sep, used) = match t.get(i + 2) {
                        Some(TokenTree::Punct(p)) if matches!(p.as_char(), '*' | '+' | '?') => (None, 1),
                        Some(sep) => match t.get(i + 3) {
                            Some(TokenTree::Punct(p)) if matches!(p.as_char(), '*' | '+' | '?') => (Some(sep.clone()), 2),
                            _ => (None, 0),
                        },
                        None => (None, 0),
                    };
                    out.push(TransNode::Rep { inner, sep });
                    i += 2 + used;
                    continue;
                }
                _ => {}
            }
        }
        match &t[i] {
            TokenTree::Group(g) => out.push(TransNode::Group { delim: g.delimiter(), inner: parse_transcriber(&trees(&g.stream())) }),
            other => out.push(TransNode::Token(other.clone())),
        }
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// matching
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Bind {
    Leaf(TokenStream),
    Seq(Vec<Bind>),
}

type Binds = HashMap<String, Bind>;

/// How many top-level trees a fragment parser consumes from `input`.
fn parse_prefix<T: Parse>(input: &[TokenTree]) -> Option<usize> {
    parse_prefix_with(input, |s| {
        let _: T = s.parse()?;
        Ok(())
    })
}

fn parse_prefix_with<F>(input: &[TokenTree], f: F) -> Option<usize>
where
    F: FnOnce(ParseStream) -> syn::Result<()>,
{
    let ts: TokenStream = input.iter().cloned().collect();
    let parser = |s: ParseStream| -> syn::Result<TokenStream> {
        f(s)?;
        let rest: TokenStream = s.parse()?;
        Ok(rest)
    };
    let rest = parser.parse2(ts).ok()?;
    let rest_len = rest.into_iter().count();
    Some(input.len() - rest_len)
}

/// Fragments parse greedily with syn, as rustc does.
fn match_frag(kind: &str, input: &[TokenTree]) -> Option<usize> {
    if input.is_empty() {
        return if kind == "vis" { Some(0) } else { None };
    }
    match kind {
        "tt" => Some(1),
        "ident" => match &input[0] {
            TokenTree::Ident(_) => Some(1),
            _ => None,
        },
        "literal" => match &input[0] {
            TokenTree::Literal(_) => Some(1),
            TokenTree::Punct(p) if p.as_char() == '-' => match input.get(1) {
                Some(TokenTree::Literal(_)) => Some(2),
                _ => None,
            },
            _ => None,
        },
        "lifetime" => match (&input[0], input.get(1)) {
            (TokenTree::Punct(p), Some(TokenTree::Ident(_))) if p.as_char() == '\'' => Some(2),
            _ => None,
        },
        "block" => match &input[0] {
            TokenTree::Group(g) if g.delimiter() == Delimiter::Brace => Some(1),
            _ => None,
        },
        "vis" => parse_prefix::<syn::Visibility>(input).or(Some(0)),
        "ty" => parse_prefix::<syn::Type>(input),
        "expr" | "expr_2021" => parse_prefix::<syn::Expr>(input),
        "path" => parse_prefix::<syn::Path>(input),
        "item" => parse_prefix::<syn::Item>(input),
        "meta" => parse_prefix::<syn::Meta>(input),
        "pat" | "pat_param" => parse_prefix_with(input, |s| {
            let _ = syn::Pat::parse_multi_with_leading_vert(s)?;
            Ok(())
        }),
        "stmt" => parse_prefix_with(input, |s| {
            let _ = syn::Block::parse_within(s)?;
            Ok(())
        })
        .filter(|n| *n > 0),
        _ => None,
    }
}

fn token_eq(node: &str, t: &TokenTree) -> bool {
    node == t.to_string()
}

/// Matches `nodes` against `input` starting at `pos`; returns the position
/// after the match. A repetition tries the longest run first and backs off
/// until the rest of the pattern matches too.
fn match_nodes(nodes: &[MatchNode], input: &[TokenTree], pos: usize, binds: &mut Binds) -> Option<usize> {
    let mut pos = pos;
    for (ni, node) in nodes.iter().enumerate() {
        match node {
            MatchNode::Token(s) => {
                let t = input.get(pos)?;
                if !token_eq(s, t) {
                    return None;
                }
                pos += 1;
            }
            MatchNode::Group { delim, inner } => {
                let TokenTree::Group(g) = input.get(pos)? else { return None };
                if g.delimiter() != *delim {
                    return None;
                }
                let inner_input = trees(&g.stream());
                let end = match_nodes(inner, &inner_input, 0, binds)?;
                if end != inner_input.len() {
                    return None;
                }
                pos += 1;
            }
            MatchNode::Frag { name, kind } => {
                let n = match_frag(kind, &input[pos..])?;
                let ts: TokenStream = input[pos..pos + n].iter().cloned().collect();
                binds.insert(name.clone(), Bind::Leaf(ts));
                pos += n;
            }
            MatchNode::Rep { inner, sep, op } => {
                // greedy run of iterations
                let mut iterations: Vec<(Binds, usize)> = Vec::new(); // (bindings, end)
                let mut cur = pos;
                loop {
                    if *op == '?' && iterations.len() == 1 {
                        break;
                    }
                    let start = if iterations.is_empty() {
                        cur
                    } else {
                        match sep {
                            Some(s) => match input.get(cur) {
                                Some(t) if token_eq(s, t) => cur + 1,
                                _ => break,
                            },
                            None => cur,
                        }
                    };
                    let mut it_binds = Binds::new();
                    match match_nodes(inner, input, start, &mut it_binds) {
                        Some(end) if end > start => {
                            iterations.push((it_binds, end));
                            cur = end;
                        }
                        _ => break,
                    }
                }
                let tail = &nodes[ni + 1..];
                let mut count = iterations.len();
                loop {
                    if *op == '+' && count == 0 {
                        return None;
                    }
                    let end_here = if count == 0 { pos } else { iterations[count - 1].1 };
                    let mut tail_binds = binds.clone();
                    if let Some(end) = match_nodes(tail, input, end_here, &mut tail_binds) {
                        let mut names: Vec<String> = Vec::new();
                        collect_names(inner, &mut names);
                        for name in names {
                            let seq: Vec<Bind> = iterations[..count].iter().map(|(b, _)| b.get(&name).cloned().unwrap_or(Bind::Seq(Vec::new()))).collect();
                            tail_binds.insert(name, Bind::Seq(seq));
                        }
                        *binds = tail_binds;
                        return Some(end);
                    }
                    if count == 0 {
                        return None;
                    }
                    count -= 1;
                }
            }
        }
    }
    Some(pos)
}

fn collect_names(nodes: &[MatchNode], out: &mut Vec<String>) {
    for n in nodes {
        match n {
            MatchNode::Frag { name, .. } => out.push(name.clone()),
            MatchNode::Group { inner, .. } | MatchNode::Rep { inner, .. } => collect_names(inner, out),
            MatchNode::Token(_) => {}
        }
    }
}

// ---------------------------------------------------------------------------
// transcription
// ---------------------------------------------------------------------------

fn transcribe(nodes: &[TransNode], binds: &Binds, crate_ident: &str, site: Span, out: &mut TokenStream) -> Result<(), String> {
    for n in nodes {
        match n {
            // Transcribed tokens are located at the invocation
            TransNode::Token(t) => {
                let mut t = t.clone();
                t.set_span(site);
                out.extend(std::iter::once(t));
            }
            TransNode::Group { delim, inner } => {
                let mut s = TokenStream::new();
                transcribe(inner, binds, crate_ident, site, &mut s)?;
                let mut g = Group::new(*delim, s);
                g.set_span(site);
                out.extend(std::iter::once(TokenTree::Group(g)));
            }
            TransNode::Var(name) => match binds.get(name) {
                Some(Bind::Leaf(ts)) => out.extend(ts.clone()),
                Some(Bind::Seq(_)) => return Err(format!("`${name}` used outside its repetition")),
                None => {
                    out.extend(quote!($));
                    out.extend(std::iter::once(TokenTree::Ident(Ident::new(name, Span::call_site()))));
                }
            },
            TransNode::Crate => out.extend(std::iter::once(TokenTree::Ident(Ident::new(crate_ident, site)))),
            TransNode::Rep { inner, sep } => {
                let mut names = Vec::new();
                collect_trans_names(inner, &mut names);
                let count = names
                    .iter()
                    .filter_map(|n| match binds.get(n) {
                        Some(Bind::Seq(v)) => Some(v.len()),
                        _ => None,
                    })
                    .max()
                    .unwrap_or(0);
                for i in 0..count {
                    if i > 0 {
                        if let Some(s) = sep {
                            out.extend(std::iter::once(s.clone()));
                        }
                    }
                    let mut view = binds.clone();
                    for n in &names {
                        if let Some(Bind::Seq(v)) = binds.get(n) {
                            if let Some(b) = v.get(i) {
                                view.insert(n.clone(), b.clone());
                            }
                        }
                    }
                    transcribe(inner, &view, crate_ident, site, out)?;
                }
            }
        }
    }
    Ok(())
}

fn collect_trans_names(nodes: &[TransNode], out: &mut Vec<String>) {
    for n in nodes {
        match n {
            TransNode::Var(name) => out.push(name.clone()),
            TransNode::Group { inner, .. } | TransNode::Rep { inner, .. } => collect_trans_names(inner, out),
            _ => {}
        }
    }
}

impl MacroDef {
    /// The first rule that matches, transcribed.
    fn expand(&self, input: &TokenStream, crate_ident: &str, site: Span) -> Result<TokenStream, String> {
        let input = trees(input);
        for rule in &self.rules {
            let mut binds = Binds::new();
            if let Some(end) = match_nodes(&rule.matcher, &input, 0, &mut binds) {
                if end == input.len() {
                    let mut out = TokenStream::new();
                    transcribe(&rule.transcriber, &binds, crate_ident, site, &mut out)?;
                    return Ok(out);
                }
            }
        }
        Err("no rule matched".into())
    }
}

// ---------------------------------------------------------------------------
// cfg_if (built in)
// ---------------------------------------------------------------------------

/// `if #[cfg(A)] { items } else if #[cfg(B)] { items } else { items }` →
/// the items with `#[cfg(A)]`, `#[cfg(all(not(A), B))]`, `#[cfg(all(not(A), not(B)))]`.
fn expand_cfg_if(ts: &TokenStream, site: Span) -> Option<TokenStream> {
    let t = trees(ts);
    let mut i = 0;
    let mut negated: Vec<TokenStream> = Vec::new();
    let mut out = TokenStream::new();
    let mut any = false;
    while i < t.len() {
        let is_if = matches!(&t[i], TokenTree::Ident(id) if id == "if");
        if is_if {
            let hash = matches!(t.get(i + 1), Some(TokenTree::Punct(p)) if p.as_char() == '#');
            let Some(TokenTree::Group(attr)) = t.get(i + 2) else { return None };
            let Some(TokenTree::Group(body)) = t.get(i + 3) else { return None };
            if !hash || attr.delimiter() != Delimiter::Bracket || body.delimiter() != Delimiter::Brace {
                return None;
            }
            let at = trees(&attr.stream());
            let pred = match (at.first(), at.get(1)) {
                (Some(TokenTree::Ident(c)), Some(TokenTree::Group(g))) if c == "cfg" => g.stream(),
                _ => return None,
            };
            let cond = conj(&negated, Some(pred.clone()));
            let items = body.stream();
            out.extend(quote::quote_spanned!(site => #[cfg(#cond)] klepto_cfg_if_branch! { #items }));
            negated.push(pred);
            any = true;
            i += 4;
            if matches!(t.get(i), Some(TokenTree::Ident(id)) if id == "else") {
                i += 1;
            }
            continue;
        }
        if let TokenTree::Group(body) = &t[i] {
            let cond = conj(&negated, None);
            let items = body.stream();
            out.extend(quote::quote_spanned!(site => #[cfg(#cond)] klepto_cfg_if_branch! { #items }));
            i += 1;
            continue;
        }
        return None;
    }
    if any { Some(out) } else { None }
}

/// `all(not(n1), …, last)`; with `last` = None: `all(not(n1), …)`.
fn conj(negated: &[TokenStream], last: Option<TokenStream>) -> TokenStream {
    let mut parts: Vec<TokenStream> = negated.iter().map(|n| quote!(not(#n))).collect();
    if let Some(l) = last {
        parts.push(l);
    }
    match parts.len() {
        0 => quote!(all()),
        1 => parts.pop().unwrap_or_default(),
        _ => quote!(all(#(#parts),*)),
    }
}

// ---------------------------------------------------------------------------
// the expander
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Outcome {
    /// Expanded into this many items / statements / one expression.
    Expanded { produced: usize },
    /// A definition exists but no rule matched the input.
    Unmatched,
    /// The expansion did not parse in this position.
    ParseError(String),
    /// A procedural macro, or a macro whose definition is not on disk.
    ProcMacroOrUnknown,
    /// Recursion past the depth limit.
    DepthExceeded,
    /// A std / core macro (`println!`, `vec!`, …): not expanded on purpose.
    Builtin,
}

/// One macro invocation klepto met while expanding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Expansion {
    pub macro_path: String,
    pub location: FileLocation,
    pub depth: usize,
    pub outcome: Outcome,
}

/// std / core macros that expand to nothing structural: left alone.
const BUILTIN_MACROS: &[&str] = &[
    "println", "print", "eprintln", "eprint", "format", "format_args", "write", "writeln", "panic", "assert", "assert_eq",
    "assert_ne", "debug_assert", "debug_assert_eq", "debug_assert_ne", "todo", "unimplemented", "unreachable", "vec",
    "matches", "dbg", "include_str", "include_bytes", "include", "concat", "stringify", "env", "option_env", "cfg", "line",
    "file", "column", "module_path", "compile_error", "thread_local", "macro_rules", "global_asm", "asm", "trace_macros",
    "log_syntax", "cfg_attr", "concat_idents", "ready", "pin", "assert_matches", "debug_assert_matches", "const_format_args",
];

pub(crate) struct Expander<'t> {
    table: &'t MacroTable,
    package: String,
    deps: Vec<String>,
    file: std::path::PathBuf,
    depth: usize,
    /// Inline modules being visited, outermost first.
    mod_chain: Vec<String>,
    pub expansions: Vec<Expansion>,
}

/// rustc's default `recursion_limit`.
const MAX_DEPTH: usize = 128;

impl<'t> Expander<'t> {
    pub(crate) fn new(table: &'t MacroTable, package: &str, deps: Vec<String>, file: &std::path::Path) -> Self {
        Self { table, package: package.to_string(), deps, file: file.to_path_buf(), depth: 0, mod_chain: Vec::new(), expansions: Vec::new() }
    }

    fn loc(&self, span: Span) -> FileLocation {
        crate::syntax::extract::span_to_location(&self.file, span)
    }

    fn record(&mut self, mac: &syn::Macro, outcome: Outcome) {
        if mac.path.is_ident("klepto_cfg_if_branch") {
            return; // an internal wrapper, not an invocation the source has
        }
        let macro_path = mac.path.segments.iter().map(|s| s.ident.to_string()).collect::<Vec<_>>().join("::");
        self.expansions.push(Expansion { macro_path, location: self.loc(mac.path.segments.last().map(|s| s.ident.span()).unwrap_or_else(Span::call_site)), depth: self.depth, outcome });
    }

    /// The raw expansion of one invocation, or `None` (recorded) when it is
    /// not expandable.
    fn expand_tokens(&mut self, mac: &syn::Macro) -> Option<(TokenStream, Vec<syn::Attribute>)> {
        let name = mac.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default();
        if self.depth >= MAX_DEPTH {
            self.record(mac, Outcome::DepthExceeded);
            return None;
        }
        if name == "cfg_if" {
            return match expand_cfg_if(&mac.tokens, site_of(mac)) {
                Some(ts) => Some((ts, Vec::new())),
                None => {
                    self.record(mac, Outcome::ParseError("cfg_if shape".into()));
                    None
                }
            };
        }
        if name == "klepto_cfg_if_branch" {
            return Some((mac.tokens.clone(), Vec::new()));
        }
        let Some(def) = self.table.find(&mac.path, &self.package, &self.deps, &self.file, &self.mod_chain) else {
            if BUILTIN_MACROS.contains(&name.as_str()) {
                self.record(mac, Outcome::Builtin);
            } else {
                self.record(mac, Outcome::ProcMacroOrUnknown);
            }
            return None;
        };
        let crate_ident = if def.crate_name == self.package { "crate".to_string() } else { def.crate_name.clone() };
        let site = mac.path.segments.last().map(|s| s.ident.span()).unwrap_or_else(Span::call_site);
        let cfgs: Vec<syn::Attribute> = def.cfg_attrs.iter().map(|a| respan_cfg(a, site)).collect();
        match def.expand(&mac.tokens, &crate_ident, site) {
            Ok(ts) => Some((ts, cfgs)),
            Err(e) if e == "no rule matched" => {
                self.record(mac, Outcome::Unmatched);
                None
            }
            Err(e) => {
                self.record(mac, Outcome::ParseError(e));
                None
            }
        }
    }

    /// Items produced by an item-position invocation, marked and
    /// carrying the invocation's attributes.
    fn expand_items(&mut self, m: &syn::ItemMacro) -> Option<Vec<syn::Item>> {
        let (ts, cfgs) = self.expand_tokens(&m.mac)?;
        match syn::parse2::<syn::File>(ts) {
            Ok(file) => {
                let mark = marker(&m.mac);
                let (inherited, mut rest) = split_invoked(&m.attrs);
                rest.extend(cfgs.iter().cloned());
                let mut items = file.items;
                for it in items.iter_mut() {
                    prepend_attrs(it, &rest, &mark);
                }
                if let Some(first) = items.first_mut() {
                    for inv in invoked_marker(&m.mac).into_iter().chain(inherited.into_iter().rev()) {
                        prepend_attrs(first, &[], &inv);
                    }
                }
                self.record(&m.mac, Outcome::Expanded { produced: items.len() });
                Some(items)
            }
            Err(e) => {
                self.record(&m.mac, Outcome::ParseError(e.to_string()));
                None
            }
        }
    }

    fn expand_impl_items(&mut self, m: &syn::ImplItemMacro) -> Option<Vec<syn::ImplItem>> {
        let (ts, cfgs) = self.expand_tokens(&m.mac)?;
        match syn::parse2::<syn::ItemImpl>(quote!(impl KleptoExpansion { #ts })) {
            Ok(imp) => {
                let mark = marker(&m.mac);
                let (inherited, mut rest) = split_invoked(&m.attrs);
                rest.extend(cfgs.iter().cloned());
                let mut items = imp.items;
                for (n, it) in items.iter_mut().enumerate() {
                    let attrs = match it {
                        syn::ImplItem::Fn(f) => &mut f.attrs,
                        syn::ImplItem::Const(c) => &mut c.attrs,
                        syn::ImplItem::Type(t) => &mut t.attrs,
                        syn::ImplItem::Macro(x) => &mut x.attrs,
                        _ => continue,
                    };
                    attrs.splice(0..0, rest.iter().cloned().chain(std::iter::once(mark.clone())));
                    if n == 0 {
                        for inv in invoked_marker(&m.mac).into_iter().chain(inherited.iter().rev().cloned()) {
                            attrs.insert(0, inv);
                        }
                    }
                }
                self.record(&m.mac, Outcome::Expanded { produced: items.len() });
                Some(items)
            }
            Err(e) => {
                self.record(&m.mac, Outcome::ParseError(e.to_string()));
                None
            }
        }
    }

    fn expand_trait_items(&mut self, m: &syn::TraitItemMacro) -> Option<Vec<syn::TraitItem>> {
        let (ts, cfgs) = self.expand_tokens(&m.mac)?;
        match syn::parse2::<syn::ItemTrait>(quote!(trait KleptoExpansion { #ts })) {
            Ok(tr) => {
                let mark = marker(&m.mac);
                let (inherited, mut rest) = split_invoked(&m.attrs);
                rest.extend(cfgs.iter().cloned());
                let mut items = tr.items;
                for (n, it) in items.iter_mut().enumerate() {
                    let attrs = match it {
                        syn::TraitItem::Fn(f) => &mut f.attrs,
                        syn::TraitItem::Const(c) => &mut c.attrs,
                        syn::TraitItem::Type(t) => &mut t.attrs,
                        syn::TraitItem::Macro(x) => &mut x.attrs,
                        _ => continue,
                    };
                    attrs.splice(0..0, rest.iter().cloned().chain(std::iter::once(mark.clone())));
                    if n == 0 {
                        for inv in invoked_marker(&m.mac).into_iter().chain(inherited.iter().rev().cloned()) {
                            attrs.insert(0, inv);
                        }
                    }
                }
                self.record(&m.mac, Outcome::Expanded { produced: items.len() });
                Some(items)
            }
            Err(e) => {
                self.record(&m.mac, Outcome::ParseError(e.to_string()));
                None
            }
        }
    }

    /// Statements from a statement-position invocation; the invocation's own
    /// attributes (`#[cfg(feature = "env")] ok!(..)` in clap) go on each.
    fn expand_stmts(&mut self, mac: &syn::Macro, inv_attrs: &[syn::Attribute]) -> Option<Vec<syn::Stmt>> {
        let (ts, mut cfgs) = self.expand_tokens(mac)?;
        cfgs.splice(0..0, inv_attrs.iter().filter(|a| !is_marker(a)).cloned());
        match syn::Block::parse_within.parse2(ts.clone()) {
            Ok(mut stmts) => {
                let produced = stmts.len();
                if stmts.is_empty() {
                    // An empty expansion still leaves its occurrence behind
                    stmts.push(syn::Stmt::Expr(syn::Expr::Tuple(syn::ExprTuple { attrs: Vec::new(), paren_token: Default::default(), elems: Default::default() }), Some(Default::default())));
                }
                mark_stmts(&mut stmts, mac, &cfgs);
                self.record(mac, Outcome::Expanded { produced });
                Some(stmts)
            }
            Err(_) => match syn::parse2::<syn::Expr>(ts) {
                Ok(e) => {
                    let mut stmts = vec![syn::Stmt::Expr(e, None)];
                    mark_stmts(&mut stmts, mac, &cfgs);
                    self.record(mac, Outcome::Expanded { produced: 1 });
                    Some(stmts)
                }
                Err(e) => {
                    self.record(mac, Outcome::ParseError(e.to_string()));
                    None
                }
            },
        }
    }

    fn expand_expr(&mut self, mac: &syn::Macro, inv_attrs: &[syn::Attribute]) -> Option<syn::Expr> {
        let (ts, mut cfgs) = self.expand_tokens(mac)?;
        cfgs.splice(0..0, inv_attrs.iter().filter(|a| !is_marker(a)).cloned());
        match syn::parse2::<syn::Expr>(ts.clone()) {
            Ok(mut e) => {
                mark_expr(&mut e, mac, &cfgs);
                self.record(mac, Outcome::Expanded { produced: 1 });
                Some(e)
            }
            Err(_) => match syn::Block::parse_within.parse2(ts) {
                Ok(mut stmts) => {
                    mark_stmts(&mut stmts, mac, &cfgs);
                    self.record(mac, Outcome::Expanded { produced: stmts.len() });
                    Some(syn::Expr::Block(syn::ExprBlock {
                        attrs: Vec::new(),
                        label: None,
                        block: syn::Block { brace_token: Default::default(), stmts },
                    }))
                }
                Err(e) => {
                    self.record(mac, Outcome::ParseError(e.to_string()));
                    None
                }
            },
        }
    }

    fn expand_item_list(&mut self, items: &mut Vec<syn::Item>) {
        let mut i = 0;
        while i < items.len() {
            let replacement = match &items[i] {
                syn::Item::Macro(m) if !m.mac.path.is_ident("macro_rules") => self.expand_items(m),
                _ => None,
            };
            match replacement {
                Some(new_items) => {
                    let mut slice = new_items;
                    self.depth += 1;
                    self.expand_item_list(&mut slice);
                    self.depth -= 1;
                    let n = slice.len();
                    items.splice(i..i + 1, slice);
                    i += n;
                }
                None => i += 1,
            }
        }
    }
}

fn macro_name(mac: &syn::Macro) -> String {
    if mac.path.is_ident("klepto_cfg_if_branch") {
        "cfg_if".to_string()
    } else {
        mac.path.segments.iter().map(|s| s.ident.to_string()).collect::<Vec<_>>().join("::")
    }
}

fn site_of(mac: &syn::Macro) -> Span {
    mac.path.segments.last().map(|s| s.ident.span()).unwrap_or_else(Span::call_site)
}

/// `#[doc = "<text>"]` with every token spanned at `site`, so an item whose
/// first token is the marker is still located at the invocation.
fn doc_attr_at(text: &str, site: Span) -> syn::Attribute {
    let lit = syn::LitStr::new(text, site);
    let tokens = quote::quote_spanned!(site => #[doc = #lit]);
    let attrs = syn::Attribute::parse_outer.parse2(tokens).unwrap_or_default();
    attrs.into_iter().next().unwrap_or_else(|| syn::parse_quote!(#[doc = #lit]))
}

/// `#[doc = "klepto:generated:name!"]`, spanned at the invocation.
fn marker(mac: &syn::Macro) -> syn::Attribute {
    doc_attr_at(&format!("{GENERATED_MARK}{}!", macro_name(mac)), site_of(mac))
}

/// `#[doc = "klepto:invoked:name!"]` for the first produced node; not for the
/// internal cfg_if wrapper (the `cfg_if!` invocation itself is recorded).
fn invoked_marker(mac: &syn::Macro) -> Option<syn::Attribute> {
    if mac.path.is_ident("klepto_cfg_if_branch") {
        return None;
    }
    Some(doc_attr_at(&format!("{INVOKED_MARK}{}!", macro_name(mac)), site_of(mac)))
}

fn prepend_attrs(item: &mut syn::Item, invocation_attrs: &[syn::Attribute], mark: &syn::Attribute) {
    let attrs: Vec<syn::Attribute> = invocation_attrs
        .iter()
        .filter(|a| !a.path().is_ident("macro_use") && !a.path().is_ident("macro_export"))
        .cloned()
        .chain(std::iter::once(mark.clone()))
        .collect();
    let target: Option<&mut Vec<syn::Attribute>> = match item {
        syn::Item::Const(x) => Some(&mut x.attrs),
        syn::Item::Enum(x) => Some(&mut x.attrs),
        syn::Item::ExternCrate(x) => Some(&mut x.attrs),
        syn::Item::Fn(x) => Some(&mut x.attrs),
        syn::Item::ForeignMod(x) => Some(&mut x.attrs),
        syn::Item::Impl(x) => Some(&mut x.attrs),
        syn::Item::Macro(x) => Some(&mut x.attrs),
        syn::Item::Mod(x) => Some(&mut x.attrs),
        syn::Item::Static(x) => Some(&mut x.attrs),
        syn::Item::Struct(x) => Some(&mut x.attrs),
        syn::Item::Trait(x) => Some(&mut x.attrs),
        syn::Item::TraitAlias(x) => Some(&mut x.attrs),
        syn::Item::Type(x) => Some(&mut x.attrs),
        syn::Item::Union(x) => Some(&mut x.attrs),
        syn::Item::Use(x) => Some(&mut x.attrs),
        _ => None,
    };
    if let Some(t) = target {
        t.splice(0..0, attrs);
    }
}

impl<'t> VisitMut for Expander<'t> {
    fn visit_file_mut(&mut self, f: &mut syn::File) {
        self.expand_item_list(&mut f.items);
        syn::visit_mut::visit_file_mut(self, f);
    }

    fn visit_item_mod_mut(&mut self, m: &mut syn::ItemMod) {
        self.mod_chain.push(m.ident.to_string());
        if let Some((_, items)) = &mut m.content {
            self.expand_item_list(items);
        }
        syn::visit_mut::visit_item_mod_mut(self, m);
        self.mod_chain.pop();
    }

    fn visit_item_impl_mut(&mut self, imp: &mut syn::ItemImpl) {
        let mut i = 0;
        while i < imp.items.len() {
            let replacement = match &imp.items[i] {
                syn::ImplItem::Macro(m) => self.expand_impl_items(m),
                _ => None,
            };
            match replacement {
                Some(new) => {
                    let n = new.len();
                    imp.items.splice(i..i + 1, new);
                    i += n;
                }
                None => i += 1,
            }
        }
        syn::visit_mut::visit_item_impl_mut(self, imp);
    }

    fn visit_item_trait_mut(&mut self, tr: &mut syn::ItemTrait) {
        let mut i = 0;
        while i < tr.items.len() {
            let replacement = match &tr.items[i] {
                syn::TraitItem::Macro(m) => self.expand_trait_items(m),
                _ => None,
            };
            match replacement {
                Some(new) => {
                    let n = new.len();
                    tr.items.splice(i..i + 1, new);
                    i += n;
                }
                None => i += 1,
            }
        }
        syn::visit_mut::visit_item_trait_mut(self, tr);
    }

    fn visit_block_mut(&mut self, b: &mut syn::Block) {
        let mut replaced = false;
        let mut i = 0;
        while i < b.stmts.len() {
            let replacement = match &b.stmts[i] {
                syn::Stmt::Macro(sm) => self.expand_stmts(&sm.mac, &sm.attrs),
                // an item-position invocation inside a body
                syn::Stmt::Item(syn::Item::Macro(m)) if !m.mac.path.is_ident("macro_rules") => {
                    self.expand_items(m).map(|items| items.into_iter().map(syn::Stmt::Item).collect())
                }
                syn::Stmt::Expr(syn::Expr::Macro(em), semi) => {
                    let semi = *semi;
                    self.expand_stmts(&em.mac, &em.attrs).map(|mut stmts| {
                        if let Some(syn::Stmt::Expr(_, s)) = stmts.last_mut() {
                            if semi.is_some() {
                                *s = semi;
                            }
                        }
                        stmts
                    })
                }
                _ => None,
            };
            match replacement {
                Some(new) => {
                    replaced = true;
                    let n = new.len();
                    b.stmts.splice(i..i + 1, new);
                    i += n;
                }
                None => i += 1,
            }
        }
        // produced statements are one level deeper (recursive macros end at MAX_DEPTH)
        if replaced {
            self.depth += 1;
        }
        syn::visit_mut::visit_block_mut(self, b);
        if replaced {
            self.depth -= 1;
        }
    }

    fn visit_expr_mut(&mut self, e: &mut syn::Expr) {
        let mut replaced = false;
        if let syn::Expr::Macro(em) = e {
            let mac = em.mac.clone();
            let attrs = em.attrs.clone();
            if let Some(new) = self.expand_expr(&mac, &attrs) {
                *e = new;
                replaced = true;
            }
        }
        if replaced {
            self.depth += 1;
        }
        syn::visit_mut::visit_expr_mut(self, e);
        if replaced {
            self.depth -= 1;
        }
    }

    fn visit_item_macro_mut(&mut self, _m: &mut syn::ItemMacro) {
        // handled by the item lists; nothing inside an invocation is visited
    }
}

/// Expands every expandable invocation in `ast` in place; returns
/// the expansion records.
pub(crate) fn expand_file(ast: &mut syn::File, table: &MacroTable, package: &str, deps: Vec<String>, file: &std::path::Path) -> Vec<Expansion> {
    let mut ex = Expander::new(table, package, deps, file);
    ex.visit_file_mut(ast);
    ex.expansions
}

/// Whether an item's attributes carry the generated marker.
pub fn generated_by(attrs: &[syn::Attribute]) -> Option<String> {
    attrs.iter().find_map(|a| {
        if !a.path().is_ident("doc") {
            return None;
        }
        let syn::Meta::NameValue(nv) = &a.meta else { return None };
        let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) = &nv.value else { return None };
        s.value().strip_prefix(GENERATED_MARK).map(str::to_string)
    })
}

/// Whether an attribute is one of klepto's own markers (never documentation).
pub fn is_marker(a: &syn::Attribute) -> bool {
    marker_text(a).is_some()
}

fn marker_text(a: &syn::Attribute) -> Option<(String, Span)> {
    if !a.path().is_ident("doc") {
        return None;
    }
    let syn::Meta::NameValue(nv) = &a.meta else { return None };
    let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) = &nv.value else { return None };
    let v = s.value();
    if v.starts_with("klepto:") { Some((v, s.span())) } else { None }
}

/// The invocation a node was the first product of: macro path and
/// the invocation's span.
pub fn invoked_by(attrs: &[syn::Attribute]) -> Option<(String, Span)> {
    attrs.iter().find_map(|a| {
        let (v, span) = marker_text(a)?;
        v.strip_prefix(INVOKED_MARK).map(|m| (m.trim_end_matches('!').to_string(), span))
    })
}

/// The attribute list of an expression, for the markers.
pub fn expr_attrs_mut(e: &mut syn::Expr) -> Option<&mut Vec<syn::Attribute>> {
    use syn::Expr::*;
    Some(match e {
        Array(x) => &mut x.attrs,
        Assign(x) => &mut x.attrs,
        Async(x) => &mut x.attrs,
        Await(x) => &mut x.attrs,
        Binary(x) => &mut x.attrs,
        Block(x) => &mut x.attrs,
        Break(x) => &mut x.attrs,
        Call(x) => &mut x.attrs,
        Cast(x) => &mut x.attrs,
        Closure(x) => &mut x.attrs,
        Const(x) => &mut x.attrs,
        Continue(x) => &mut x.attrs,
        Field(x) => &mut x.attrs,
        ForLoop(x) => &mut x.attrs,
        Group(x) => &mut x.attrs,
        If(x) => &mut x.attrs,
        Index(x) => &mut x.attrs,
        Infer(x) => &mut x.attrs,
        Let(x) => &mut x.attrs,
        Lit(x) => &mut x.attrs,
        Loop(x) => &mut x.attrs,
        Macro(x) => &mut x.attrs,
        Match(x) => &mut x.attrs,
        MethodCall(x) => &mut x.attrs,
        Paren(x) => &mut x.attrs,
        Path(x) => &mut x.attrs,
        Range(x) => &mut x.attrs,
        Reference(x) => &mut x.attrs,
        Repeat(x) => &mut x.attrs,
        Return(x) => &mut x.attrs,
        Struct(x) => &mut x.attrs,
        Try(x) => &mut x.attrs,
        TryBlock(x) => &mut x.attrs,
        Tuple(x) => &mut x.attrs,
        Unary(x) => &mut x.attrs,
        Unsafe(x) => &mut x.attrs,
        While(x) => &mut x.attrs,
        Yield(x) => &mut x.attrs,
        _ => return None,
    })
}

/// Read-only twin of [`expr_attrs_mut`].
pub fn expr_attrs(e: &syn::Expr) -> &[syn::Attribute] {
    use syn::Expr::*;
    match e {
        Array(x) => &x.attrs,
        Assign(x) => &x.attrs,
        Async(x) => &x.attrs,
        Await(x) => &x.attrs,
        Binary(x) => &x.attrs,
        Block(x) => &x.attrs,
        Break(x) => &x.attrs,
        Call(x) => &x.attrs,
        Cast(x) => &x.attrs,
        Closure(x) => &x.attrs,
        Const(x) => &x.attrs,
        Continue(x) => &x.attrs,
        Field(x) => &x.attrs,
        ForLoop(x) => &x.attrs,
        Group(x) => &x.attrs,
        If(x) => &x.attrs,
        Index(x) => &x.attrs,
        Infer(x) => &x.attrs,
        Let(x) => &x.attrs,
        Lit(x) => &x.attrs,
        Loop(x) => &x.attrs,
        Macro(x) => &x.attrs,
        Match(x) => &x.attrs,
        MethodCall(x) => &x.attrs,
        Paren(x) => &x.attrs,
        Path(x) => &x.attrs,
        Range(x) => &x.attrs,
        Reference(x) => &x.attrs,
        Repeat(x) => &x.attrs,
        Return(x) => &x.attrs,
        Struct(x) => &x.attrs,
        Try(x) => &x.attrs,
        TryBlock(x) => &x.attrs,
        Tuple(x) => &x.attrs,
        Unary(x) => &x.attrs,
        Unsafe(x) => &x.attrs,
        While(x) => &x.attrs,
        Yield(x) => &x.attrs,
        _ => &[],
    }
}

fn stmt_attrs_mut(s: &mut syn::Stmt) -> Option<&mut Vec<syn::Attribute>> {
    match s {
        syn::Stmt::Local(l) => Some(&mut l.attrs),
        syn::Stmt::Macro(m) => Some(&mut m.attrs),
        syn::Stmt::Expr(e, _) => expr_attrs_mut(e),
        syn::Stmt::Item(_) => None,
    }
}

/// Marks produced statements: every one generated, the first one invoked.
fn mark_stmts(stmts: &mut [syn::Stmt], mac: &syn::Macro, cfgs: &[syn::Attribute]) {
    let mark = marker(mac);
    for (n, s) in stmts.iter_mut().enumerate() {
        if let syn::Stmt::Item(it) = s {
            prepend_attrs(it, cfgs, &mark);
            if n == 0 {
                if let Some(inv) = invoked_marker(mac) {
                    prepend_attrs(it, &[], &inv);
                }
            }
            continue;
        }
        if let Some(attrs) = stmt_attrs_mut(s) {
            attrs.insert(0, mark.clone());
            attrs.splice(0..0, cfgs.iter().cloned());
            if n == 0 {
                if let Some(inv) = invoked_marker(mac) {
                    attrs.insert(0, inv);
                }
            }
        }
    }
}

fn mark_expr(e: &mut syn::Expr, mac: &syn::Macro, cfgs: &[syn::Attribute]) {
    let mark = marker(mac);
    let inv = invoked_marker(mac);
    if let Some(attrs) = expr_attrs_mut(e) {
        attrs.insert(0, mark);
        attrs.splice(0..0, cfgs.iter().cloned());
        if let Some(inv) = inv {
            attrs.insert(0, inv);
        }
    }
}

/// Splits an invocation's attributes into inherited `invoked` markers (they
/// belong to the first produced node only) and everything else.
fn split_invoked(attrs: &[syn::Attribute]) -> (Vec<syn::Attribute>, Vec<syn::Attribute>) {
    attrs.iter().cloned().partition(|a| invoked_by(std::slice::from_ref(a)).is_some())
}

/// A definition's `#[cfg(..)]`, re-spanned to the invocation.
fn respan_cfg(a: &syn::Attribute, site: Span) -> syn::Attribute {
    match &a.meta {
        syn::Meta::List(l) => {
            let inner = &l.tokens;
            let tokens = quote::quote_spanned!(site => #[cfg(#inner)]);
            syn::Attribute::parse_outer.parse2(tokens).ok().and_then(|v| v.into_iter().next()).unwrap_or_else(|| a.clone())
        }
        _ => a.clone(),
    }
}
