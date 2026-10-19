use crate::resolve::cfg::{cfg_of_attrs, CfgPredicate};
use crate::graph::NodeKind;
use crate::model::*;
use crate::resolve::join_key;
use crate::typing::types::{normalize_type_spelling, FieldDecl, GenericKind, GenericParam, ImplDecl, MethodSig, TypeDecl, TypeKind, TypeRef, VariantDecl};
use proc_macro2::{Span, TokenStream, TokenTree};
use quote::ToTokens;
use syn::{Attribute, File, Item, Visibility, spanned::Spanned, visit::Visit};

pub(crate) fn span_to_location(path: &std::path::Path, span: Span) -> FileLocation {
    #[cfg(feature = "span-locations")]
    let (line, column) = {
        let start = span.start();
        (Some(start.line as u32), Some(start.column as u32))
    };
    #[cfg(not(feature = "span-locations"))]
    let (line, column) = {
        let _ = span;
        (None, None)
    };
    FileLocation { path: path.to_path_buf(), line, column }
}

fn vis_is_public(vis: &Visibility) -> bool {
    matches!(vis, Visibility::Public(_))
}

fn has_docs(attrs: &[Attribute]) -> bool {
    // the generated marker is a doc attribute but not documentation
    attrs.iter().any(|a| a.path().is_ident("doc") && !crate::resolve::expand::is_marker(a))
}

fn attr_paths(attrs: &[Attribute]) -> Vec<String> {
    attrs.iter().map(|a| path_to_string(a.path())).collect()
}

fn path_to_string(p: &syn::Path) -> String {
    p.segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

/// Like `path_to_string`, keeping a leading `::` so the resolver treats the
/// path as absolute.
pub(crate) fn path_string(p: &syn::Path) -> String {
    let s = path_to_string(p);
    if p.leading_colon.is_some() { format!("::{s}") } else { s }
}

fn type_to_string(t: &syn::Type) -> String {
    t.to_token_stream().to_string()
}

/// `impl Foo<T> { .. }` → `Foo<T>`: the last path segment including its
/// generic arguments, spaces removed. Non-path self types keep
/// their token spelling, spaces removed.
pub(crate) fn impl_self_ty_name(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(tp) => tp
            .path
            .segments
            .last()
            .map(|s| s.to_token_stream().to_string().replace(' ', ""))
            .unwrap_or_else(|| type_to_string(ty).replace(' ', "")),
        _ => type_to_string(ty).replace(' ', ""),
    }
}

/// `<impl Ty>` / `<impl Trait for Ty>` (trait as written; the
/// resolver canonicalises it).
pub(crate) fn impl_label(self_ty: &str, trait_ty: Option<&str>) -> String {
    match trait_ty {
        Some(t) => format!("<impl {t} for {self_ty}>"),
        None => format!("<impl {self_ty}>"),
    }
}

fn generic_names(g: &syn::Generics) -> Vec<String> {
    g.params
        .iter()
        .filter_map(|p| match p {
            syn::GenericParam::Type(t) => Some(t.ident.to_string()),
            syn::GenericParam::Const(c) => Some(c.ident.to_string()),
            syn::GenericParam::Lifetime(_) => None,
        })
        .collect()
}

fn fn_args(sig: &syn::Signature) -> Vec<String> {
    sig.inputs
        .iter()
        .map(|i| i.to_token_stream().to_string())
        .collect()
}

fn fn_return(sig: &syn::Signature) -> Option<String> {
    match &sig.output {
        syn::ReturnType::Default => None,
        syn::ReturnType::Type(_, ty) => Some(ty.to_token_stream().to_string()),
    }
}

/// `package::module::name`, `package::module::Ty::name`,
/// `package::module::<Ty as Trait>::name`, `package::module::Trait::name`.
pub(crate) fn fq_name(package: &str, module_path: &[String], kind: &FnKind, name: &str) -> String {
    let mut parts = vec![package.to_string()];
    parts.extend(module_path.iter().cloned());
    match kind {
        FnKind::FreeFn => parts.push(name.to_string()),
        FnKind::TraitMethod { trait_name } => {
            parts.push(trait_name.clone());
            parts.push(name.to_string());
        }
        FnKind::ImplMethod { self_ty, trait_ty: Some(t) } => {
            parts.push(format!("<{self_ty} as {t}>"));
            parts.push(name.to_string());
        }
        FnKind::ImplMethod { self_ty, trait_ty: None } => {
            parts.push(self_ty.clone());
            parts.push(name.to_string());
        }
    }
    parts.join("::")
}

// ---------------------------------------------------------------------------
// attribute access for the cfg stack
// ---------------------------------------------------------------------------

fn item_attrs(i: &Item) -> &[Attribute] {
    match i {
        Item::Const(x) => &x.attrs,
        Item::Enum(x) => &x.attrs,
        Item::ExternCrate(x) => &x.attrs,
        Item::Fn(x) => &x.attrs,
        Item::ForeignMod(x) => &x.attrs,
        Item::Impl(x) => &x.attrs,
        Item::Macro(x) => &x.attrs,
        Item::Mod(x) => &x.attrs,
        Item::Static(x) => &x.attrs,
        Item::Struct(x) => &x.attrs,
        Item::Trait(x) => &x.attrs,
        Item::TraitAlias(x) => &x.attrs,
        Item::Type(x) => &x.attrs,
        Item::Union(x) => &x.attrs,
        Item::Use(x) => &x.attrs,
        _ => &[],
    }
}

fn impl_item_attrs(i: &syn::ImplItem) -> &[Attribute] {
    match i {
        syn::ImplItem::Const(x) => &x.attrs,
        syn::ImplItem::Fn(x) => &x.attrs,
        syn::ImplItem::Type(x) => &x.attrs,
        syn::ImplItem::Macro(x) => &x.attrs,
        _ => &[],
    }
}

fn trait_item_attrs(i: &syn::TraitItem) -> &[Attribute] {
    match i {
        syn::TraitItem::Const(x) => &x.attrs,
        syn::TraitItem::Fn(x) => &x.attrs,
        syn::TraitItem::Type(x) => &x.attrs,
        syn::TraitItem::Macro(x) => &x.attrs,
        _ => &[],
    }
}

fn stmt_attrs(s: &syn::Stmt) -> &[Attribute] {
    match s {
        syn::Stmt::Local(l) => &l.attrs,
        syn::Stmt::Macro(m) => &m.attrs,
        // Stmt::Item → visit_item, Stmt::Expr → visit_expr
        _ => &[],
    }
}

fn expr_attrs(e: &syn::Expr) -> &[Attribute] {
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

fn fn_arg_attrs(a: &syn::FnArg) -> &[Attribute] {
    match a {
        syn::FnArg::Receiver(r) => &r.attrs,
        syn::FnArg::Typed(t) => &t.attrs,
    }
}

pub fn crate_is_no_std(ast: &File) -> bool {
    ast.attrs.iter().any(|a| a.path().is_ident("no_std"))
}

/// `pub use` trees → `ExportedSymbol`. `module` is the file's module path
/// (segments without the package) and `base_cfg` the file's effective cfg;
/// inline `mod { }` blocks extend both.
pub fn extract_public_surface(file_path: &std::path::Path, module: &[String], base_cfg: &CfgPredicate, ast: &File) -> Vec<ExportedSymbol> {
    let mut out = Vec::new();

    fn walk(
        file_path: &std::path::Path,
        tree: &syn::UseTree,
        mut prefix: Vec<String>,
        module_path: &[String],
        cfg: &CfgPredicate,
        out: &mut Vec<ExportedSymbol>,
    ) {
        let mut emit = |exported_as: String, segs: Vec<String>, span: Span| {
            out.push(ExportedSymbol {
                exported_as,
                source_path: segs.join("::"),
                module_path: module_path.to_vec(),
                location: span_to_location(file_path, span),
                resolution: None,
                cfg: cfg.clone(),
            });
        };
        match tree {
            syn::UseTree::Path(p) => {
                prefix.push(p.ident.to_string());
                walk(file_path, &p.tree, prefix, module_path, cfg, out);
            }
            syn::UseTree::Group(g) => {
                for t in &g.items {
                    walk(file_path, t, prefix.clone(), module_path, cfg, out);
                }
            }
            syn::UseTree::Name(n) => {
                let mut segs = prefix;
                segs.push(n.ident.to_string());
                emit(n.ident.to_string(), segs, n.span());
            }
            syn::UseTree::Rename(r) => {
                let mut segs = prefix;
                segs.push(r.ident.to_string());
                emit(r.rename.to_string(), segs, r.span());
            }
            syn::UseTree::Glob(g) => {
                let mut segs = prefix;
                segs.push("*".to_string());
                emit("*".to_string(), segs, g.span());
            }
        }
    }

    fn walk_items(
        file_path: &std::path::Path,
        items: &[Item],
        mod_stack: &mut Vec<String>,
        cfg: &CfgPredicate,
        out: &mut Vec<ExportedSymbol>,
    ) {
        for it in items {
            match it {
                Item::Use(u) => {
                    if matches!(u.vis, Visibility::Public(_)) {
                        let c = cfg.clone().and(cfg_of_attrs(&u.attrs));
                        walk(file_path, &u.tree, Vec::new(), mod_stack, &c, out);
                    }
                }
                Item::Mod(m) => {
                    if let Some((_, items)) = &m.content {
                        let c = cfg.clone().and(cfg_of_attrs(&m.attrs));
                        mod_stack.push(m.ident.to_string());
                        walk_items(file_path, items, mod_stack, &c, out);
                        mod_stack.pop();
                    }
                }
                _ => {}
            }
        }
    }

    let mut ms = module.to_vec();
    walk_items(file_path, &ast.items, &mut ms, base_cfg, &mut out);
    out
}

// ---------------------------------------------------------------------------
// `use` trees → StolenPath
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn emit_use(
    file_path: &std::path::Path,
    module_path: &[String],
    enclosing_fn: Option<&str>,
    cfg: &CfgPredicate,
    mut segs: Vec<String>,
    is_public_use: bool,
    is_absolute: bool,
    kind: UseKind,
    span: Span,
    out: &mut Vec<StolenPath>,
) {
    if segs.is_empty() {
        return;
    }

    let root = segs.remove(0);
    let is_internal = matches!(root.as_str(), "crate" | "self" | "super");

    let base = if segs.is_empty() {
        root.clone()
    } else {
        format!("{}::{}", root, segs.join("::"))
    };

    let full_path = if is_absolute { format!("::{}", base) } else { base };

    out.push(StolenPath {
        root,
        segments: segs,
        module_path: module_path.to_vec(),
        is_internal,
        is_public_use,
        kind,
        full_path,
        location: span_to_location(file_path, span),
        origin: None, // classified in KleptoBuilder::parse() once every file is extracted
        is_absolute: Some(is_absolute),
        enclosing_fn: enclosing_fn.map(|s| s.to_string()),
        enclosing_item: None,
        resolution: None,
        cfg: cfg.clone(),
    });
}

#[allow(clippy::too_many_arguments)]
fn walk_use_tree(
    file_path: &std::path::Path,
    module_path: &[String],
    enclosing_fn: Option<&str>,
    cfg: &CfgPredicate,
    tree: &syn::UseTree,
    prefix: Vec<String>,
    is_public_use: bool,
    is_absolute: bool,
    out: &mut Vec<StolenPath>,
) {
    match tree {
        syn::UseTree::Path(p) => {
            let mut next = prefix;
            next.push(p.ident.to_string());
            walk_use_tree(file_path, module_path, enclosing_fn, cfg, &p.tree, next, is_public_use, is_absolute, out);
        }
        syn::UseTree::Group(g) => {
            for t in &g.items {
                walk_use_tree(file_path, module_path, enclosing_fn, cfg, t, prefix.clone(), is_public_use, is_absolute, out);
            }
        }
        syn::UseTree::Name(n) => {
            // `{self, X}`: `self` means the module itself
            let mut segs = prefix;
            if n.ident != "self" {
                segs.push(n.ident.to_string());
            }
            emit_use(file_path, module_path, enclosing_fn, cfg, segs, is_public_use, is_absolute, UseKind::Name, n.span(), out);
        }
        syn::UseTree::Glob(g) => {
            let mut segs = prefix;
            segs.push("*".to_string());
            emit_use(file_path, module_path, enclosing_fn, cfg, segs, is_public_use, is_absolute, UseKind::Glob, g.span(), out);
        }
        syn::UseTree::Rename(r) => {
            let mut segs = prefix;
            segs.push(r.ident.to_string());
            let kind = UseKind::Rename { alias: r.rename.to_string() };
            emit_use(file_path, module_path, enclosing_fn, cfg, segs, is_public_use, is_absolute, kind, r.span(), out);
        }
    }
}

/// `cfg` is the enclosing effective cfg; the item's own `#[cfg]` is added.
pub(crate) fn imports_from_item_use(
    file_path: &std::path::Path,
    module_path: &[String],
    enclosing_fn: Option<&str>,
    cfg: &CfgPredicate,
    u: &syn::ItemUse,
    out: &mut Vec<StolenPath>,
) {
    let is_pub = matches!(u.vis, Visibility::Public(_));
    let is_abs = u.leading_colon.is_some();
    let c = cfg.clone().and(cfg_of_attrs(&u.attrs));
    walk_use_tree(file_path, module_path, enclosing_fn, &c, &u.tree, Vec::new(), is_pub, is_abs, out);
}

/// Module-level `use` items (file root and inline `mod { }` blocks).
/// `use` items inside function bodies come from [`extract_occurrences`].
pub fn extract_imports(file_path: &std::path::Path, module: &[String], base_cfg: &CfgPredicate, ast: &syn::File) -> Vec<StolenPath> {
    fn walk_items(
        file_path: &std::path::Path,
        items: &[Item],
        mod_stack: &mut Vec<String>,
        cfg: &CfgPredicate,
        out: &mut Vec<StolenPath>,
    ) {
        for item in items {
            match item {
                Item::Use(u) => imports_from_item_use(file_path, mod_stack, None, cfg, u, out),
                Item::Mod(m) => {
                    if let Some((_, inner)) = &m.content {
                        let c = cfg.clone().and(cfg_of_attrs(&m.attrs));
                        mod_stack.push(m.ident.to_string());
                        walk_items(file_path, inner, mod_stack, &c, out);
                        mod_stack.pop();
                    }
                }
                _ => {}
            }
        }
    }

    let mut out = Vec::new();
    let mut mod_stack = module.to_vec();
    walk_items(file_path, &ast.items, &mut mod_stack, base_cfg, &mut out);
    out
}

pub fn extract_functions(
    package: &str,
    module: &[String],
    base_cfg: &CfgPredicate,
    file_path: &std::path::Path,
    ast: &File,
) -> Vec<CapturedFn> {
    let mut out = Vec::new();
    let mut mod_stack: Vec<String> = module.to_vec();

    #[allow(clippy::too_many_arguments)]
    fn captured(
        package: &str,
        file_path: &std::path::Path,
        mod_stack: &[String],
        kind: FnKind,
        sig: &syn::Signature,
        attrs: &[Attribute],
        is_public: bool,
        generics: Vec<String>,
        cfg: CfgPredicate,
        span: Span,
    ) -> CapturedFn {
        let name = sig.ident.to_string();
        let fq = fq_name(package, mod_stack, &kind, &name);
        CapturedFn {
            name,
            fq_name: fq,
            is_public,
            has_docs: has_docs(attrs),

            is_async: sig.asyncness.is_some(),
            is_unsafe: sig.unsafety.is_some(),
            is_const: sig.constness.is_some(),
            is_generic: !sig.generics.params.is_empty(),

            args: fn_args(sig),
            return_ty: fn_return(sig),

            kind,
            module_path: mod_stack.to_vec(),
            attrs: attr_paths(attrs),
            signature: sig.to_token_stream().to_string(),
            location: span_to_location(file_path, span),
            generics,
            cfg,
            generated: crate::resolve::expand::generated_by(attrs),
        }
    }

    fn walk_items(
        package: &str,
        file_path: &std::path::Path,
        items: &[Item],
        mod_stack: &mut Vec<String>,
        cfg: &CfgPredicate,
        out: &mut Vec<CapturedFn>,
    ) {
        for item in items {
            match item {
                Item::Fn(f) => {
                    let generics = generic_names(&f.sig.generics);
                    let c = cfg.clone().and(cfg_of_attrs(&f.attrs));
                    out.push(captured(package, file_path, mod_stack, FnKind::FreeFn, &f.sig, &f.attrs, vis_is_public(&f.vis), generics, c, f.span()));
                }
                Item::Impl(imp) => {
                    let self_ty = impl_self_ty_name(&imp.self_ty);
                    let trait_ty = imp.trait_.as_ref().map(|(_, path, _)| path_to_string(path));
                    let impl_generics = generic_names(&imp.generics);
                    let impl_cfg = cfg.clone().and(cfg_of_attrs(&imp.attrs));

                    for it in &imp.items {
                        if let syn::ImplItem::Fn(m) = it {
                            let kind = FnKind::ImplMethod { self_ty: self_ty.clone(), trait_ty: trait_ty.clone() };
                            let mut generics = impl_generics.clone();
                            generics.extend(generic_names(&m.sig.generics));
                            let c = impl_cfg.clone().and(cfg_of_attrs(&m.attrs));
                            out.push(captured(package, file_path, mod_stack, kind, &m.sig, &m.attrs, vis_is_public(&m.vis), generics, c, m.span()));
                        }
                    }
                }
                Item::Trait(t) => {
                    let trait_name = t.ident.to_string();
                    let trait_generics = generic_names(&t.generics);
                    let trait_cfg = cfg.clone().and(cfg_of_attrs(&t.attrs));
                    for it in &t.items {
                        if let syn::TraitItem::Fn(tf) = it {
                            let kind = FnKind::TraitMethod { trait_name: trait_name.clone() };
                            let mut generics = trait_generics.clone();
                            generics.extend(generic_names(&tf.sig.generics));
                            let c = trait_cfg.clone().and(cfg_of_attrs(&tf.attrs));
                            out.push(captured(package, file_path, mod_stack, kind, &tf.sig, &tf.attrs, true, generics, c, tf.span()));
                        }
                    }
                }
                Item::Mod(m) => {
                    if let Some((_, items)) = &m.content {
                        let c = cfg.clone().and(cfg_of_attrs(&m.attrs));
                        mod_stack.push(m.ident.to_string());
                        walk_items(package, file_path, items, mod_stack, &c, out);
                        mod_stack.pop();
                    }
                }
                _ => {}
            }
        }
    }

    walk_items(package, file_path, &ast.items, &mut mod_stack, base_cfg, &mut out);
    out
}

/// Paths (`a::b`, and single identifiers) inside an attribute's
/// argument tokens, recursing into groups.
fn paths_in_tokens(ts: &TokenStream) -> Vec<String> {
    fn flush(segs: &mut Vec<String>, out: &mut Vec<String>) {
        if !segs.is_empty() {
            out.push(segs.join("::"));
        }
        segs.clear();
    }

    fn walk(ts: &TokenStream, out: &mut Vec<String>) {
        let mut segs: Vec<String> = Vec::new();
        let mut colons = 0u8; // ':' punctuation seen since the last ident
        for tt in ts.clone() {
            match tt {
                TokenTree::Ident(id) => {
                    if !segs.is_empty() && colons != 2 {
                        flush(&mut segs, out);
                    }
                    segs.push(id.to_string());
                    colons = 0;
                }
                TokenTree::Punct(p) if p.as_char() == ':' => {
                    colons += 1;
                    if colons > 2 {
                        flush(&mut segs, out);
                        colons = 0;
                    }
                }
                TokenTree::Group(g) => {
                    flush(&mut segs, out);
                    colons = 0;
                    walk(&g.stream(), out);
                }
                _ => {
                    flush(&mut segs, out);
                    colons = 0;
                }
            }
        }
        flush(&mut segs, out);
    }

    let mut out = Vec::new();
    walk(ts, &mut out);
    out
}

/// Top-level comma-separated chunks of a token stream (for `cfg_attr`).
fn split_top_level_commas(ts: &TokenStream) -> Vec<Vec<TokenTree>> {
    let mut out: Vec<Vec<TokenTree>> = vec![Vec::new()];
    for tt in ts.clone() {
        match &tt {
            TokenTree::Punct(p) if p.as_char() == ',' => out.push(Vec::new()),
            _ => out.last_mut().expect("non-empty").push(tt),
        }
    }
    out.retain(|c| !c.is_empty());
    out
}

// ---------------------------------------------------------------------------
// Type records
// ---------------------------------------------------------------------------

const PRIMITIVES: &[&str] = &[
    "bool", "char", "str", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32", "f64",
];

fn is_primitive(p: &syn::Path) -> bool {
    p.leading_colon.is_none() && p.segments.len() == 1 && PRIMITIVES.contains(&p.segments[0].ident.to_string().as_str())
}

/// A mention as spelled; `resolve_types` replaces it.
fn placeholder(raw: String) -> Resolution {
    Resolution::Unresolved { raw, reason: UnresolvedReason::UnknownName, candidates: Vec::new() }
}

/// Every non-primitive path inside a type or bound, in source order, the
/// head first.
#[derive(Default)]
struct PathCollector(Vec<Resolution>);

impl<'ast> Visit<'ast> for PathCollector {
    fn visit_path(&mut self, p: &'ast syn::Path) {
        if !is_primitive(p) {
            self.0.push(placeholder(path_string(p)));
        }
        syn::visit::visit_path(self, p);
    }
}

fn spelled<T: ToTokens>(t: &T) -> String {
    normalize_type_spelling(&t.to_token_stream().to_string())
}

fn type_ref(t: &syn::Type) -> TypeRef {
    let mut c = PathCollector::default();
    c.visit_type(t);
    TypeRef { raw: spelled(t), mentions: c.0 }
}

fn path_ref(p: &syn::Path) -> TypeRef {
    let mut c = PathCollector::default();
    c.visit_path(p);
    TypeRef { raw: spelled(p), mentions: c.0 }
}

/// Trait bounds as written; lifetime bounds are dropped.
fn bounds_of<'a>(bounds: impl IntoIterator<Item = &'a syn::TypeParamBound>) -> Vec<TypeRef> {
    bounds
        .into_iter()
        .filter_map(|b| match b {
            syn::TypeParamBound::Trait(tb) => {
                let mut c = PathCollector::default();
                c.visit_trait_bound(tb);
                Some(TypeRef { raw: spelled(tb), mentions: c.0 })
            }
            _ => None,
        })
        .collect()
}

/// Parameters in order; `where` predicates on exactly one
/// parameter fold into its bounds.
fn generic_params(g: &syn::Generics) -> Vec<GenericParam> {
    let mut out: Vec<GenericParam> = g
        .params
        .iter()
        .map(|p| match p {
            syn::GenericParam::Lifetime(l) => GenericParam {
                name: l.lifetime.to_string(),
                kind: GenericKind::Lifetime,
                bounds: Vec::new(),
                ty: None,
                default: None,
            },
            syn::GenericParam::Type(t) => GenericParam {
                name: t.ident.to_string(),
                kind: GenericKind::Type,
                bounds: bounds_of(t.bounds.iter()),
                ty: None,
                default: t.default.as_ref().map(type_ref),
            },
            syn::GenericParam::Const(c) => GenericParam {
                name: c.ident.to_string(),
                kind: GenericKind::Const,
                bounds: Vec::new(),
                ty: Some(type_ref(&c.ty)),
                default: c.default.as_ref().map(|e| TypeRef { raw: spelled(e), mentions: Vec::new() }),
            },
        })
        .collect();
    if let Some(wc) = &g.where_clause {
        for pred in &wc.predicates {
            let syn::WherePredicate::Type(pt) = pred else { continue };
            let syn::Type::Path(tp) = &pt.bounded_ty else { continue };
            if tp.qself.is_some() || tp.path.segments.len() != 1 {
                continue;
            }
            let name = tp.path.segments[0].ident.to_string();
            if let Some(gp) = out.iter_mut().find(|gp| gp.kind == GenericKind::Type && gp.name == name) {
                gp.bounds.extend(bounds_of(pt.bounds.iter()));
            }
        }
    }
    out
}

/// Fields in order; cfg = the owner's and the field's own.
fn field_decls<'a>(fields: impl IntoIterator<Item = &'a syn::Field>, owner_cfg: &CfgPredicate) -> Vec<FieldDecl> {
    fields
        .into_iter()
        .enumerate()
        .map(|(index, f)| FieldDecl {
            index,
            name: f.ident.as_ref().map(|i| i.to_string()),
            ty: type_ref(&f.ty),
            is_public: vis_is_public(&f.vis),
            cfg: owner_cfg.clone().and(cfg_of_attrs(&f.attrs)),
            attrs: attr_paths(&f.attrs),
        })
        .collect()
}

/// `#[derive(..)]` arguments as spelled.
fn derive_placeholders(attrs: &[Attribute]) -> Vec<Resolution> {
    let mut out = Vec::new();
    for a in attrs {
        if !a.path().is_ident("derive") {
            continue;
        }
        if let syn::Meta::List(l) = &a.meta {
            out.extend(paths_in_tokens(&l.tokens).into_iter().map(placeholder));
        }
    }
    out
}

/// Everything the occurrence visitor collects for one file.
#[derive(Debug, Default, Clone)]
pub struct Occurrences {
    pub macros_def: Vec<MacroDef>,
    pub macros_inv: Vec<MacroInvocation>,
    pub paths: Vec<PathOccurrence>,
    pub calls: Vec<CallOccurrence>,
    pub attrs: Vec<AttrOccurrence>,
    pub extern_crates: Vec<ExternCrateOccurrence>,
    /// `use` items inside function bodies or other blocks; module-level
    /// `use` items come from [`extract_imports`].
    pub body_imports: Vec<StolenPath>,
    /// Single-identifier paths (`helper`, `Widget`, `Self`). The resolver
    /// keeps the ones that name an item and drops the rest.
    pub single_paths: Vec<PathOccurrence>,
    /// Every item declaration of every kind.
    pub items: Vec<ItemDecl>,
    /// Type records, unresolved until `resolve_types`.
    pub types: Vec<crate::typing::types::TypeDecl>,
    /// Impl records, unresolved until `resolve_types`.
    pub impls: Vec<crate::typing::types::ImplDecl>,
    /// String literals in code.
    pub literals: Vec<LiteralOccurrence>,
    /// `match` expressions and their arms' pattern paths.
    pub matches: Vec<MatchOccurrence>,
}

/// Macro invocations, path and call occurrences, attributes, `extern crate`
/// items, block-level `use` items and item declarations, each tagged with
/// its enclosing function / item and effective cfg.
pub fn extract_occurrences(
    package: &str,
    module: &[String],
    base_cfg: &CfgPredicate,
    file_path: &std::path::Path,
    ast: &syn::File,
) -> Occurrences {
    #[derive(Default)]
    struct V {
        package: String,
        file_path: std::path::PathBuf,

        mod_stack: Vec<String>,

        // impl / trait context
        impl_self_ty: Option<String>,
        impl_trait_ty: Option<String>,
        in_trait: Option<String>,

        // current enclosing function
        current_fn: Option<String>,
        current_fn_is_public: Option<bool>,

        // > 0 while inside any `{ }` block (fn bodies, const blocks, ...)
        block_depth: usize,

        // effective cfg: the file's, then one entry per node
        // carrying its own `#[cfg]`
        base_cfg: CfgPredicate,
        cfg_stack: Vec<CfgPredicate>,

        // innermost item: ids of the items being visited
        item_stack: Vec<String>,
        // Namespace of the position being visited, innermost last
        ns_stack: Vec<Namespace>,
        // Local bindings in scope (params, `let`, patterns,
        // closures, `for`), innermost last — a single identifier naming one is
        // a value, not a reference to an item
        locals: Vec<Vec<String>>,
        // Generating macro per item on the stack (inherited by nested items)
        generated_stack: Vec<Option<String>>,
        pending_generated: Option<String>,
        // Calls whose arguments are being visited, innermost last
        arg_stack: Vec<ArgOf>,
        // `match` expressions being visited (indexes into out.matches), innermost last
        match_stack: Vec<usize>,
        // (match, arm) whose pattern is being visited
        arm_pattern: Option<(usize, usize)>,
        // > 0 while inside a pattern
        pattern_depth: usize,
        // the next path visited is a struct literal's own path
        struct_path: bool,

        out: Occurrences,
    }

    impl V {
        fn location(&self, span: Span) -> FileLocation {
            span_to_location(&self.file_path, span)
        }

        fn cfg(&self) -> CfgPredicate {
            self.cfg_stack.last().cloned().unwrap_or_else(|| self.base_cfg.clone())
        }

        fn item(&self) -> Option<String> {
            self.item_stack.last().cloned()
        }

        fn module_key(&self) -> String {
            join_key(&self.package, &self.mod_stack)
        }

        /// Push the conjunction if `attrs` carry a `#[cfg]`; returns whether it did.
        fn push_cfg(&mut self, attrs: &[Attribute]) -> bool {
            let own = cfg_of_attrs(attrs);
            if own == CfgPredicate::True {
                return false;
            }
            let c = self.cfg().and(own);
            self.cfg_stack.push(c);
            true
        }

        fn pop_cfg(&mut self, pushed: bool) {
            if pushed {
                self.cfg_stack.pop();
            }
        }

        /// Record a declaration and make it the innermost item.
        fn declare(&mut self, kind: NodeKind, id: String, name: String, is_public: bool, span: Span) {
            let generated = self.pending_generated.take().or_else(|| self.generated_stack.last().cloned().flatten());
            self.out.items.push(ItemDecl {
                id: id.clone(),
                kind,
                name,
                package: self.package.clone(),
                module: self.module_key(),
                parent: self.item(),
                location: self.location(span),
                cfg: self.cfg(),
                is_public,
                generated: generated.clone(),
            });
            self.item_stack.push(id);
            self.generated_stack.push(generated);
        }

        /// Base for a new item id: the enclosing item or the module key.
        fn id_base(&self) -> String {
            self.item().unwrap_or_else(|| self.module_key())
        }

        /// Records a module-level or nested item; returns whether an id was pushed.
        fn declare_item(&mut self, i: &Item) -> bool {
            let base = self.id_base();
            match i {
                Item::Fn(f) => {
                    let name = f.sig.ident.to_string();
                    let id = match self.item() {
                        Some(parent) => format!("{parent}::{name}"),
                        None => fq_name(&self.package, &self.mod_stack, &FnKind::FreeFn, &name),
                    };
                    self.declare(NodeKind::Fn, id, name, vis_is_public(&f.vis), f.span());
                }
                Item::Struct(s) => self.declare(NodeKind::Struct, format!("{base}::{}", s.ident), s.ident.to_string(), vis_is_public(&s.vis), s.span()),
                Item::Enum(e) => self.declare(NodeKind::Enum, format!("{base}::{}", e.ident), e.ident.to_string(), vis_is_public(&e.vis), e.span()),
                Item::Union(u) => self.declare(NodeKind::Union, format!("{base}::{}", u.ident), u.ident.to_string(), vis_is_public(&u.vis), u.span()),
                Item::Trait(t) => self.declare(NodeKind::Trait, format!("{base}::{}", t.ident), t.ident.to_string(), vis_is_public(&t.vis), t.span()),
                Item::Type(t) => self.declare(NodeKind::TypeAlias, format!("{base}::{}", t.ident), t.ident.to_string(), vis_is_public(&t.vis), t.span()),
                Item::Const(c) => self.declare(NodeKind::Const, format!("{base}::{}", c.ident), c.ident.to_string(), vis_is_public(&c.vis), c.span()),
                Item::Static(s) => self.declare(NodeKind::Static, format!("{base}::{}", s.ident), s.ident.to_string(), vis_is_public(&s.vis), s.span()),
                Item::Macro(m) => {
                    let Some(ident) = &m.ident else { return false };
                    if !m.mac.path.is_ident("macro_rules") {
                        return false;
                    }
                    let exported = m.attrs.iter().any(|a| a.path().is_ident("macro_export"));
                    self.declare(NodeKind::Macro, format!("{base}::{ident}"), ident.to_string(), exported, m.span());
                }
                Item::Impl(imp) => {
                    let self_ty = impl_self_ty_name(&imp.self_ty);
                    let trait_ty = imp.trait_.as_ref().map(|(_, p, _)| path_to_string(p));
                    let label = impl_label(&self_ty, trait_ty.as_deref());
                    self.declare(NodeKind::Impl, format!("{base}::{label}"), label, false, imp.span());
                }
                _ => return false,
            }
            true
        }

        /// The common part of a type record for the item just declared.
        fn type_base(&self, kind: TypeKind, name: String, is_public: bool, generics: &syn::Generics, attrs: &[Attribute], span: Span) -> TypeDecl {
            TypeDecl {
                id: self.item().unwrap_or_default(),
                kind,
                name,
                package: self.package.clone(),
                module: self.module_key(),
                location: self.location(span),
                cfg: self.cfg(),
                is_public,
                generics: generic_params(generics),
                fields: Vec::new(),
                variants: Vec::new(),
                supertraits: Vec::new(),
                trait_methods: Vec::new(),
                assoc_types: Vec::new(),
                assoc_consts: Vec::new(),
                aliased: None,
                derives: derive_placeholders(attrs),
                attrs: attr_paths(attrs),
                impls: Vec::new(),
            }
        }

        /// The type / impl record for the item `declare_item`
        /// just pushed. Mentions are placeholders until `resolve_types`.
        fn record_type(&mut self, i: &Item) {
            match i {
                Item::Struct(s) => {
                    let mut t = self.type_base(TypeKind::Struct, s.ident.to_string(), vis_is_public(&s.vis), &s.generics, &s.attrs, s.span());
                    t.fields = field_decls(s.fields.iter(), &t.cfg);
                    self.out.types.push(t);
                }
                Item::Union(u) => {
                    let mut t = self.type_base(TypeKind::Union, u.ident.to_string(), vis_is_public(&u.vis), &u.generics, &u.attrs, u.span());
                    t.fields = field_decls(u.fields.named.iter(), &t.cfg);
                    self.out.types.push(t);
                }
                Item::Enum(e) => {
                    let mut t = self.type_base(TypeKind::Enum, e.ident.to_string(), vis_is_public(&e.vis), &e.generics, &e.attrs, e.span());
                    let owner_cfg = t.cfg.clone();
                    t.variants = e
                        .variants
                        .iter()
                        .map(|v| {
                            let cfg = owner_cfg.clone().and(cfg_of_attrs(&v.attrs));
                            VariantDecl {
                                name: v.ident.to_string(),
                                fields: field_decls(v.fields.iter(), &cfg),
                                discriminant: v.discriminant.as_ref().map(|(_, x)| x.to_token_stream().to_string()),
                                cfg,
                                location: Some(self.location(v.ident.span())),
                                attrs: v
                                    .attrs
                                    .iter()
                                    .filter(|a| !a.path().is_ident("doc"))
                                    .map(|a| {
                                        let args = match &a.meta {
                                            syn::Meta::Path(_) => String::new(),
                                            syn::Meta::List(l) => l.tokens.to_string(),
                                            syn::Meta::NameValue(nv) => nv.value.to_token_stream().to_string(),
                                        };
                                        (path_to_string(a.path()), args)
                                    })
                                    .collect(),
                            }
                        })
                        .collect();
                    self.out.types.push(t);
                }
                Item::Trait(tr) => {
                    let mut t = self.type_base(TypeKind::Trait, tr.ident.to_string(), vis_is_public(&tr.vis), &tr.generics, &tr.attrs, tr.span());
                    t.supertraits = bounds_of(tr.supertraits.iter());
                    let kind = FnKind::TraitMethod { trait_name: tr.ident.to_string() };
                    for it in &tr.items {
                        match it {
                            syn::TraitItem::Fn(f) => t.trait_methods.push(MethodSig {
                                id: fq_name(&self.package, &self.mod_stack, &kind, &f.sig.ident.to_string()),
                                name: f.sig.ident.to_string(),
                                has_default: f.default.is_some(),
                                signature: f.sig.to_token_stream().to_string(),
                            }),
                            syn::TraitItem::Type(a) => t.assoc_types.push(a.ident.to_string()),
                            syn::TraitItem::Const(c) => t.assoc_consts.push(c.ident.to_string()),
                            _ => {}
                        }
                    }
                    self.out.types.push(t);
                }
                Item::Type(a) => {
                    let mut t = self.type_base(TypeKind::TypeAlias, a.ident.to_string(), vis_is_public(&a.vis), &a.generics, &a.attrs, a.span());
                    t.aliased = Some(type_ref(&a.ty));
                    self.out.types.push(t);
                }
                Item::Impl(imp) => {
                    let self_ty = impl_self_ty_name(&imp.self_ty);
                    let trait_ty = imp.trait_.as_ref().map(|(_, p, _)| path_to_string(p));
                    let kind = FnKind::ImplMethod { self_ty, trait_ty };
                    let mut d = ImplDecl {
                        id: self.item().unwrap_or_default(),
                        package: self.package.clone(),
                        module: self.module_key(),
                        location: self.location(imp.span()),
                        cfg: self.cfg(),
                        generics: generic_params(&imp.generics),
                        self_ty: type_ref(&imp.self_ty),
                        trait_: imp.trait_.as_ref().map(|(_, p, _)| path_ref(p)),
                        methods: Vec::new(),
                        assoc_types: Vec::new(),
                        assoc_consts: Vec::new(),
                    };
                    for it in &imp.items {
                        match it {
                            syn::ImplItem::Fn(m) => d.methods.push(fq_name(&self.package, &self.mod_stack, &kind, &m.sig.ident.to_string())),
                            syn::ImplItem::Type(a) => d.assoc_types.push((a.ident.to_string(), type_ref(&a.ty))),
                            syn::ImplItem::Const(c) => d.assoc_consts.push(c.ident.to_string()),
                            _ => {}
                        }
                    }
                    self.out.impls.push(d);
                }
                _ => {}
            }
        }

        fn ns(&self) -> Namespace {
            self.ns_stack.last().copied().unwrap_or(Namespace::Any)
        }

        fn is_local(&self, name: &str) -> bool {
            self.locals.iter().rev().any(|s| s.iter().any(|n| n == name))
        }

        /// Names a pattern binds as locals (lowercase-initial: unit variants
        /// and consts are written in upper case and stay item references).
        fn local_names(pat: &syn::Pat) -> Vec<String> {
            struct Idents(Vec<String>);
            impl<'a> Visit<'a> for Idents {
                fn visit_pat_ident(&mut self, p: &'a syn::PatIdent) {
                    let s = p.ident.to_string();
                    if s.chars().next().is_some_and(|c| c.is_lowercase() || c == '_') {
                        self.0.push(s);
                    }
                    syn::visit::visit_pat_ident(self, p);
                }
            }
            let mut v = Idents(Vec::new());
            v.visit_pat(pat);
            v.0
        }

        fn bind_locals(&mut self, names: Vec<String>) {
            if let Some(scope) = self.locals.last_mut() {
                scope.extend(names);
            }
        }

        fn param_names(sig: &syn::Signature) -> Vec<String> {
            let mut v = vec!["self".to_string()];
            for a in &sig.inputs {
                if let syn::FnArg::Typed(pt) = a {
                    v.extend(Self::local_names(&pt.pat));
                }
            }
            v
        }

        fn enter_fn(&mut self, fq: String, is_pub: bool) -> (Option<String>, Option<bool>) {
            let prev = (self.current_fn.take(), self.current_fn_is_public.take());
            self.current_fn = Some(fq);
            self.current_fn_is_public = Some(is_pub);
            prev
        }

        fn leave_fn(&mut self, prev: (Option<String>, Option<bool>)) {
            self.current_fn = prev.0;
            self.current_fn_is_public = prev.1;
        }

        /// An invocation klepto expanded is still an occurrence; the
        /// first produced node carries its path and span.
        fn record_invoked(&mut self, attrs: &[Attribute]) {
            if let Some((path, span)) = crate::resolve::expand::invoked_by(attrs) {
                let segs: Vec<String> = path.split("::").map(str::to_string).collect();
                let cfg = self.cfg();
                self.out.macros_inv.push(MacroInvocation {
                    covered: false,
                    name: segs.last().cloned().unwrap_or_else(|| "<macro>".into()),
                    module_path: self.mod_stack.clone(),
                    path: Some(segs.join("::")),
                    location: self.location(span),
                    enclosing_fn: self.current_fn.clone(),
                    enclosing_public: self.current_fn_is_public,
                    enclosing_item: self.item(),
                    resolution: None,
                    cfg,
                    tokens: String::new(),
                    token_paths: Vec::new(),
                });
            }
        }

        fn push_literal(&mut self, value: String, span: Span, in_macro: Option<String>) {
            self.out.literals.push(LiteralOccurrence {
                value,
                module_path: self.mod_stack.clone(),
                location: self.location(span),
                enclosing_fn: self.current_fn.clone(),
                enclosing_item: self.item(),
                arg_of: self.arg_stack.last().cloned(),
                in_macro,
                cfg: self.cfg(),
            });
        }

        /// String literals inside an unexpanded macro's tokens.
        fn literals_in_tokens(&mut self, ts: &TokenStream, name: &str) {
            for tt in ts.clone() {
                match tt {
                    TokenTree::Group(g) => self.literals_in_tokens(&g.stream(), name),
                    TokenTree::Literal(l) => {
                        if let Ok(s) = syn::parse_str::<syn::LitStr>(&l.to_string()) {
                            self.push_literal(s.value(), l.span(), Some(name.to_string()));
                        }
                    }
                    _ => {}
                }
            }
        }

        fn record_macro(&mut self, mac: &syn::Macro, span: Span) {
            let segs: Vec<String> = mac.path.segments.iter().map(|s| s.ident.to_string()).collect();
            let cfg = self.cfg();
            self.out.macros_inv.push(MacroInvocation {
                name: segs.last().cloned().unwrap_or_else(|| "<macro>".into()),
                covered: false,
                module_path: self.mod_stack.clone(),
                path: if segs.is_empty() { None } else { Some(segs.join("::")) },
                location: self.location(span),
                enclosing_fn: self.current_fn.clone(),
                enclosing_public: self.current_fn_is_public,
                enclosing_item: self.item(),
                resolution: None,
                cfg,
                tokens: mac.tokens.to_string(),
                token_paths: paths_in_tokens(&mac.tokens),
            });
            let name = segs.join("::");
            self.literals_in_tokens(&mac.tokens, &name);
        }

        fn push_attr(&mut self, path: String, args: &TokenStream, predicate: Option<CfgPredicate>, cfg: CfgPredicate, span: Span) {
            self.out.attrs.push(AttrOccurrence {
                path,
                arg_paths: paths_in_tokens(args),
                args: args.to_string(),
                module_path: self.mod_stack.clone(),
                location: self.location(span),
                enclosing_fn: self.current_fn.clone(),
                enclosing_public: self.current_fn_is_public,
                enclosing_item: self.item(),
                resolution: None,
                arg_resolutions: Vec::new(),
                predicate,
                cfg,
            });
        }

        /// `#[cfg_attr(pred, a, b)]` → attributes `a`, `b` under `pred`.
        fn expand_cfg_attr(&mut self, tokens: &TokenStream, enclosing: CfgPredicate, span: Span) {
            let chunks = split_top_level_commas(tokens);
            let Some((pred_tokens, specs)) = chunks.split_first() else { return };
            let pred = CfgPredicate::parse_tokens(&pred_tokens.iter().cloned().collect::<TokenStream>());
            let cfg = enclosing.and(pred);
            for spec in specs {
                self.record_attr_spec(spec, cfg.clone(), span);
            }
        }

        /// One `path(args)` / `path = value` / `path` inside a `cfg_attr`.
        fn record_attr_spec(&mut self, spec: &[TokenTree], cfg: CfgPredicate, span: Span) {
            let mut i = 0;
            let mut segs: Vec<String> = Vec::new();
            while let Some(TokenTree::Ident(id)) = spec.get(i) {
                segs.push(id.to_string());
                i += 1;
                match (spec.get(i), spec.get(i + 1)) {
                    (Some(TokenTree::Punct(a)), Some(TokenTree::Punct(b))) if a.as_char() == ':' && b.as_char() == ':' => i += 2,
                    _ => break,
                }
            }
            if segs.is_empty() {
                return;
            }
            let path = segs.join("::");
            let args: TokenStream = match spec.get(i) {
                Some(TokenTree::Group(g)) => g.stream(),
                Some(TokenTree::Punct(p)) if p.as_char() == '=' => spec[i + 1..].iter().cloned().collect(),
                _ => TokenStream::new(),
            };
            if path == "cfg_attr" {
                self.expand_cfg_attr(&args, cfg, span);
                return;
            }
            let predicate = if path == "cfg" { Some(CfgPredicate::parse_tokens(&args)) } else { None };
            self.push_attr(path, &args, predicate, cfg, span);
        }

        fn visit_attrs(&mut self, attrs: &[Attribute]) {
            for a in attrs {
                self.visit_attribute(a);
            }
        }
    }

    impl<'ast> Visit<'ast> for V {
        // ---- cfg stack and item stack ----

        fn visit_item(&mut self, i: &'ast Item) {
            let pushed = self.push_cfg(item_attrs(i));
            self.record_invoked(item_attrs(i));
            self.pending_generated = crate::resolve::expand::generated_by(item_attrs(i));
            let declared = self.declare_item(i);
            self.pending_generated = None;
            if declared {
                self.record_type(i);
            }
            syn::visit::visit_item(self, i);
            if declared {
                self.item_stack.pop();
                self.generated_stack.pop();
            }
            self.pop_cfg(pushed);
        }

        fn visit_impl_item(&mut self, i: &'ast syn::ImplItem) {
            let pushed = self.push_cfg(impl_item_attrs(i));
            self.record_invoked(impl_item_attrs(i));
            self.pending_generated = crate::resolve::expand::generated_by(impl_item_attrs(i));
            let declared = if let syn::ImplItem::Fn(m) = i {
                let self_ty = self.impl_self_ty.clone().unwrap_or_else(|| "<impl>".into());
                let kind = FnKind::ImplMethod { self_ty, trait_ty: self.impl_trait_ty.clone() };
                let name = m.sig.ident.to_string();
                let id = fq_name(&self.package, &self.mod_stack, &kind, &name);
                self.declare(NodeKind::Method, id, name, vis_is_public(&m.vis), m.span());
                true
            } else {
                false
            };
            syn::visit::visit_impl_item(self, i);
            if declared {
                self.item_stack.pop();
                self.generated_stack.pop();
            }
            self.pop_cfg(pushed);
        }

        fn visit_trait_item(&mut self, i: &'ast syn::TraitItem) {
            let pushed = self.push_cfg(trait_item_attrs(i));
            self.record_invoked(trait_item_attrs(i));
            self.pending_generated = crate::resolve::expand::generated_by(trait_item_attrs(i));
            let declared = if let syn::TraitItem::Fn(tf) = i {
                let tr = self.in_trait.clone().unwrap_or_else(|| "<trait>".into());
                let name = tf.sig.ident.to_string();
                let id = fq_name(&self.package, &self.mod_stack, &FnKind::TraitMethod { trait_name: tr }, &name);
                self.declare(NodeKind::Method, id, name, true, tf.span());
                true
            } else {
                false
            };
            syn::visit::visit_trait_item(self, i);
            if declared {
                self.item_stack.pop();
                self.generated_stack.pop();
            }
            self.pop_cfg(pushed);
        }

        fn visit_stmt(&mut self, s: &'ast syn::Stmt) {
            let pushed = self.push_cfg(stmt_attrs(s));
            self.record_invoked(stmt_attrs(s));
            syn::visit::visit_stmt(self, s);
            self.pop_cfg(pushed);
        }

        fn visit_expr(&mut self, e: &'ast syn::Expr) {
            self.record_invoked(expr_attrs(e));
            let pushed = self.push_cfg(expr_attrs(e));
            syn::visit::visit_expr(self, e);
            self.pop_cfg(pushed);
        }

        fn visit_expr_match(&mut self, m: &'ast syn::ExprMatch) {
            let mut scrutinee: String = m.expr.to_token_stream().to_string().chars().take(160).collect();
            if scrutinee.len() >= 160 {
                scrutinee.push('…');
            }
            self.out.matches.push(MatchOccurrence {
                module_path: self.mod_stack.clone(),
                location: self.location(m.span()),
                enclosing_fn: self.current_fn.clone(),
                enclosing_item: self.item(),
                scrutinee,
                arms: Vec::new(),
                cfg: self.cfg(),
            });
            self.match_stack.push(self.out.matches.len() - 1);
            syn::visit::visit_expr_match(self, m);
            self.match_stack.pop();
        }

        fn visit_arm(&mut self, a: &'ast syn::Arm) {
            let pushed = self.push_cfg(&a.attrs);
            self.locals.push(Self::local_names(&a.pat));
            // as `syn::visit::visit_arm`, with the pattern's paths recorded on the arm
            for attr in &a.attrs {
                self.visit_attribute(attr);
            }
            let outer = self.arm_pattern.take();
            if let Some(&mi) = self.match_stack.last() {
                let catch_all = match &a.pat {
                    syn::Pat::Wild(_) => true,
                    syn::Pat::Ident(pi) => pi.subpat.is_none() && pi.ident.to_string().chars().next().is_some_and(|c| c.is_lowercase() || c == '_'),
                    _ => false,
                };
                let location = self.location(a.pat.span());
                self.out.matches[mi].arms.push(MatchArm { location, catch_all, paths: Vec::new() });
                self.arm_pattern = Some((mi, self.out.matches[mi].arms.len() - 1));
            }
            self.visit_pat(&a.pat);
            self.arm_pattern = outer;
            if let Some((_, guard)) = &a.guard {
                self.visit_expr(guard);
            }
            self.visit_expr(&a.body);
            self.locals.pop();
            self.pop_cfg(pushed);
        }

        fn visit_field(&mut self, f: &'ast syn::Field) {
            let pushed = self.push_cfg(&f.attrs);
            syn::visit::visit_field(self, f);
            self.pop_cfg(pushed);
        }

        // A struct literal's field value and a struct pattern's field can
        // carry `#[cfg]` too (clap's `#[cfg(debug_assertions)] valid_subcommands: …`)
        fn visit_field_value(&mut self, f: &'ast syn::FieldValue) {
            let pushed = self.push_cfg(&f.attrs);
            syn::visit::visit_field_value(self, f);
            self.pop_cfg(pushed);
        }

        fn visit_field_pat(&mut self, f: &'ast syn::FieldPat) {
            let pushed = self.push_cfg(&f.attrs);
            syn::visit::visit_field_pat(self, f);
            self.pop_cfg(pushed);
        }

        fn visit_variant(&mut self, v: &'ast syn::Variant) {
            let pushed = self.push_cfg(&v.attrs);
            syn::visit::visit_variant(self, v);
            self.pop_cfg(pushed);
        }

        fn visit_fn_arg(&mut self, a: &'ast syn::FnArg) {
            let pushed = self.push_cfg(fn_arg_attrs(a));
            syn::visit::visit_fn_arg(self, a);
            self.pop_cfg(pushed);
        }

        // ---- structure ----

        fn visit_item_mod(&mut self, i: &'ast syn::ItemMod) {
            self.visit_attrs(&i.attrs);
            if let Some((_, items)) = &i.content {
                self.mod_stack.push(i.ident.to_string());
                for it in items {
                    self.visit_item(it);
                }
                self.mod_stack.pop();
            }
        }

        fn visit_item_trait(&mut self, i: &'ast syn::ItemTrait) {
            let prev = self.in_trait.take();
            self.in_trait = Some(i.ident.to_string());
            syn::visit::visit_item_trait(self, i);
            self.in_trait = prev;
        }

        fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
            let prev_self = self.impl_self_ty.take();
            let prev_trait = self.impl_trait_ty.take();

            self.impl_self_ty = Some(impl_self_ty_name(&i.self_ty));
            self.impl_trait_ty = i.trait_.as_ref().map(|(_, p, _)| path_to_string(p));

            syn::visit::visit_item_impl(self, i);

            self.impl_self_ty = prev_self;
            self.impl_trait_ty = prev_trait;
        }

        fn visit_item_fn(&mut self, i: &'ast syn::ItemFn) {
            let is_pub = self.in_trait.is_some() || vis_is_public(&i.vis);
            // the id was pushed by visit_item for nested fns
            let fq = self.item().unwrap_or_else(|| fq_name(&self.package, &self.mod_stack, &FnKind::FreeFn, &i.sig.ident.to_string()));
            let prev = self.enter_fn(fq, is_pub);
            self.locals.push(Self::param_names(&i.sig));
            syn::visit::visit_item_fn(self, i);
            self.locals.pop();
            self.leave_fn(prev);
        }

        fn visit_impl_item_fn(&mut self, i: &'ast syn::ImplItemFn) {
            let fq = self.item().unwrap_or_else(|| {
                let self_ty = self.impl_self_ty.clone().unwrap_or_else(|| "<impl>".into());
                let kind = FnKind::ImplMethod { self_ty, trait_ty: self.impl_trait_ty.clone() };
                fq_name(&self.package, &self.mod_stack, &kind, &i.sig.ident.to_string())
            });
            let prev = self.enter_fn(fq, vis_is_public(&i.vis));
            self.locals.push(Self::param_names(&i.sig));
            syn::visit::visit_impl_item_fn(self, i);
            self.locals.pop();
            self.leave_fn(prev);
        }

        fn visit_trait_item_fn(&mut self, i: &'ast syn::TraitItemFn) {
            let fq = self.item().unwrap_or_else(|| {
                let tr = self.in_trait.clone().unwrap_or_else(|| "<trait>".into());
                fq_name(&self.package, &self.mod_stack, &FnKind::TraitMethod { trait_name: tr }, &i.sig.ident.to_string())
            });
            let prev = self.enter_fn(fq, true);
            self.locals.push(Self::param_names(&i.sig));
            syn::visit::visit_trait_item_fn(self, i);
            self.locals.pop();
            self.leave_fn(prev);
        }

        // ---- namespaces ----

        fn visit_type(&mut self, t: &'ast syn::Type) {
            self.ns_stack.push(Namespace::Type);
            syn::visit::visit_type(self, t);
            self.ns_stack.pop();
        }

        fn visit_expr_path(&mut self, e: &'ast syn::ExprPath) {
            self.ns_stack.push(Namespace::Value);
            syn::visit::visit_expr_path(self, e);
            self.ns_stack.pop();
        }

        fn visit_expr_struct(&mut self, s: &'ast syn::ExprStruct) {
            for a in &s.attrs {
                self.visit_attribute(a);
            }
            if let Some(q) = &s.qself {
                self.visit_qself(q);
            }
            self.ns_stack.push(Namespace::Type);
            self.struct_path = true;
            self.visit_path(&s.path);
            self.struct_path = false;
            self.ns_stack.pop();
            for f in &s.fields {
                self.visit_field_value(f);
            }
            if let Some(rest) = &s.rest {
                self.visit_expr(rest);
            }
        }

        fn visit_pat(&mut self, p: &'ast syn::Pat) {
            self.pattern_depth += 1;
            syn::visit::visit_pat(self, p);
            self.pattern_depth -= 1;
        }

        fn visit_pat_tuple_struct(&mut self, p: &'ast syn::PatTupleStruct) {
            for a in &p.attrs {
                self.visit_attribute(a);
            }
            if let Some(q) = &p.qself {
                self.visit_qself(q);
            }
            self.ns_stack.push(Namespace::Value);
            self.visit_path(&p.path);
            self.ns_stack.pop();
            for e in &p.elems {
                self.visit_pat(e);
            }
        }

        fn visit_pat_struct(&mut self, p: &'ast syn::PatStruct) {
            for a in &p.attrs {
                self.visit_attribute(a);
            }
            if let Some(q) = &p.qself {
                self.visit_qself(q);
            }
            self.ns_stack.push(Namespace::Type);
            self.visit_path(&p.path);
            self.ns_stack.pop();
            for f in &p.fields {
                self.visit_field_pat(f);
            }
            if let Some(rest) = &p.rest {
                self.visit_pat_rest(rest);
            }
        }

        fn visit_block(&mut self, b: &'ast syn::Block) {
            self.block_depth += 1;
            self.locals.push(Vec::new());
            syn::visit::visit_block(self, b);
            self.locals.pop();
            self.block_depth -= 1;
        }

        fn visit_local(&mut self, l: &'ast syn::Local) {
            syn::visit::visit_local(self, l);
            let names = Self::local_names(&l.pat);
            self.bind_locals(names);
        }

        fn visit_expr_let(&mut self, l: &'ast syn::ExprLet) {
            syn::visit::visit_expr_let(self, l);
            let names = Self::local_names(&l.pat);
            self.bind_locals(names);
        }

        fn visit_expr_closure(&mut self, c: &'ast syn::ExprClosure) {
            let names: Vec<String> = c.inputs.iter().flat_map(Self::local_names).collect();
            self.locals.push(names);
            syn::visit::visit_expr_closure(self, c);
            self.locals.pop();
        }

        fn visit_expr_for_loop(&mut self, f: &'ast syn::ExprForLoop) {
            let pushed = self.push_cfg(&f.attrs);
            self.locals.push(Self::local_names(&f.pat));
            syn::visit::visit_expr_for_loop(self, f);
            self.locals.pop();
            self.pop_cfg(pushed);
        }

        fn visit_item_use(&mut self, u: &'ast syn::ItemUse) {
            self.visit_attrs(&u.attrs);
            // Module-level `use` items are extracted by `extract_imports`;
            // only the ones nested inside a block belong here.
            if self.block_depth > 0 {
                let cfg = self.cfg();
                let start = self.out.body_imports.len();
                imports_from_item_use(
                    &self.file_path,
                    &self.mod_stack,
                    self.current_fn.as_deref(),
                    &cfg,
                    u,
                    &mut self.out.body_imports,
                );
                let item = self.item();
                for imp in &mut self.out.body_imports[start..] {
                    imp.enclosing_item = item.clone();
                }
            }
        }

        fn visit_item_extern_crate(&mut self, i: &'ast syn::ItemExternCrate) {
            self.visit_attrs(&i.attrs);
            let cfg = self.cfg();
            self.out.extern_crates.push(ExternCrateOccurrence {
                name: i.ident.to_string(),
                alias: i.rename.as_ref().map(|(_, id)| id.to_string()),
                module_path: self.mod_stack.clone(),
                location: self.location(i.span()),
                cfg,
            });
        }

        fn visit_attribute(&mut self, a: &'ast Attribute) {
            let path = path_to_string(a.path());
            if path == "doc" {
                return;
            }
            let cfg = self.cfg();
            if path == "cfg_attr" {
                if let syn::Meta::List(l) = &a.meta {
                    self.expand_cfg_attr(&l.tokens, cfg, a.span());
                }
                return;
            }
            let args: TokenStream = match &a.meta {
                syn::Meta::Path(_) => TokenStream::new(),
                syn::Meta::List(l) => l.tokens.clone(),
                syn::Meta::NameValue(nv) => nv.value.to_token_stream(),
            };
            // `#[cfg(..)]` keeps its parsed predicate
            let predicate = if path == "cfg" { Some(CfgPredicate::parse_tokens(&args)) } else { None };
            self.push_attr(path, &args, predicate, cfg, a.span());
            // No delegation: the attribute path is recorded here, not as a PathOccurrence.
        }

        fn visit_item_macro(&mut self, i: &'ast syn::ItemMacro) {
            self.visit_attrs(&i.attrs);
            if i.mac.path.is_ident("macro_rules") {
                self.out.macros_def.push(MacroDef {
                    name: i.ident.as_ref().map(|x| x.to_string()).unwrap_or_else(|| "<macro>".into()),
                    module_path: self.mod_stack.clone(),
                    location: self.location(i.span()),
                });
            } else {
                self.record_macro(&i.mac, i.span());
            }
            // No delegation: the macro path must not also become a PathOccurrence.
        }

        fn visit_expr_macro(&mut self, i: &'ast syn::ExprMacro) {
            self.visit_attrs(&i.attrs);
            self.record_macro(&i.mac, i.span());
        }

        fn visit_stmt_macro(&mut self, i: &'ast syn::StmtMacro) {
            self.visit_attrs(&i.attrs);
            self.record_macro(&i.mac, i.span());
        }

        fn visit_path(&mut self, p: &'ast syn::Path) {
            let s = path_string(p);
            let occ = PathOccurrence {
                path: s.clone(),
                covered: false,
                module_path: self.mod_stack.clone(),
                location: self.location(p.span()),
                enclosing_fn: self.current_fn.clone(),
                enclosing_public: self.current_fn_is_public,
                enclosing_item: self.item(),
                resolution: None,
                cfg: self.cfg(),
                ns: self.ns(),
                arg_of: self.arg_stack.last().cloned(),
                role: if std::mem::take(&mut self.struct_path) {
                    PathRole::StructLiteral
                } else if self.pattern_depth > 0 {
                    PathRole::Pattern
                } else {
                    PathRole::Other
                },
            };
            if let Some((mi, ai)) = self.arm_pattern {
                self.out.matches[mi].arms[ai].paths.push(ArmPath { path: s.clone(), location: occ.location.clone(), resolution: None });
            }

            if s.contains("::") || matches!(s.as_str(), "std" | "core" | "alloc") {
                self.out.paths.push(occ);
            } else if !matches!(s.as_str(), "self" | "super" | "crate") && !self.is_local(&s) {
                // single identifier: kept only if the resolver finds an item for it;
                // a local binding of that name is a value, never an item reference
                self.out.single_paths.push(occ);
            }

            syn::visit::visit_path(self, p);
        }

        fn visit_lit_str(&mut self, s: &'ast syn::LitStr) {
            self.push_literal(s.value(), s.span(), None);
        }

        fn visit_expr_method_call(&mut self, m: &'ast syn::ExprMethodCall) {
            let cfg = self.cfg();
            self.out.calls.push(CallOccurrence {
                callee: m.method.to_string(),
                covered: false,
                module_path: self.mod_stack.clone(),
                location: self.location(m.span()),
                enclosing_fn: self.current_fn.clone(),
                enclosing_public: self.current_fn_is_public,
                enclosing_item: self.item(),
                is_method: true,
                resolution: None,
                receiver_type: None,
                cfg,
            });
            // as `syn::visit::visit_expr_method_call`, the arguments under this call
            for a in &m.attrs {
                self.visit_attribute(a);
            }
            self.visit_expr(&m.receiver);
            self.visit_ident(&m.method);
            if let Some(t) = &m.turbofish {
                self.visit_angle_bracketed_generic_arguments(t);
            }
            self.arg_stack.push(ArgOf { callee: m.method.to_string(), is_method: true, location: self.location(m.span()), resolution: None });
            for a in &m.args {
                self.visit_expr(a);
            }
            self.arg_stack.pop();
        }

        fn visit_expr_call(&mut self, c: &'ast syn::ExprCall) {
            let cfg = self.cfg();
            self.out.calls.push(CallOccurrence {
                callee: c.func.to_token_stream().to_string(),
                covered: false,
                module_path: self.mod_stack.clone(),
                location: self.location(c.span()),
                enclosing_fn: self.current_fn.clone(),
                enclosing_public: self.current_fn_is_public,
                enclosing_item: self.item(),
                is_method: false,
                resolution: None,
                receiver_type: None,
                cfg,
            });
            // as `syn::visit::visit_expr_call`, the arguments under this call
            for a in &c.attrs {
                self.visit_attribute(a);
            }
            self.visit_expr(&c.func);
            let callee = match &*c.func {
                syn::Expr::Path(p) => path_string(&p.path),
                other => other.to_token_stream().to_string(),
            };
            self.arg_stack.push(ArgOf { callee, is_method: false, location: self.location(c.span()), resolution: None });
            for a in &c.args {
                self.visit_expr(a);
            }
            self.arg_stack.pop();
        }
    }

    let mut v = V {
        package: package.to_string(),
        file_path: file_path.to_path_buf(),
        mod_stack: module.to_vec(),
        base_cfg: base_cfg.clone(),
        ..Default::default()
    };
    v.visit_file(ast);
    v.out
}
