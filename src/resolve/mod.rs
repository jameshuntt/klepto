//! Module tree + alias resolution.
//!
//! Acceptance tests: `tests/resolve.rs`. Everything here is an
//! over-approximation on purpose: when a rule cannot decide it emits
//! [`Resolution::Unresolved`] with every candidate it knows, and never drops
//! an occurrence. There is no type checking and no trait resolution.

pub mod cfg;
pub mod expand;
pub mod features;

use crate::resolve::cfg::{cfg_of_attrs, CfgEnv, CfgPredicate, Implication};
use crate::syntax::extract::{fq_name, imports_from_item_use};
use crate::klepto::{Klepto, ParsedFile};
use crate::model::*;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// One lib/bin target root from cargo metadata.
#[derive(Debug, Clone)]
pub(crate) struct PackageRoot {
    pub package: String,
    pub target: String,
    pub kind: String,
    pub file: PathBuf,
}

pub(crate) fn norm(s: &str) -> String {
    s.replace('-', "_")
}

fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// `app`, `app::engine::parts`.
pub(crate) fn join_key(package: &str, segs: &[String]) -> String {
    if segs.is_empty() { package.to_string() } else { format!("{}::{}", package, segs.join("::")) }
}

fn join_path(base: &str, rest: &[String]) -> String {
    if rest.is_empty() { base.to_string() } else { format!("{}::{}", base, rest.join("::")) }
}

/// `(is_absolute, segments)` of a path string as the extractors record it.
fn split_path(s: &str) -> (bool, Vec<String>) {
    let abs = s.starts_with("::");
    let segs = s
        .trim_start_matches("::")
        .split("::")
        .filter(|x| !x.is_empty())
        .map(String::from)
        .collect();
    (abs, segs)
}

/// Package of every parsed file, from the module tree.
pub(crate) fn file_packages(k: &Klepto) -> HashMap<PathBuf, String> {
    let mut m = HashMap::new();
    for f in &k.files {
        if let Some(mi) = f.module.as_deref().and_then(|key| k.modules.get(key)) {
            m.insert(f.path.clone(), mi.package.clone());
        }
    }
    m
}

// ---------------------------------------------------------------------------
// M rules — module tree
// ---------------------------------------------------------------------------

struct ModDecl {
    name: String,
    is_pub: bool,
    path_attr: Option<String>,
    /// The declaration's own `#[cfg]`.
    cfg: CfgPredicate,
    inline: Option<Decls>,
    /// `#[macro_use]` on the declaration.
    macro_use: bool,
}

#[derive(Default)]
struct Decls {
    items: BTreeMap<String, ItemKind>,
    mods: Vec<ModDecl>,
}

fn path_attr(attrs: &[syn::Attribute]) -> Option<String> {
    attrs.iter().find_map(|a| {
        if !a.path().is_ident("path") {
            return None;
        }
        match &a.meta {
            syn::Meta::NameValue(nv) => match &nv.value {
                syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) => Some(s.value()),
                _ => None,
            },
            _ => None,
        }
    })
}

fn add_item(d: &mut Decls, name: String, kind: ItemKind) {
    // an item outranks a `use` of the same name (the `use` is the
    // other namespace's binding and is kept as the shadow)
    match d.items.get(&name) {
        Some(ItemKind::Use) if kind != ItemKind::Use => {
            d.items.insert(name, kind);
        }
        Some(_) => {}
        None => {
            d.items.insert(name, kind);
        }
    }
}

/// `[pub[(..)]] mod name ;` sequences anywhere in a macro
/// invocation's tokens (groups included, so `cfg_if!` branches count).
fn mods_in_tokens(ts: &proc_macro2::TokenStream) -> Vec<(String, bool)> {
    fn walk(ts: &proc_macro2::TokenStream, out: &mut Vec<(String, bool)>) {
        use proc_macro2::TokenTree;
        let trees: Vec<TokenTree> = ts.clone().into_iter().collect();
        for (i, t) in trees.iter().enumerate() {
            match t {
                TokenTree::Group(g) => walk(&g.stream(), out),
                TokenTree::Ident(id) if id == "mod" => {
                    let name = match trees.get(i + 1) {
                        Some(TokenTree::Ident(n)) => n.to_string(),
                        _ => continue,
                    };
                    let is_semi = matches!(trees.get(i + 2), Some(TokenTree::Punct(p)) if p.as_char() == ';');
                    if !is_semi {
                        continue;
                    }
                    // visibility: the ident before `mod`, skipping a `(crate)` group
                    let mut j = i;
                    let mut is_pub = false;
                    while j > 0 {
                        j -= 1;
                        match &trees[j] {
                            TokenTree::Group(_) => continue,
                            TokenTree::Ident(v) if v == "pub" => {
                                is_pub = true;
                                break;
                            }
                            _ => break,
                        }
                    }
                    out.push((name, is_pub));
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(ts, &mut out);
    out
}

/// Names declared directly in a module body, plus the `mod`
/// declarations to follow. Namespaces are ignored; first declaration wins.
/// `pred` with the conjunct `old` (a parent module's cfg) replaced by `new`.
fn replace_conjunct(pred: &CfgPredicate, old: &CfgPredicate, new: &CfgPredicate) -> CfgPredicate {
    if pred == old {
        return new.clone();
    }
    match pred {
        CfgPredicate::All(parts) => CfgPredicate::All(parts.iter().map(|p| replace_conjunct(p, old, new)).collect()).simplify(),
        other => other.clone(),
    }
}

fn decls_of(items: &[syn::Item]) -> Decls {
    let mut d = Decls::default();
    for it in items {
        match it {
            syn::Item::Fn(f) => add_item(&mut d, f.sig.ident.to_string(), ItemKind::Fn),
            syn::Item::Struct(s) => add_item(&mut d, s.ident.to_string(), ItemKind::Struct),
            syn::Item::Enum(e) => add_item(&mut d, e.ident.to_string(), ItemKind::Enum),
            syn::Item::Union(u) => add_item(&mut d, u.ident.to_string(), ItemKind::Union),
            syn::Item::Trait(t) => add_item(&mut d, t.ident.to_string(), ItemKind::Trait),
            syn::Item::Type(t) => add_item(&mut d, t.ident.to_string(), ItemKind::TypeAlias),
            syn::Item::Const(c) => add_item(&mut d, c.ident.to_string(), ItemKind::Const),
            syn::Item::Static(s) => add_item(&mut d, s.ident.to_string(), ItemKind::Static),
            syn::Item::Macro(m) => {
                if let Some(id) = &m.ident {
                    add_item(&mut d, id.to_string(), ItemKind::Macro);
                } else {
                    // `cfg_rt! { pub mod runtime; }`, `cfg_if!`,
                    // hyper's `cfg_feature!` — a `mod name;` inside a module-level
                    // macro invocation is still a declaration. Its cfg is Unknown
                    // (the macro decides), never a guess.
                    let macro_name = format!("{}!", m.mac.path.segments.iter().map(|s| s.ident.to_string()).collect::<Vec<_>>().join("::"));
                    for (name, is_pub) in mods_in_tokens(&m.mac.tokens) {
                        add_item(&mut d, name.clone(), ItemKind::Mod);
                        d.mods.push(ModDecl {
                            name,
                            is_pub,
                            path_attr: None,
                            cfg: CfgPredicate::Unknown(macro_name.clone()),
                            inline: None,
                            macro_use: false,
                        });
                    }
                }
            }
            syn::Item::Mod(m) => {
                add_item(&mut d, m.ident.to_string(), ItemKind::Mod);
                d.mods.push(ModDecl {
                    name: m.ident.to_string(),
                    is_pub: matches!(m.vis, syn::Visibility::Public(_)),
                    path_attr: path_attr(&m.attrs),
                    cfg: cfg_of_attrs(&m.attrs),
                    inline: m.content.as_ref().map(|(_, inner)| decls_of(inner)),
                    macro_use: m.attrs.iter().any(|a| a.path().is_ident("macro_use")),
                });
            }
            syn::Item::Use(u) => {
                let mut tmp = Vec::new();
                imports_from_item_use(Path::new(""), &[], None, &CfgPredicate::True, u, &mut tmp);
                for sp in tmp {
                    let name = match &sp.kind {
                        UseKind::Glob => continue,
                        UseKind::Rename { alias } => alias.clone(),
                        UseKind::Name => sp.segments.last().cloned().unwrap_or_else(|| sp.root.clone()),
                    };
                    add_item(&mut d, name, ItemKind::Use);
                }
            }
            _ => {}
        }
    }
    d
}

fn is_mod_rs(file: &Path) -> bool {
    file.file_name().is_some_and(|n| n == "mod.rs")
}

/// Directory that `mod x;` declarations inside `file` look in:
/// the file's own directory for roots, `mod.rs` and `#[path]` files; the
/// directory named after the file otherwise.
fn child_dir_of(file: &Path, mod_rs_style: bool) -> PathBuf {
    let parent = file.parent().map(Path::to_path_buf).unwrap_or_default();
    if mod_rs_style { parent } else { parent.join(file.file_stem().unwrap_or_default()) }
}

struct TreeBuilder<'a> {
    files: &'a mut [ParsedFile],
    decls: &'a [Decls],
    index: HashMap<PathBuf, usize>,
    tree: ModuleTree,
    assigned: HashSet<usize>,
}

impl<'a> TreeBuilder<'a> {
    #[allow(clippy::too_many_arguments)]
    fn declare(
        &mut self,
        package: &str,
        segs: Vec<String>,
        decls: &'a Decls,
        file_idx: Option<usize>,
        file: PathBuf,
        inline: bool,
        declared_in: Option<PathBuf>,
        is_pub: bool,
        macro_use: bool,
        orphan: bool,
        cfg: CfgPredicate,
        child_dir: PathBuf,
    ) {
        let key = join_key(package, &segs);
        self.tree.modules.insert(
            key.clone(),
            ModuleInfo {
                path: key.clone(),
                package: package.to_string(),
                segments: segs.clone(),
                file: file.clone(),
                inline,
                declared_in,
                is_pub,
                orphan,
                items: decls.items.clone(),
                alternates: Vec::new(),
                macro_use,
                cfg: cfg.clone(),
            },
        );
        if let Some(i) = file_idx {
            self.files[i].module = Some(key.clone());
            self.assigned.insert(i);
        }

        let decls_all: &'a [Decls] = self.decls;
        for m in &decls.mods {
            let mut child = segs.clone();
            child.push(m.name.clone());
            let child_key = join_key(package, &child);
            let child_cfg = cfg.clone().and(m.cfg.clone());
            if let Some(inner) = &m.inline {
                // an inline module (a second inline declaration of the same name is ignored)
                if self.tree.modules.contains_key(&child_key) {
                    continue;
                }
                self.declare(package, child, inner, None, file.clone(), true, Some(file.clone()), m.is_pub, m.macro_use, orphan, child_cfg, child_dir.join(&m.name));
                continue;
            }
            let candidates: Vec<PathBuf> = match &m.path_attr {
                Some(p) => vec![file.parent().map(|d| d.join(p)).unwrap_or_else(|| PathBuf::from(p))],
                None => vec![child_dir.join(format!("{}.rs", m.name)), child_dir.join(&m.name).join("mod.rs")],
            };
            let found = candidates.iter().find_map(|c| self.index.get(&canon(c)).copied());

            if let Some(existing) = self.tree.modules.get_mut(&child_key) {
                // A second `mod x;` for the same module (cfg alternates)
                let f = found.map(|ci| self.files[ci].path.clone()).unwrap_or_else(|| candidates[0].clone());
                if canon(&f) == canon(&existing.file) {
                    // The SAME file declared again under
                    // another cfg (`#[cfg(feature = "std")] pub mod ser;` +
                    // `#[cfg(not(feature = "std"))] mod ser;` in serde_json) is
                    // present under either: the module's cfg is the disjunction,
                    // `True` when the declarations cover every configuration.
                    let merged = CfgPredicate::Any(vec![existing.cfg.clone(), child_cfg.clone()]).simplify();
                    let old = existing.cfg.clone();
                    let new = if matches!(CfgPredicate::True.implies(&merged, None), crate::resolve::cfg::Implication::Always) {
                        CfgPredicate::True
                    } else {
                        // not provably covering: which declaration applies is undecidable
                        CfgPredicate::Unknown(format!("twins: {}", merged.render()))
                    };
                    existing.cfg = new.clone();
                    existing.is_pub |= m.is_pub;
                    // The descendants declared under the first
                    // twin inherited its cfg (tokio's `runtime::park` under `rt`);
                    // they are present exactly when the merged module is
                    let prefix = format!("{child_key}::");
                    let below: Vec<String> = self.tree.modules.keys().filter(|k| k.starts_with(&prefix)).cloned().collect();
                    for k in below {
                        if let Some(d) = self.tree.modules.get_mut(&k) {
                            d.cfg = replace_conjunct(&d.cfg, &old, &new);
                        }
                    }
                } else {
                    existing.alternates.push(f);
                }
                continue;
            }

            match found {
                Some(ci) => {
                    let cf = self.files[ci].path.clone();
                    let cdir = child_dir_of(&cf, m.path_attr.is_some() || is_mod_rs(&cf));
                    self.declare(package, child, &decls_all[ci], Some(ci), cf, false, Some(file.clone()), m.is_pub, m.macro_use, orphan, child_cfg, cdir);
                }
                None => {
                    // declared but no file was scanned for it: keep the module, never fail
                    self.tree.modules.insert(
                        child_key.clone(),
                        ModuleInfo {
                            path: child_key,
                            package: package.to_string(),
                            segments: child,
                            file: candidates[0].clone(),
                            inline: false,
                            declared_in: Some(file.clone()),
                            is_pub: m.is_pub,
                            orphan: true,
                            items: BTreeMap::new(),
                            alternates: Vec::new(),
                            macro_use: m.macro_use,
                            cfg: child_cfg,
                        },
                    );
                }
            }
        }
    }
}

/// The crate-root name of each target: a lib is its package; a bin is its
/// target's name, or `<package>_bin` when that is the package's and the
/// package also has a lib. Its items are named under it (`appd::main`).
pub(crate) fn root_names(roots: &[PackageRoot]) -> Vec<(String, String)> {
    let is_lib = |k: &str| !matches!(k, "bin" | "test" | "example" | "bench");
    let has_lib: HashSet<&str> = roots.iter().filter(|r| is_lib(&r.kind)).map(|r| r.package.as_str()).collect();
    roots
        .iter()
        .map(|r| {
            let name = match r.kind.as_str() {
                "bin" if norm(&r.target) == r.package => {
                    if has_lib.contains(r.package.as_str()) { format!("{}_bin", r.package) } else { r.package.clone() }
                }
                // a test, example or bench target is a crate of its own beside the package's
                "test" | "example" | "bench" if norm(&r.target) == r.package => format!("{}_{}", r.package, r.kind),
                "bin" | "test" | "example" | "bench" => norm(&r.target),
                _ => r.package.clone(),
            };
            (name, r.package.clone())
        })
        .collect()
}

/// `(package name, index of the root file)` per crate root.
fn root_entries(
    crate_name: &str,
    roots: &[PackageRoot],
    scan_roots: &[PathBuf],
    index: &HashMap<PathBuf, usize>,
) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    if !roots.is_empty() {
        for (r, (name, _)) in roots.iter().zip(root_names(roots)) {
            if let Some(&i) = index.get(&canon(&r.file)) {
                out.push((name, i));
            }
        }
        return out;
    }
    let pkg = norm(crate_name);
    for sr in scan_roots {
        if sr.is_file() {
            if let Some(&i) = index.get(&canon(sr)) {
                out.push((pkg.clone(), i));
            }
            continue;
        }
        for cand in ["lib.rs", "main.rs", "mod.rs"] {
            if let Some(&i) = index.get(&canon(&sr.join(cand))) {
                out.push((pkg.clone(), i));
                break;
            }
        }
    }
    out
}

/// Builds the module tree and fills `ParsedFile::module` on every file.
pub(crate) fn build_module_tree(
    crate_name: &str,
    roots: &[PackageRoot],
    scan_roots: &[PathBuf],
    files: &mut [ParsedFile],
) -> ModuleTree {
    let decls_all: Vec<Decls> = files.iter().map(|f| decls_of(&f.ast.items)).collect();
    let index: HashMap<PathBuf, usize> = files.iter().enumerate().map(|(i, f)| (canon(&f.path), i)).collect();
    let entries = root_entries(crate_name, roots, scan_roots, &index);

    let mut b = TreeBuilder { files, decls: &decls_all, index, tree: ModuleTree::default(), assigned: HashSet::new() };

    for (pkg, idx) in &entries {
        let file = b.files[*idx].path.clone();
        let dir = child_dir_of(&file, true);
        b.declare(pkg, Vec::new(), &decls_all[*idx], Some(*idx), file, false, None, true, false, false, CfgPredicate::True, dir);
    }

    // Orphans, attributed to the package whose root directory contains them
    let root_dirs: Vec<(PathBuf, String)> = entries
        .iter()
        .map(|(pkg, idx)| (canon(&b.files[*idx].path).parent().map(Path::to_path_buf).unwrap_or_default(), pkg.clone()))
        .collect();
    let mut order: Vec<usize> = (0..b.files.len()).collect();
    order.sort_by(|x, y| b.files[*x].path.cmp(&b.files[*y].path));
    for i in order {
        if b.assigned.contains(&i) {
            continue;
        }
        let file = b.files[i].path.clone();
        let cf = canon(&file);
        let pkg = root_dirs
            .iter()
            .filter(|(d, _)| cf.starts_with(d))
            .max_by_key(|(d, _)| d.components().count())
            .map(|(_, p)| p.clone())
            .unwrap_or_else(|| norm(crate_name));
        let stem = if is_mod_rs(&file) {
            file.parent().and_then(Path::file_name).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "mod".into())
        } else {
            file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
        };
        let mut segs = vec![stem.clone()];
        let mut n = 1;
        while b.tree.modules.contains_key(&join_key(&pkg, &segs)) {
            n += 1;
            segs = vec![format!("{stem}_{n}")];
        }
        let dir = child_dir_of(&file, is_mod_rs(&file));
        b.declare(&pkg, segs, &decls_all[i], Some(i), file, false, None, false, false, true, CfgPredicate::True, dir);
    }

    b.tree
}

// ---------------------------------------------------------------------------
// S / R rules — scope tables and resolution
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Bind {
    /// An item declared in the module: canonical path and kind.
    Item(String, ItemKind),
    /// A `use` binding: the target path as written, resolved on demand,
    /// with the import's effective cfg.
    Use { target: Vec<String>, is_absolute: bool, cfg: CfgPredicate },
    /// `extern crate name [as alias]`.
    Extern(String),
}

#[derive(Default, Debug)]
struct Scope {
    bindings: BTreeMap<String, Bind>,
    /// A `use` binding an item of the other namespace shadows
    /// (syn's `fn Ident` beside `use proc_macro2::Ident`).
    shadow: BTreeMap<String, Bind>,
    /// Every binding of a name with its cfg, in source order, for
    /// names bound more than once under different cfgs.
    twins: BTreeMap<String, Vec<(Bind, CfgPredicate)>>,
    /// Glob targets as written, source order.
    globs: Vec<(Vec<String>, bool)>,
}

pub(crate) struct Ctx<'c> {
    pub(crate) module: &'c str,
    pub(crate) package: &'c str,
    /// The namespace the path is written in.
    pub(crate) ns: Namespace,
    /// The effective cfg at the occurrence, when known.
    pub(crate) cfg: Option<&'c CfgPredicate>,
    /// The enclosing fn's fq_name — or, for type records, the item
    /// id registered with `Resolver::add_item_context`.
    pub(crate) enclosing_fn: Option<&'c str>,
}

/// Rust 2021 prelude names plus the std macros.
const PRELUDE: &[&str] = &[
    "Option", "Some", "None", "Result", "Ok", "Err", "Box", "String", "Vec", "ToString", "ToOwned",
    "Clone", "Copy", "Send", "Sync", "Sized", "Unpin", "Drop", "Fn", "FnMut", "FnOnce", "AsRef",
    "AsMut", "Into", "From", "TryInto", "TryFrom", "Iterator", "IntoIterator",
    "DoubleEndedIterator", "ExactSizeIterator", "Extend", "Default", "PartialEq", "Eq",
    "PartialOrd", "Ord", "Hash", "Debug", "drop",
    "println", "print", "eprintln", "eprint", "format", "vec", "panic", "assert", "assert_eq",
    "assert_ne", "debug_assert", "debug_assert_eq", "debug_assert_ne", "todo", "unimplemented",
    "unreachable", "write", "writeln", "matches", "dbg", "include_str", "include_bytes",
    "include", "concat", "stringify", "env", "option_env", "cfg", "line", "file", "column",
    "module_path", "compile_error", "format_args", "thread_local",
];

fn is_prelude(s: &str) -> bool {
    PRELUDE.contains(&s)
}

/// Attributes the compiler and std define; any other bare attribute name
/// (`#[serde(..)]`, `#[tokio::main]`'s root) names a crate.
const BUILTIN_ATTRS: &[&str] = &[
    "cfg", "cfg_attr", "allow", "warn", "deny", "forbid", "expect", "deprecated", "must_use", "doc",
    "test", "ignore", "should_panic", "bench", "inline", "cold", "derive", "path", "macro_use",
    "macro_export", "non_exhaustive", "repr", "no_std", "no_main", "no_implicit_prelude", "feature",
    "global_allocator", "panic_handler", "track_caller", "automatically_derived", "link",
    "link_name", "link_section", "no_mangle", "export_name", "used", "target_feature", "proc_macro",
    "proc_macro_derive", "proc_macro_attribute", "diagnostic", "rustfmt", "clippy", "lint",
    "crate_name", "crate_type", "recursion_limit", "type_length_limit", "windows_subsystem",
    "start", "no_builtins", "debugger_visualizer", "collapse_debuginfo", "instruction_set",
];

pub(crate) fn is_builtin_attr(s: &str) -> bool {
    BUILTIN_ATTRS.contains(&s)
}

fn unresolved(raw: &str, reason: UnresolvedReason) -> Resolution {
    Resolution::Unresolved { raw: raw.to_string(), reason, candidates: Vec::new() }
}

/// Continue a resolved prefix with the remaining segments.
/// Features a predicate asks for (not under a `not`), for twin coverage.
fn positive_features(p: &CfgPredicate) -> BTreeSet<String> {
    fn walk(p: &CfgPredicate, neg: bool, out: &mut BTreeSet<String>) {
        match p {
            CfgPredicate::Feature(f) => {
                if !neg {
                    out.insert(f.clone());
                }
            }
            CfgPredicate::All(ps) | CfgPredicate::Any(ps) => ps.iter().for_each(|q| walk(q, neg, out)),
            CfgPredicate::Not(q) => walk(q, !neg, out),
            _ => {}
        }
    }
    let mut out = BTreeSet::new();
    walk(p, false, &mut out);
    out
}

/// Does a binding answer a position of this namespace?
fn ns_fits(b: &Bind, ns: Namespace) -> bool {
    match ns {
        Namespace::Type => !matches!(b, Bind::Item(_, ItemKind::Fn | ItemKind::Const | ItemKind::Static)),
        Namespace::Value => !matches!(b, Bind::Item(_, ItemKind::Trait | ItemKind::Mod | ItemKind::TypeAlias | ItemKind::Union)),
        Namespace::Any => true,
    }
}

fn bind_use(scope: &mut Scope, name: String, b: Bind) {
    if let Bind::Use { cfg, .. } = &b {
        scope.twins.entry(name.clone()).or_default().push((b.clone(), cfg.clone()));
    }
    match scope.bindings.get(&name) {
        None => {
            scope.bindings.insert(name, b);
        }
        Some(Bind::Item(..)) => {
            scope.shadow.entry(name).or_insert(b);
        }
        Some(_) => {}
    }
}

fn append(r: Resolution, rest: &[String]) -> Resolution {
    match r {
        Resolution::Local(s) => Resolution::Local(join_path(&s, rest)),
        Resolution::External(s) => Resolution::External(join_path(&s, rest)),
        Resolution::Unresolved { raw, reason, candidates } => Resolution::Unresolved {
            raw: join_path(&raw, rest),
            reason,
            candidates: candidates.into_iter().map(|c| join_path(&c, rest)).collect(),
        },
    }
}

/// `Regex :: new`, `Vec :: < u8 > :: new` → segments; `None` for anything
/// that is not a plain path (closures, method calls, field calls).
fn callee_segments(callee: &str) -> Option<(Vec<String>, bool)> {
    let mut out = String::new();
    let mut depth = 0usize;
    for ch in callee.chars().filter(|c| !c.is_whitespace()) {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    if out.is_empty() || out.chars().any(|c| !(c.is_alphanumeric() || c == '_' || c == ':')) {
        return None;
    }
    let (abs, segs) = split_path(&out);
    if segs.is_empty() { None } else { Some((segs, abs)) }
}

pub(crate) struct Resolver {
    modules: ModuleTree,
    scopes: HashMap<String, Scope>,
    overlays: HashMap<String, Scope>,
    package_deps: BTreeMap<String, BTreeSet<String>>,
    deps_known: bool,
    fn_generics: HashMap<String, HashSet<String>>,
    fn_self_ty: HashMap<String, String>,
    /// Per binding: its resolution and whether resolving it went through
    /// covering cfg twins, replayed on every memo hit.
    memo: RefCell<HashMap<String, (Resolution, bool)>>,
    /// The default build per package, to choose among cfg twins.
    default_envs: HashMap<String, CfgEnv>,
    /// The feature models, to decide whether twins cover the
    /// builds the reference exists in (feature implications included).
    feature_models: BTreeMap<String, crate::resolve::features::FeatureModel>,
    /// Set by `pick` when the chosen twin's siblings cover every
    /// build; read (and cleared) by `resolve_all` per occurrence.
    covered: std::cell::Cell<bool>,
    /// What each glob import of a module names, resolved once per module
    /// (`None` while it is being resolved: a glob whose own path can only come
    /// through the module's globs is a cycle, and stays unresolved). Without
    /// it a module with two globs whose roots are unknown re-resolves each
    /// one through the other until `MAX_DEPTH`, 2^32 times.
    glob_targets: RefCell<HashMap<GlobKey, Option<Resolution>>>,
}

/// A glob import as `glob_targets` keys it: the module it is in, the path it
/// names, and whether that path is absolute.
type GlobKey = (String, Vec<String>, bool);

const MAX_DEPTH: u32 = 32;

impl Resolver {
    /// Resolves a path spelled `segs` (with `raw` kept on failure).
    fn resolve(&self, segs: &[String], is_absolute: bool, cx: &Ctx, depth: u32) -> Resolution {
        let raw = if is_absolute { format!("::{}", segs.join("::")) } else { segs.join("::") };
        let mut r = self.resolve_inner(segs, is_absolute, cx, depth);
        if let Resolution::Unresolved { raw: r_raw, .. } = &mut r {
            *r_raw = raw;
        }
        r
    }

    fn resolve_inner(&self, segs: &[String], is_absolute: bool, cx: &Ctx, depth: u32) -> Resolution {
        if depth > MAX_DEPTH || segs.is_empty() {
            return unresolved("", UnresolvedReason::UnknownName);
        }
        let s0 = segs[0].as_str();
        let rest = &segs[1..];

        if is_absolute {
            return Resolution::External(join_path(&norm(s0), rest));
        }
        match s0 {
            // `crate` / `self` / `super`, continued through the module's scope
            "crate" => return self.descend(Resolution::Local(cx.package.to_string()), rest, cx, depth),
            "self" => return self.descend(Resolution::Local(cx.module.to_string()), rest, cx, depth),
            "super" => {
                let mut parts: Vec<&str> = cx.module.split("::").collect();
                let mut i = 0;
                while i < segs.len() && segs[i] == "super" {
                    if parts.len() <= 1 {
                        return unresolved("", UnresolvedReason::UnknownName);
                    }
                    parts.pop();
                    i += 1;
                }
                return self.descend(Resolution::Local(parts.join("::")), &segs[i..], cx, depth);
            }
            "Self" => {
                if let Some(ty) = cx.enclosing_fn.and_then(|f| self.fn_self_ty.get(f)) {
                    let mut v = vec![ty.clone()];
                    v.extend_from_slice(rest);
                    return self.resolve_inner(&v, false, cx, depth + 1);
                }
                return unresolved("", UnresolvedReason::SelfType);
            }
            _ => {}
        }

        // The namespace applies to the last segment only; a head segment
        // names a module or a type to look inside
        let head = Ctx { module: cx.module, package: cx.package, enclosing_fn: cx.enclosing_fn, ns: if rest.is_empty() { cx.ns } else { Namespace::Any }, cfg: cx.cfg };
        // Function-body overlay
        if let Some(f) = cx.enclosing_fn {
            if let Some(b) = self.overlays.get(f).and_then(|o| self.pick(o, s0, &head)) {
                let r = self.bind(f, cx.module, cx.package, s0, b, depth, head.ns, cx.cfg);
                return self.descend(r, rest, cx, depth);
            }
        }
        // Module scope (items and `use` bindings)
        if let Some(b) = self.scopes.get(cx.module).and_then(|s| self.pick(s, s0, &head)) {
            let r = self.bind(cx.module, cx.module, cx.package, s0, b, depth, head.ns, cx.cfg);
            return self.descend(r, rest, cx, depth);
        }
        // Generic parameter
        if cx.enclosing_fn.and_then(|f| self.fn_generics.get(f)).is_some_and(|g| g.contains(s0)) {
            return unresolved("", UnresolvedReason::GenericParam);
        }
        // Extern prelude
        let ns0 = norm(s0);
        let is_std = matches!(ns0.as_str(), "std" | "core" | "alloc");
        let is_dep = self.package_deps.get(cx.package).is_some_and(|d| d.contains(&ns0));
        if is_std || is_dep {
            return Resolution::External(join_path(&ns0, rest));
        }
        if !self.deps_known && !rest.is_empty() && !is_prelude(s0) {
            return Resolution::External(join_path(&ns0, rest));
        }
        // Std prelude
        if is_prelude(s0) {
            return Resolution::External(format!("std::{}", join_path(s0, rest)));
        }
        // Glob imports
        if let Some(sc) = self.scopes.get(cx.module) {
            if let Some(r) = self.via_globs(sc, s0, rest, cx, depth) {
                return r;
            }
        }
        unresolved("", UnresolvedReason::UnknownName)
    }

    /// Resolve what a binding names, memoised per scope; a cycle yields
    /// `UnknownName`.
    #[allow(clippy::too_many_arguments)]
    /// The binding for `name` in this scope for the position
    /// `cx` describes. A value-namespace item does not answer a type position
    /// when a `use` of the name exists (and the other way round). Among
    /// bindings of one name under different cfgs, the one the occurrence's
    /// cfg implies wins; else the one active in the default build; when the
    /// twins together cover every build the reference is marked covered.
    fn pick<'s>(&self, sc: &'s Scope, name: &str, cx: &Ctx) -> Option<&'s Bind> {
        let b = sc.bindings.get(name)?;
        if let Some(tw) = sc.twins.get(name) {
            let cands: Vec<&(Bind, CfgPredicate)> = tw.iter().filter(|(b, _)| ns_fits(b, cx.ns)).collect();
            if cands.len() > 1 && cands.iter().any(|(_, c)| *c != cands[0].1) {
                if let Some(rc) = cx.cfg {
                    let implied: Vec<&&(Bind, CfgPredicate)> = cands.iter().filter(|(_, c)| matches!(rc.implies(c, None), Implication::Always)).collect();
                    if implied.len() == 1 {
                        return Some(&implied[0].0);
                    }
                }
                // the build the reference is compiled in: defaults plus the features
                // its own cfg asks for, no cfg keys set
                let env = self.default_envs.get(cx.package).map(|e| {
                    let mut e = e.clone();
                    if let Some(rc) = cx.cfg {
                        e.features.extend(positive_features(rc));
                    }
                    e
                });
                let chosen = env.and_then(|e| cands.iter().find(|(_, c)| c.eval(&e) == Some(true)).copied()).unwrap_or(cands[0]);
                // covered = in every build the reference exists in, one twin exists
                // (tokio's `blocking::JoinHandle`: a `use` under `rt`, a struct under
                // `not(rt)`, both inside a module gated on `fs`/`net`/…)
                let together = CfgPredicate::Any(cands.iter().map(|(_, c)| c.clone()).collect());
                let base = cx.cfg.cloned().unwrap_or(CfgPredicate::True);
                let verdict = match self.feature_models.get(cx.package) {
                    Some(model) => model.closed_implies(&base, &together),
                    None => base.implies(&together, None),
                };
                if matches!(verdict, Implication::Always) {
                    self.covered.set(true);
                }
                return Some(&chosen.0);
            }
        }
        if let Some(alt) = sc.shadow.get(name) {
            if !ns_fits(b, cx.ns) && ns_fits(alt, cx.ns) {
                return Some(alt);
            }
        }
        Some(b)
    }

    /// After a prefix that names a module, the next segment is
    /// looked up in that module's scope (items, `use` bindings, globs) in
    /// the namespace of the position, so `crate::ident::Ident` follows the
    /// re-export a type position needs instead of spelling a non-item.
    fn descend(&self, r: Resolution, rest: &[String], cx: &Ctx, depth: u32) -> Resolution {
        if rest.is_empty() || depth > MAX_DEPTH {
            return append(r, rest);
        }
        let Resolution::Local(prefix) = &r else { return append(r, rest) };
        let (Some(sc), Some(mi)) = (self.scopes.get(prefix), self.modules.get(prefix)) else { return append(r, rest) };
        let s0 = &rest[0];
        let tail = &rest[1..];
        let ns = if tail.is_empty() { cx.ns } else { Namespace::Any };
        let inner = Ctx { module: prefix, package: &mi.package, enclosing_fn: None, ns, cfg: cx.cfg };
        if let Some(b) = self.pick(sc, s0, &inner) {
            let r2 = self.bind(prefix, prefix, &mi.package, s0, b, depth + 1, ns, cx.cfg);
            return self.descend(r2, tail, cx, depth + 1);
        }
        if let Some(r2) = self.via_globs(sc, s0, tail, &inner, depth + 1) {
            return r2;
        }
        append(r, rest)
    }

    #[allow(clippy::too_many_arguments)]
    fn bind(&self, scope_id: &str, module: &str, package: &str, name: &str, b: &Bind, depth: u32, ns: Namespace, cfg: Option<&CfgPredicate>) -> Resolution {
        // the chosen binding is part of the key: cfg twins share a name;
        // so is the reference's cfg, which decides twins further down the chain
        let key = format!("{scope_id}|{name}|{ns:?}|{b:?}|{}", cfg.map(|c| c.render()).unwrap_or_default());
        if let Some((r, cov)) = self.memo.borrow().get(&key) {
            if *cov {
                self.covered.set(true);
            }
            return r.clone();
        }
        self.memo.borrow_mut().insert(key.clone(), (unresolved(name, UnresolvedReason::UnknownName), false));
        let outer = self.covered.get();
        self.covered.set(false);
        let r = match b {
            Bind::Item(c, _) => Resolution::Local(c.clone()),
            Bind::Extern(n) => Resolution::External(norm(n)),
            Bind::Use { target, is_absolute, .. } => {
                self.resolve(target, *is_absolute, &Ctx { module, package, enclosing_fn: None, ns, cfg }, depth + 1)
            }
        };
        let cov = self.covered.get();
        self.covered.set(outer || cov);
        self.memo.borrow_mut().insert(key, (r.clone(), cov));
        r
    }

    /// Looks `s0` up through the scope's glob imports: one verified hit wins,
    /// several are reported as candidates.
    fn via_globs(&self, sc: &Scope, s0: &str, rest: &[String], cx: &Ctx, depth: u32) -> Option<Resolution> {
        let mut entries: Vec<(String, bool)> = Vec::new(); // (candidate, is a verified local hit)
        for (target, abs) in &sc.globs {
            let key = (cx.module.to_string(), target.clone(), *abs);
            let cached = self.glob_targets.borrow().get(&key).cloned();
            let t = match cached {
                Some(Some(r)) => r,
                Some(None) => unresolved("", UnresolvedReason::UnknownName),
                None => {
                    self.glob_targets.borrow_mut().insert(key.clone(), None);
                    let r = self.resolve(target, *abs, &Ctx { module: cx.module, package: cx.package, enclosing_fn: None, ns: Namespace::Any, cfg: None }, depth + 1);
                    self.glob_targets.borrow_mut().insert(key, Some(r.clone()));
                    r
                }
            };
            match t {
                Resolution::Local(m) => {
                    if self.modules.get(&m).is_some_and(|mi| mi.items.contains_key(s0)) {
                        entries.push((join_path(&format!("{m}::{s0}"), rest), true));
                    }
                }
                Resolution::External(e) => entries.push((join_path(&format!("{e}::{s0}"), rest), false)),
                Resolution::Unresolved { candidates, .. } => {
                    for c in candidates {
                        entries.push((join_path(&format!("{c}::{s0}"), rest), false));
                    }
                }
            }
        }
        if entries.is_empty() {
            return None;
        }
        if entries.len() == 1 && entries[0].1 {
            return Some(Resolution::Local(entries[0].0.clone()));
        }
        let reason = if entries.len() > 1 { UnresolvedReason::Ambiguous } else { UnresolvedReason::GlobImport };
        Some(Resolution::Unresolved { raw: String::new(), reason, candidates: entries.into_iter().map(|e| e.0).collect() })
    }

    /// A single identifier is kept only when it names something —
    /// a binding, an item, `Self`, or a glob candidate (uppercase only, so
    /// local variables in a module with external globs stay out).
    fn resolve_single(&self, s0: &str, cx: &Ctx) -> Option<Resolution> {
        if s0 == "Self" {
            let r = self.resolve(&[s0.to_string()], false, cx, 0);
            return matches!(r, Resolution::Local(_) | Resolution::External(_)).then_some(r);
        }
        let bound = cx.enclosing_fn.and_then(|f| self.overlays.get(f)).is_some_and(|o| o.bindings.contains_key(s0))
            || self.scopes.get(cx.module).is_some_and(|s| s.bindings.contains_key(s0));
        if bound {
            return Some(self.resolve(&[s0.to_string()], false, cx, 0));
        }
        if cx.enclosing_fn.and_then(|f| self.fn_generics.get(f)).is_some_and(|g| g.contains(s0)) {
            return None;
        }
        if is_prelude(s0) {
            return None;
        }
        let sc = self.scopes.get(cx.module)?;
        if sc.globs.is_empty() {
            return None;
        }
        let mut r = self.via_globs(sc, s0, &[], cx, 0)?;
        if let Resolution::Unresolved { raw, .. } = &mut r {
            *raw = s0.to_string();
            if !s0.chars().next().is_some_and(char::is_uppercase) {
                return None;
            }
        }
        Some(r)
    }

    /// A path as spelled in source (`store::Engine`, `::serde::Serialize`).
    pub(crate) fn resolve_path(&self, raw: &str, cx: &Ctx) -> Resolution {
        let (abs, segs) = split_path(raw);
        self.resolve(&segs, abs, cx, 0)
    }

    /// Put an item's generic parameters and `Self` type in scope
    /// under its id, so a `Ctx { enclosing_fn: Some(id), ns: Namespace::Any, cfg: None }` resolves generics and `Self`.
    pub(crate) fn add_item_context(&mut self, id: &str, generics: impl IntoIterator<Item = String>, self_ty: Option<String>) {
        self.fn_generics.insert(id.to_string(), generics.into_iter().collect());
        if let Some(t) = self_ty {
            self.fn_self_ty.insert(id.to_string(), t);
        }
    }
}

/// Package and module key of a file, shared by every resolution pass.
pub(crate) struct CtxParts {
    crate_pkg: String,
    file_pkg: HashMap<PathBuf, String>,
}

impl CtxParts {
    pub(crate) fn new(k: &Klepto) -> Self {
        Self { crate_pkg: norm(&k.crate_name), file_pkg: file_packages(k) }
    }

    pub(crate) fn package_of(&self, p: &Path) -> String {
        self.file_pkg.get(p).cloned().unwrap_or_else(|| self.crate_pkg.clone())
    }

    /// `(package, module key)` for an occurrence in file `p` at `module_path`.
    pub(crate) fn parts(&self, p: &Path, module_path: &[String]) -> (String, String) {
        let pkg = self.package_of(p);
        let module = join_key(&pkg, module_path);
        (pkg, module)
    }
}

/// The scope tables and per-function context.
pub(crate) fn build_resolver(k: &Klepto) -> Resolver {
    let parts = CtxParts::new(k);
    let ctx_parts = |p: &Path, module_path: &[String]| -> (String, String) { parts.parts(p, module_path) };

    // Items (with their cfg, for twin coverage)
    let item_cfgs: HashMap<&str, &CfgPredicate> = k.items.iter().map(|i| (i.id.as_str(), &i.cfg)).collect();
    let mut scopes: HashMap<String, Scope> = HashMap::new();
    for (key, mi) in &k.modules.modules {
        let sc = scopes.entry(key.clone()).or_default();
        for (name, kind) in &mi.items {
            if *kind != ItemKind::Use {
                let id = format!("{key}::{name}");
                let cfg = if *kind == ItemKind::Mod {
                    k.modules.modules.get(&id).map(|m| m.cfg.clone())
                } else {
                    item_cfgs.get(id.as_str()).map(|c| (*c).clone())
                }
                .unwrap_or(CfgPredicate::True);
                sc.bindings.insert(name.clone(), Bind::Item(id.clone(), *kind));
                sc.twins.entry(name.clone()).or_default().push((Bind::Item(id, *kind), cfg));
            }
        }
    }
    // `use` bindings, globs, overlays
    let mut overlays: HashMap<String, Scope> = HashMap::new();
    for imp in &k.imports {
        let (_, module) = ctx_parts(&imp.location.path, &imp.module_path);
        let scope = match &imp.enclosing_fn {
            Some(f) => overlays.entry(f.clone()).or_default(),
            None => scopes.entry(module).or_default(),
        };
        let mut target = vec![imp.root.clone()];
        target.extend(imp.segments.iter().cloned());
        let is_absolute = imp.is_absolute.unwrap_or(false);
        match &imp.kind {
            UseKind::Glob => {
                target.pop();
                scope.globs.push((target, is_absolute));
            }
            UseKind::Rename { alias } => {
                bind_use(scope, alias.clone(), Bind::Use { target, is_absolute, cfg: imp.cfg.clone() });
            }
            UseKind::Name => {
                let name = target.last().cloned().unwrap_or_default();
                bind_use(scope, name, Bind::Use { target, is_absolute, cfg: imp.cfg.clone() });
            }
        }
    }
    // `macro_rules!` names are textually scoped —
    // with `#[macro_use]` on the module, or simply by source order — across
    // the whole crate (hyper's `trace!`, `ffi_fn!`). klepto over-approximates:
    // every module of the package sees the package's `macro_rules!` names;
    // the module's own items and `use` bindings, inserted before, still win.
    // Visible in the defining module and its descendants;
    // `#[macro_use]` on a module lifts its macros to the parent, and so on up
    // to the crate root. A macro defined in `mod tests` (regex-syntax's own
    // `assert_eq!`) is not what `assert_eq!` means elsewhere.
    let mut package_macros: Vec<(String, String, String, String)> = Vec::new(); // (package, name, canonical, defining module)
    for (key, mi) in &k.modules.modules {
        for (name, kind) in &mi.items {
            if *kind == ItemKind::Macro {
                package_macros.push((mi.package.clone(), name.clone(), format!("{key}::{name}"), key.clone()));
            }
        }
    }
    for (pkg, name, canonical, key) in &package_macros {
        let mut root = key.clone();
        while root != *pkg && k.modules.modules.get(&root).is_some_and(|m| m.macro_use) {
            root = root.rsplit_once("::").map(|(p, _)| p.to_string()).unwrap_or_else(|| pkg.clone());
        }
        let prefix = format!("{root}::");
        for mi in k.modules.modules.values().filter(|mi| &mi.package == pkg && (mi.path == root || mi.path.starts_with(&prefix))) {
            scopes.entry(mi.path.clone()).or_default().bindings.entry(name.clone()).or_insert(Bind::Item(canonical.clone(), ItemKind::Macro));
        }
    }
    for e in &k.extern_crates {
        let (pkg, module) = ctx_parts(&e.location.path, &e.module_path);
        let name = e.alias.clone().unwrap_or_else(|| e.name.clone());
        scopes.entry(module.clone()).or_default().bindings.entry(name.clone()).or_insert(Bind::Extern(e.name.clone()));
        // `extern crate x [as y]` at the crate root
        // joins the extern prelude — the name is visible in every module of
        // the package (`extern crate serde_core as serde;` in serde_json).
        // Items and `use` bindings, inserted before, still win.
        if module == pkg {
            for mi in k.modules.modules.values().filter(|mi| mi.package == pkg) {
                scopes.entry(mi.path.clone()).or_default().bindings.entry(name.clone()).or_insert(Bind::Extern(e.name.clone()));
            }
        }
    }
    // generics and `Self` context per function
    let mut fn_generics = HashMap::new();
    let mut fn_self_ty = HashMap::new();
    for f in &k.functions {
        fn_generics.insert(f.fq_name.clone(), f.generics.iter().cloned().collect::<HashSet<_>>());
        match &f.kind {
            FnKind::ImplMethod { self_ty, .. } => {
                fn_self_ty.insert(f.fq_name.clone(), self_ty.split('<').next().unwrap_or(self_ty).to_string());
            }
            // Inside a trait, `Self` is the trait
            FnKind::TraitMethod { trait_name } => {
                fn_self_ty.insert(f.fq_name.clone(), trait_name.clone());
            }
            FnKind::FreeFn => {}
        }
    }
    // Impl blocks and type declarations are
    // contexts too — `B::Data` in an impl header or associated type, `Self`
    // in an impl's associated items — keyed by their item id (pre-rename,
    // like the occurrences that name them as `enclosing_item`).
    for i in &k.impls {
        let names: HashSet<String> = i.generics.iter().filter(|g| g.kind != crate::typing::types::GenericKind::Lifetime).map(|g| g.name.clone()).collect();
        fn_generics.entry(i.id.clone()).or_insert(names);
        let head = i.self_ty.raw.split('<').next().unwrap_or(&i.self_ty.raw);
        let head = head.rsplit("::").next().unwrap_or(head).trim_start_matches('&').trim();
        fn_self_ty.entry(i.id.clone()).or_insert_with(|| head.to_string());
    }
    for t in &k.types {
        let names: HashSet<String> = t.generics.iter().filter(|g| g.kind != crate::typing::types::GenericKind::Lifetime).map(|g| g.name.clone()).collect();
        fn_generics.entry(t.id.clone()).or_insert(names);
        fn_self_ty.entry(t.id.clone()).or_insert_with(|| t.name.clone());
    }

    Resolver {
        modules: k.modules.clone(),
        scopes,
        overlays,
        deps_known: !k.package_deps.is_empty(),
        package_deps: k.package_deps.clone(),
        fn_generics,
        fn_self_ty,
        memo: RefCell::new(HashMap::new()),
        default_envs: k.features.iter().map(|(p, m)| (p.clone(), m.default_env())).collect(),
        feature_models: k.features.clone(),
        covered: std::cell::Cell::new(false),
        glob_targets: RefCell::new(HashMap::new()),
    }
}

/// Builds the scope tables and fills every
/// `resolution` field, then canonicalises trait paths in `fq_name`s.
pub(crate) fn resolve_all(k: &mut Klepto) {
    let parts = CtxParts::new(k);
    let ctx_parts = |p: &Path, module_path: &[String]| -> (String, String) { parts.parts(p, module_path) };
    let r = build_resolver(k);

    // multi-segment paths (outside a fn, the enclosing impl / type is the
    // context)
    for p in k.paths.iter_mut() {
        let (pkg, module) = ctx_parts(&p.location.path, &p.module_path);
        let (abs, segs) = split_path(&p.path);
        let cx = Ctx { module: &module, package: &pkg, enclosing_fn: p.enclosing_fn.as_deref().or(p.enclosing_item.as_deref()), ns: p.ns, cfg: Some(&p.cfg) };
        r.covered.set(false);
        p.resolution = Some(r.resolve(&segs, abs, &cx, 0));
        p.covered = r.covered.get();
    }
    // Single identifiers
    let pending = std::mem::take(&mut k.pending_singles);
    for mut p in pending {
        let (pkg, module) = ctx_parts(&p.location.path, &p.module_path);
        let cx = Ctx { module: &module, package: &pkg, enclosing_fn: p.enclosing_fn.as_deref().or(p.enclosing_item.as_deref()), ns: p.ns, cfg: Some(&p.cfg) };
        r.covered.set(false);
        if let Some(res) = r.resolve_single(&p.path, &cx) {
            p.resolution = Some(res);
            p.covered = r.covered.get();
            k.paths.push(p);
        }
    }
    // Macros
    for m in k.macros_inv.iter_mut() {
        let (pkg, module) = ctx_parts(&m.location.path, &m.module_path);
        let cx = Ctx { module: &module, package: &pkg, enclosing_fn: m.enclosing_fn.as_deref().or(m.enclosing_item.as_deref()), ns: Namespace::Any, cfg: Some(&m.cfg) };
        r.covered.set(false);
        m.resolution = Some(match m.path.as_deref() {
            Some(p) => {
                let (abs, segs) = split_path(p);
                r.resolve(&segs, abs, &cx, 0)
            }
            None => unresolved(&m.name, UnresolvedReason::UnknownName),
        });
        m.covered = r.covered.get();
    }
    // Calls
    for c in k.calls.iter_mut() {
        let (pkg, module) = ctx_parts(&c.location.path, &c.module_path);
        let cx = Ctx { module: &module, package: &pkg, enclosing_fn: c.enclosing_fn.as_deref().or(c.enclosing_item.as_deref()), ns: Namespace::Any, cfg: Some(&c.cfg) };
        r.covered.set(false);
        c.resolution = Some(match (c.is_method, callee_segments(&c.callee)) {
            (false, Some((segs, abs))) => r.resolve(&segs, abs, &cx, 0),
            _ => unresolved(&c.callee, UnresolvedReason::MethodCall),
        });
        c.covered = r.covered.get();
    }
    // Attributes
    for a in k.attrs.iter_mut() {
        let (pkg, module) = ctx_parts(&a.location.path, &a.module_path);
        let cx = Ctx { module: &module, package: &pkg, enclosing_fn: a.enclosing_fn.as_deref().or(a.enclosing_item.as_deref()), ns: Namespace::Any, cfg: None };
        let (abs, segs) = split_path(&a.path);
        let own = r.resolve(&segs, abs, &cx, 0);
        // With no dependency list, a bare non-builtin attribute name is a crate
        a.resolution = Some(match own {
            Resolution::Unresolved { reason: UnresolvedReason::UnknownName, .. }
                if !r.deps_known && segs.len() == 1 && !is_builtin_attr(&segs[0]) =>
            {
                Resolution::External(norm(&segs[0]))
            }
            other => other,
        });
        a.arg_resolutions = a
            .arg_paths
            .iter()
            .map(|ap| {
                let (abs, segs) = split_path(ap);
                r.resolve(&segs, abs, &cx, 0)
            })
            .collect();
    }
    // Use targets and exports
    for imp in k.imports.iter_mut() {
        let (pkg, module) = ctx_parts(&imp.location.path, &imp.module_path);
        let cx = Ctx { module: &module, package: &pkg, enclosing_fn: None, ns: Namespace::Any, cfg: None };
        let mut target = vec![imp.root.clone()];
        target.extend(imp.segments.iter().cloned());
        if matches!(imp.kind, UseKind::Glob) {
            target.pop();
        }
        imp.resolution = Some(r.resolve(&target, imp.is_absolute.unwrap_or(false), &cx, 0));
    }
    for e in k.exports.iter_mut() {
        let (pkg, module) = ctx_parts(&e.location.path, &e.module_path);
        let cx = Ctx { module: &module, package: &pkg, enclosing_fn: None, ns: Namespace::Any, cfg: None };
        let (abs, mut segs) = split_path(&e.source_path);
        if segs.last().is_some_and(|s| s == "*") {
            segs.pop();
        }
        e.resolution = Some(r.resolve(&segs, abs, &cx, 0));
    }

    // Canonical trait paths in trait-impl fq_names, applied everywhere
    let mut rename: HashMap<String, String> = HashMap::new();
    for f in k.functions.iter_mut() {
        let (pkg, module) = ctx_parts(&f.location.path, &f.module_path);
        let FnKind::ImplMethod { self_ty, trait_ty: Some(t) } = &mut f.kind else { continue };
        let (abs, segs) = split_path(t);
        let canonical = match r.resolve(&segs, abs, &Ctx { module: &module, package: &pkg, enclosing_fn: None, ns: Namespace::Any, cfg: None }, 0) {
            Resolution::Local(s) | Resolution::External(s) => s,
            _ => continue,
        };
        if canonical == *t {
            continue;
        }
        *t = canonical.clone();
        let kind = FnKind::ImplMethod { self_ty: self_ty.clone(), trait_ty: Some(canonical) };
        let new = fq_name(&pkg, &f.module_path, &kind, &f.name);
        let old = std::mem::replace(&mut f.fq_name, new.clone());
        rename.insert(old, new);
    }
    // canonical trait paths in impl-block ids: `<impl Trait for Ty>` with the
    // trait canonicalised; nested ids under them follow through the map.
    for d in k.items.iter_mut() {
        if d.kind != crate::graph::NodeKind::Impl {
            continue;
        }
        let Some((prefix, label)) = d.id.rsplit_once("::<impl ") else { continue };
        let Some((raw_trait, rest)) = label.split_once(" for ") else { continue };
        let ty = rest.strip_suffix('>').unwrap_or(rest);
        let (abs, segs) = split_path(raw_trait);
        let canonical = match r.resolve(&segs, abs, &Ctx { module: &d.module, package: &d.package, enclosing_fn: None, ns: Namespace::Any, cfg: None }, 0) {
            Resolution::Local(s) | Resolution::External(s) => s,
            _ => continue,
        };
        if canonical == raw_trait {
            continue;
        }
        let new = format!("{prefix}::<impl {canonical} for {ty}>");
        d.name = format!("<impl {canonical} for {ty}>");
        let old = std::mem::replace(&mut d.id, new.clone());
        rename.insert(old, new);
    }
    if !rename.is_empty() {
        let fix = |e: &mut Option<String>| {
            if let Some(n) = e.as_ref().and_then(|s| rename.get(s)) {
                *e = Some(n.clone());
            }
        };
        for p in k.paths.iter_mut() { fix(&mut p.enclosing_fn); fix(&mut p.enclosing_item); }
        for c in k.calls.iter_mut() { fix(&mut c.enclosing_fn); fix(&mut c.enclosing_item); }
        for m in k.macros_inv.iter_mut() { fix(&mut m.enclosing_fn); fix(&mut m.enclosing_item); }
        for a in k.attrs.iter_mut() { fix(&mut a.enclosing_fn); fix(&mut a.enclosing_item); }
        for i in k.imports.iter_mut() { fix(&mut i.enclosing_fn); fix(&mut i.enclosing_item); }
        for d in k.items.iter_mut() {
            if let Some(n) = rename.get(&d.id) { d.id = n.clone(); }
            fix(&mut d.parent);
        }
        // Impl records carry the same ids as the nodes
        for i in k.impls.iter_mut() {
            if let Some(n) = rename.get(&i.id) { i.id = n.clone(); }
            for m in i.methods.iter_mut() {
                if let Some(n) = rename.get(m) { *m = n.clone(); }
            }
        }
        k.index.rename_fns(&rename);
    }
}
