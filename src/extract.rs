use crate::model::*;
use proc_macro2::Span;
use quote::ToTokens;
use syn::{Attribute, File, Item, Visibility, spanned::Spanned, visit::Visit};

fn span_to_location(path: &std::path::Path, span: Span) -> FileLocation {
    #[cfg(feature = "span-locations")]
    {
        let start = span.start();
        return FileLocation {
            path: path.to_path_buf(),
            line: Some(start.line as u32),
            column: Some(start.column as u32),
        };
    }
    #[cfg(not(feature = "span-locations"))]
    {
        let _ = span;
        FileLocation {
            path: path.to_path_buf(),
            line: None,
            column: None,
        }
    }
}

fn vis_is_public(vis: &Visibility) -> bool {
    matches!(vis, Visibility::Public(_))
}

fn has_docs(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| a.path().is_ident("doc"))
}

fn attr_paths(attrs: &[Attribute]) -> Vec<String> {
    attrs
        .iter()
        .map(|a| {
            a.path()
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect::<Vec<_>>()
                .join("::")
        })
        .collect()
}

fn path_to_string(p: &syn::Path) -> String {
    p.segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

fn type_to_string(t: &syn::Type) -> String {
    t.to_token_stream().to_string()
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

fn fq_name(crate_name: &str, module_path: &[String], kind: &FnKind, name: &str) -> String {
    let mut parts = Vec::new();
    parts.push(crate_name.to_string());
    parts.extend(module_path.iter().cloned());
    match kind {
        FnKind::FreeFn => parts.push(name.to_string()),
        FnKind::TraitMethod { trait_name } => {
            parts.push(trait_name.clone());
            parts.push(name.to_string());
        }
        FnKind::ImplMethod { self_ty, .. } => {
            parts.push(self_ty.clone());
            parts.push(name.to_string());
        }
    }
    parts.join("::")
}

pub fn crate_is_no_std(ast: &File) -> bool {
    ast.attrs.iter().any(|a| a.path().is_ident("no_std"))
}

pub fn extract_public_surface(file_path: &std::path::Path, ast: &File) -> Vec<ExportedSymbol> {
    // best-effort: pub use trees -> ExportedSymbol
    let mut out = Vec::new();

    fn walk(
        file_path: &std::path::Path,
        tree: &syn::UseTree,
        mut prefix: Vec<String>,
        module_path: &[String],
        out: &mut Vec<ExportedSymbol>,
    ) {
        match tree {
            syn::UseTree::Path(p) => {
                prefix.push(p.ident.to_string());
                walk(file_path, &p.tree, prefix, module_path, out);
            }
            syn::UseTree::Group(g) => {
                for t in &g.items {
                    walk(file_path, t, prefix.clone(), module_path, out);
                }
            }
            syn::UseTree::Name(n) => {
                let mut segs = prefix;
                segs.push(n.ident.to_string());
                let src = segs.join("::");
                let exported_as = n.ident.to_string();
                out.push(ExportedSymbol {
                    exported_as,
                    source_path: src,
                    module_path: module_path.to_vec(),
                    location: span_to_location(file_path, n.span()),
                });
            }
            syn::UseTree::Rename(r) => {
                let mut segs = prefix;
                segs.push(r.ident.to_string());
                let src = segs.join("::");
                let exported_as = r.rename.to_string();
                out.push(ExportedSymbol {
                    exported_as,
                    source_path: src,
                    module_path: module_path.to_vec(),
                    location: span_to_location(file_path, r.span()),
                });
            }
            syn::UseTree::Glob(g) => {
                let mut segs = prefix;
                segs.push("*".to_string());
                out.push(ExportedSymbol {
                    exported_as: "*".to_string(),
                    source_path: segs.join("::"),
                    module_path: module_path.to_vec(),
                    location: span_to_location(file_path, g.span()),
                });
            }
            _ => {}
        }
    }

    // track module stack for correct module_path
    fn walk_items(
        file_path: &std::path::Path,
        items: &[Item],
        mod_stack: &mut Vec<String>,
        out: &mut Vec<ExportedSymbol>,
    ) {
        for it in items {
            match it {
                Item::Use(u) => {
                    if matches!(u.vis, Visibility::Public(_)) {
                        walk(file_path, &u.tree, Vec::new(), mod_stack, out);
                    }
                }
                Item::Mod(m) => {
                    if let Some((_, items)) = &m.content {
                        mod_stack.push(m.ident.to_string());
                        walk_items(file_path, items, mod_stack, out);
                        mod_stack.pop();
                    }
                }
                _ => {}
            }
        }
    }

    let mut ms = Vec::new();
    walk_items(file_path, &ast.items, &mut ms, &mut out);
    out
}

pub fn extract_imports_v1(file_path: &std::path::Path, ast: &File) -> Vec<StolenPathV1> {
    let mut out = Vec::new();

    fn emit(
        file_path: &std::path::Path,
        mut segs: Vec<String>,
        is_public_use: bool,
        kind: UseKind,
        span: Span,
        out: &mut Vec<StolenPathV1>,
    ) {
        if segs.is_empty() {
            return;
        }
        let root = segs.remove(0);
        let is_internal = matches!(root.as_str(), "crate" | "self" | "super");
        let full_path = if segs.is_empty() {
            root.clone()
        } else {
            format!("{}::{}", root, segs.join("::"))
        };
        out.push(StolenPathV1 {
            root,
            segments: segs,
            is_internal,
            is_public_use,
            kind,
            full_path,
            location: span_to_location(file_path, span),
        });
    }

    fn walk_tree(
        file_path: &std::path::Path,
        tree: &syn::UseTree,
        mut prefix: Vec<String>,
        is_public_use: bool,
        span: Span,
        out: &mut Vec<StolenPathV1>,
    ) {
        match tree {
            syn::UseTree::Path(p) => {
                prefix.push(p.ident.to_string());
                walk_tree(file_path, &p.tree, prefix, is_public_use, p.span(), out);
            }
            syn::UseTree::Group(g) => {
                for t in &g.items {
                    walk_tree(file_path, t, prefix.clone(), is_public_use, t.span(), out);
                }
            }
            syn::UseTree::Name(n) => {
                prefix.push(n.ident.to_string());
                emit(file_path, prefix, is_public_use, UseKind::Name, span, out);
            }
            syn::UseTree::Glob(g) => {
                prefix.push("*".to_string());
                emit(
                    file_path,
                    prefix,
                    is_public_use,
                    UseKind::Glob,
                    g.span(),
                    out,
                );
            }
            syn::UseTree::Rename(r) => {
                prefix.push(r.ident.to_string());
                emit(
                    file_path,
                    prefix,
                    is_public_use,
                    UseKind::Rename {
                        alias: r.rename.to_string(),
                    },
                    r.span(),
                    out,
                );
            }
            _ => {}
        }
    }

    for item in &ast.items {
        if let Item::Use(u) = item {
            let is_pub = matches!(u.vis, Visibility::Public(_));
            walk_tree(file_path, &u.tree, Vec::new(), is_pub, u.span(), &mut out);
        }
    }

    out
}
pub fn extract_imports(
    file_path: &std::path::Path,
    ast: &syn::File,
) -> Vec<crate::model::StolenPath> {
    use crate::model::{FileLocation, StolenPath, UseKind};

    fn span_to_location(path: &std::path::Path, span: proc_macro2::Span) -> FileLocation {
        #[cfg(feature = "span-locations")]
        {
            let start = span.start();
            return FileLocation {
                path: path.to_path_buf(),
                line: Some(start.line as u32),
                column: Some(start.column as u32),
            };
        }
        #[cfg(not(feature = "span-locations"))]
        {
            let _ = span;
            FileLocation {
                path: path.to_path_buf(),
                line: None,
                column: None,
            }
        }
    }

    fn emit(
        file_path: &std::path::Path,
        module_path: &[String],
        mut segs: Vec<String>,
        is_public_use: bool,
        is_absolute: bool,
        kind: UseKind,
        span: proc_macro2::Span,
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

        let full_path = if is_absolute {
            format!("::{}", base)
        } else {
            base
        };

        out.push(StolenPath {
            root,
            segments: segs,
            module_path: module_path.to_vec(),
            is_internal,
            is_public_use,
            kind,
            full_path,
            location: span_to_location(file_path, span),
            origin: None,                   // classified later in KleptoBuilder::parse()
            is_absolute: Some(is_absolute), // tracked here
        });
    }

    fn walk_tree(
        file_path: &std::path::Path,
        module_path: &[String],
        tree: &syn::UseTree,
        prefix: Vec<String>,
        is_public_use: bool,
        is_absolute: bool,
        // span: proc_macro2::Span,
        out: &mut Vec<StolenPath>,
    ) {
        match tree {
            syn::UseTree::Path(p) => {
                let mut next = prefix;
                next.push(p.ident.to_string());
                walk_tree(
                    file_path,
                    module_path,
                    &p.tree,
                    next,
                    is_public_use,
                    is_absolute,
                    // p.span(),
                    out,
                );
            }

            syn::UseTree::Group(g) => {
                for t in &g.items {
                    walk_tree(
                        file_path,
                        module_path,
                        t,
                        prefix.clone(),
                        is_public_use,
                        is_absolute,
                        // t.span(),
                        out,
                    );
                }
            }

            syn::UseTree::Name(n) => {
                // 👇 normalization: {self, X} means "import the module itself"
                if n.ident == "self" {
                    emit(
                        file_path,
                        module_path,
                        prefix,
                        is_public_use,
                        is_absolute,
                        UseKind::Name,
                        n.span(),
                        out,
                    );
                } else {
                    let mut segs = prefix;
                    segs.push(n.ident.to_string());
                    emit(
                        file_path,
                        module_path,
                        segs,
                        is_public_use,
                        is_absolute,
                        UseKind::Name,
                        n.span(),
                        out,
                    );
                }
            }

            syn::UseTree::Glob(g) => {
                let mut segs = prefix;
                segs.push("*".to_string());
                emit(
                    file_path,
                    module_path,
                    segs,
                    is_public_use,
                    is_absolute,
                    UseKind::Glob,
                    g.span(),
                    out,
                );
            }

            syn::UseTree::Rename(r) => {
                let mut segs = prefix;
                segs.push(r.ident.to_string());
                emit(
                    file_path,
                    module_path,
                    segs,
                    is_public_use,
                    is_absolute,
                    UseKind::Rename {
                        alias: r.rename.to_string(),
                    },
                    r.span(),
                    out,
                );
            }

            _ => {}
        }
    }

    fn walk_items(
        file_path: &std::path::Path,
        items: &[Item],
        mod_stack: &mut Vec<String>,
        out: &mut Vec<StolenPath>,
    ) {
        for item in items {
            match item {
                Item::Use(u) => {
                    let is_pub = matches!(u.vis, Visibility::Public(_));
                    let is_abs = u.leading_colon.is_some();
                    walk_tree(file_path, mod_stack, &u.tree, Vec::new(), is_pub, is_abs, out);
                }
                Item::Mod(m) => {
                    if let Some((_, inner)) = &m.content {
                        mod_stack.push(m.ident.to_string());
                        walk_items(file_path, inner, mod_stack, out);
                        mod_stack.pop();
                    }
                }
                _ => {}
            }
        }
    }

    let mut out = Vec::new();
    let mut mod_stack = Vec::new();
    walk_items(file_path, &ast.items, &mut mod_stack, &mut out);
//     let mut out = Vec::new();
// 
//     for item in &ast.items {
//         if let syn::Item::Use(u) = item {
//             let is_pub = matches!(u.vis, syn::Visibility::Public(_));
//             let is_absolute = u.leading_colon.is_some();
//             let empty_module_path: Vec<String> = Vec::new();
//             walk_tree(
//                 file_path,
//                 &empty_module_path,
//                 &u.tree,
//                 Vec::new(),
//                 is_pub,
//                 is_absolute,
//                 u.span(),
//                 &mut out,
//             );
//         }
//     }

    out
}

pub fn extract_functions(
    crate_name: &str,
    file_path: &std::path::Path,
    ast: &File,
) -> Vec<CapturedFn> {
    let mut out = Vec::new();
    let mut mod_stack: Vec<String> = Vec::new();

    fn walk_items(
        crate_name: &str,
        file_path: &std::path::Path,
        items: &[Item],
        mod_stack: &mut Vec<String>,
        out: &mut Vec<CapturedFn>,
    ) {
        for item in items {
            match item {
                Item::Fn(f) => {
                    let kind = FnKind::FreeFn;
                    let name = f.sig.ident.to_string();
                    let fq = super::extract::fq_name(crate_name, mod_stack, &kind, &name);

                    out.push(CapturedFn {
                        name,
                        fq_name: fq,
                        is_public: vis_is_public(&f.vis),
                        has_docs: has_docs(&f.attrs),

                        is_async: f.sig.asyncness.is_some(),
                        is_unsafe: f.sig.unsafety.is_some(),
                        is_const: f.sig.constness.is_some(),
                        is_generic: !f.sig.generics.params.is_empty(),

                        args: fn_args(&f.sig),
                        return_ty: fn_return(&f.sig),

                        kind,
                        module_path: mod_stack.clone(),
                        attrs: attr_paths(&f.attrs),
                        signature: f.sig.to_token_stream().to_string(),
                        location: span_to_location(file_path, f.span()),
                    });
                }
                Item::Impl(imp) => {
                    let self_ty = match &*imp.self_ty {
                        syn::Type::Path(tp) => tp
                            .path
                            .segments
                            .last()
                            .map(|s| s.ident.to_string())
                            .unwrap_or_else(|| type_to_string(&imp.self_ty)),
                        _ => type_to_string(&imp.self_ty),
                    };
                    let trait_ty = imp.trait_.as_ref().map(|(_, path, _)| path_to_string(path));

                    for it in &imp.items {
                        if let syn::ImplItem::Fn(m) = it {
                            let kind = FnKind::ImplMethod {
                                self_ty: self_ty.clone(),
                                trait_ty: trait_ty.clone(),
                            };
                            let name = m.sig.ident.to_string();
                            let fq = super::extract::fq_name(crate_name, mod_stack, &kind, &name);

                            out.push(CapturedFn {
                                name,
                                fq_name: fq,
                                is_public: vis_is_public(&m.vis),
                                has_docs: has_docs(&m.attrs),

                                is_async: m.sig.asyncness.is_some(),
                                is_unsafe: m.sig.unsafety.is_some(),
                                is_const: m.sig.constness.is_some(),
                                is_generic: !m.sig.generics.params.is_empty(),

                                args: fn_args(&m.sig),
                                return_ty: fn_return(&m.sig),

                                kind,
                                module_path: mod_stack.clone(),
                                attrs: attr_paths(&m.attrs),
                                signature: m.sig.to_token_stream().to_string(),
                                location: span_to_location(file_path, m.span()),
                            });
                        }
                    }
                }
                Item::Trait(t) => {
                    let trait_name = t.ident.to_string();
                    for it in &t.items {
                        if let syn::TraitItem::Fn(tf) = it {
                            let kind = FnKind::TraitMethod {
                                trait_name: trait_name.clone(),
                            };
                            let name = tf.sig.ident.to_string();
                            let fq = super::extract::fq_name(crate_name, mod_stack, &kind, &name);

                            out.push(CapturedFn {
                                name,
                                fq_name: fq,
                                is_public: true,
                                has_docs: has_docs(&tf.attrs),

                                is_async: tf.sig.asyncness.is_some(),
                                is_unsafe: tf.sig.unsafety.is_some(),
                                is_const: tf.sig.constness.is_some(),
                                is_generic: !tf.sig.generics.params.is_empty(),

                                args: fn_args(&tf.sig),
                                return_ty: fn_return(&tf.sig),

                                kind,
                                module_path: mod_stack.clone(),
                                attrs: attr_paths(&tf.attrs),
                                signature: tf.sig.to_token_stream().to_string(),
                                location: span_to_location(file_path, tf.span()),
                            });
                        }
                    }
                }
                Item::Mod(m) => {
                    if let Some((_, items)) = &m.content {
                        mod_stack.push(m.ident.to_string());
                        walk_items(crate_name, file_path, items, mod_stack, out);
                        mod_stack.pop();
                    }
                }
                _ => {}
            }
        }
    }

    walk_items(crate_name, file_path, &ast.items, &mut mod_stack, &mut out);
    out
}

/// Token-level macro and call / path occurrences.
/// (This is what powers finders + rules.)
pub fn extract_occurrences_v1(
    file_path: &std::path::Path,
    ast: &File,
) -> (
    Vec<MacroDef>,
    Vec<MacroInvocationV1>,
    Vec<PathOccurrenceV1>,
    Vec<CallOccurrenceV1>,
) {
    #[derive(Default)]
    struct V {
        mod_stack: Vec<String>,
        macros_def: Vec<MacroDef>,
        macros_inv: Vec<MacroInvocationV1>,
        paths: Vec<PathOccurrenceV1>,
        calls: Vec<CallOccurrenceV1>,
        file_path: std::path::PathBuf,
    }

    impl<'ast> Visit<'ast> for V {
        fn visit_item_mod(&mut self, i: &'ast syn::ItemMod) {
            if let Some((_, items)) = &i.content {
                self.mod_stack.push(i.ident.to_string());
                for it in items {
                    self.visit_item(it);
                }
                self.mod_stack.pop();
            }
        }

        fn visit_item_macro(&mut self, i: &'ast syn::ItemMacro) {
            // macro_rules! foo { ... }  OR  foo!{...}
            let name = i
                .ident
                .as_ref()
                .map(|x| x.to_string())
                .unwrap_or_else(|| "<macro>".into());
            // if this is a macro_rules definition, record as def
            if i.mac.path.is_ident("macro_rules") {
                self.macros_def.push(MacroDef {
                    name,
                    module_path: self.mod_stack.clone(),
                    location: span_to_location(&self.file_path, i.span()),
                });
            } else {
                // invocation-ish
                self.macros_inv.push(MacroInvocationV1 {
                    name: i
                        .mac
                        .path
                        .segments
                        .last()
                        .map(|s| s.ident.to_string())
                        .unwrap_or_else(|| "<macro>".into()),
                    module_path: self.mod_stack.clone(),
                    location: span_to_location(&self.file_path, i.span()),
                });
            }
            syn::visit::visit_item_macro(self, i);
        }

        fn visit_expr_macro(&mut self, i: &'ast syn::ExprMacro) {
            self.macros_inv.push(MacroInvocationV1 {
                name: i
                    .mac
                    .path
                    .segments
                    .last()
                    .map(|s| s.ident.to_string())
                    .unwrap_or_else(|| "<macro>".into()),
                module_path: self.mod_stack.clone(),
                location: span_to_location(&self.file_path, i.span()),
            });
            syn::visit::visit_expr_macro(self, i);
        }

        fn visit_path(&mut self, p: &'ast syn::Path) {
            let s = p
                .segments
                .iter()
                .map(|x| x.ident.to_string())
                .collect::<Vec<_>>()
                .join("::");
            if !s.is_empty() {
                self.paths.push(PathOccurrenceV1 {
                    path: s,
                    module_path: self.mod_stack.clone(),
                    location: span_to_location(&self.file_path, p.span()),
                });
            }
            syn::visit::visit_path(self, p);
        }

        fn visit_expr_method_call(&mut self, m: &'ast syn::ExprMethodCall) {
            self.calls.push(CallOccurrenceV1 {
                callee: m.method.to_string(),
                module_path: self.mod_stack.clone(),
                location: span_to_location(&self.file_path, m.span()),
            });
            syn::visit::visit_expr_method_call(self, m);
        }

        fn visit_expr_call(&mut self, c: &'ast syn::ExprCall) {
            // foo(...) or path::to::foo(...)
            let callee = c.func.to_token_stream().to_string();
            self.calls.push(CallOccurrenceV1 {
                callee,
                module_path: self.mod_stack.clone(),
                location: span_to_location(&self.file_path, c.span()),
            });
            syn::visit::visit_expr_call(self, c);
        }
    }

    let mut v = V::default();
    v.file_path = file_path.to_path_buf();
    v.visit_file(ast);
    (v.macros_def, v.macros_inv, v.paths, v.calls)
}

pub fn extract_occurrences(
    crate_name: &str,
    file_path: &std::path::Path,
    ast: &syn::File,
) -> (
    Vec<MacroDef>,
    Vec<MacroInvocation>,
    Vec<PathOccurrence>,
    Vec<CallOccurrence>,
) {
    use syn::visit::Visit;

    #[derive(Default)]
    struct V {
        crate_name: String,
        file_path: std::path::PathBuf,

        mod_stack: Vec<String>,

        // impl / trait context
        impl_self_ty: Option<String>,
        impl_trait_ty: Option<String>,
        in_trait: Option<String>,

        // current enclosing function
        current_fn: Option<String>,
        current_fn_is_public: Option<bool>,

        macros_def: Vec<MacroDef>,
        macros_inv: Vec<MacroInvocation>,
        paths: Vec<PathOccurrence>,
        calls: Vec<CallOccurrence>,
    }

    fn vis_is_public(vis: &syn::Visibility) -> bool {
        matches!(vis, syn::Visibility::Public(_))
    }

    fn path_to_string(p: &syn::Path) -> String {
        p.segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect::<Vec<_>>()
            .join("::")
    }

    fn type_to_string(ty: &syn::Type) -> String {
        use quote::ToTokens;
        ty.to_token_stream().to_string()
    }

    fn fq_name(crate_name: &str, module_path: &[String], kind: &FnKind, name: &str) -> String {
        let mut parts = Vec::new();
        parts.push(crate_name.to_string());
        parts.extend(module_path.iter().cloned());
        match kind {
            FnKind::FreeFn => parts.push(name.to_string()),
            FnKind::TraitMethod { trait_name } => {
                parts.push(trait_name.clone());
                parts.push(name.to_string());
            }
            FnKind::ImplMethod { self_ty, .. } => {
                parts.push(self_ty.clone());
                parts.push(name.to_string());
            }
        }
        parts.join("::")
    }

    impl<'ast> Visit<'ast> for V {
        fn visit_item_mod(&mut self, i: &'ast syn::ItemMod) {
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

            let self_ty = match &*i.self_ty {
                syn::Type::Path(tp) => tp
                    .path
                    .segments
                    .last()
                    .map(|s| s.ident.to_string())
                    .unwrap_or_else(|| type_to_string(&i.self_ty)),
                _ => type_to_string(&i.self_ty),
            };
            self.impl_self_ty = Some(self_ty);

            self.impl_trait_ty = i.trait_.as_ref().map(|(_, p, _)| path_to_string(p));

            syn::visit::visit_item_impl(self, i);

            self.impl_self_ty = prev_self;
            self.impl_trait_ty = prev_trait;
        }

        fn visit_item_fn(&mut self, i: &'ast syn::ItemFn) {
            let (kind, is_pub) = if let Some(tr) = &self.in_trait {
                (
                    FnKind::TraitMethod {
                        trait_name: tr.clone(),
                    },
                    true,
                )
            } else {
                (FnKind::FreeFn, vis_is_public(&i.vis))
            };

            let name = i.sig.ident.to_string();
            let fq = fq_name(&self.crate_name, &self.mod_stack, &kind, &name);

            let prev_fn = self.current_fn.take();
            let prev_pub = self.current_fn_is_public.take();

            self.current_fn = Some(fq);
            self.current_fn_is_public = Some(is_pub);

            // visit inside function body
            syn::visit::visit_item_fn(self, i);

            self.current_fn = prev_fn;
            self.current_fn_is_public = prev_pub;
        }

        fn visit_impl_item_fn(&mut self, i: &'ast syn::ImplItemFn) {
            let self_ty = self.impl_self_ty.clone().unwrap_or_else(|| "<impl>".into());
            let trait_ty = self.impl_trait_ty.clone();
            let kind = FnKind::ImplMethod { self_ty, trait_ty };

            let name = i.sig.ident.to_string();
            let fq = fq_name(&self.crate_name, &self.mod_stack, &kind, &name);

            let is_pub = vis_is_public(&i.vis);

            let prev_fn = self.current_fn.take();
            let prev_pub = self.current_fn_is_public.take();

            self.current_fn = Some(fq);
            self.current_fn_is_public = Some(is_pub);

            syn::visit::visit_impl_item_fn(self, i);

            self.current_fn = prev_fn;
            self.current_fn_is_public = prev_pub;
        }

        fn visit_trait_item_fn(&mut self, i: &'ast syn::TraitItemFn) {
            let tr = self.in_trait.clone().unwrap_or_else(|| "<trait>".into());
            let kind = FnKind::TraitMethod { trait_name: tr };

            let name = i.sig.ident.to_string();
            let fq = fq_name(&self.crate_name, &self.mod_stack, &kind, &name);

            let prev_fn = self.current_fn.take();
            let prev_pub = self.current_fn_is_public.take();

            self.current_fn = Some(fq);
            self.current_fn_is_public = Some(true);

            syn::visit::visit_trait_item_fn(self, i);

            self.current_fn = prev_fn;
            self.current_fn_is_public = prev_pub;
        }

        fn visit_item_macro(&mut self, i: &'ast syn::ItemMacro) {
            let name = i
                .ident
                .as_ref()
                .map(|x| x.to_string())
                .unwrap_or_else(|| "<macro>".into());
            if i.mac.path.is_ident("macro_rules") {
                self.macros_def.push(MacroDef {
                    name,
                    module_path: self.mod_stack.clone(),
                    location: super::extract::span_to_location(&self.file_path, i.span()),
                });
            } else {
                let full_path = {
                    let segs: Vec<String> = i
                        .mac
                        .path
                        .segments
                        .iter()
                        .map(|s| s.ident.to_string())
                        .collect();
                    if segs.is_empty() {
                        None
                    } else {
                        Some(segs.join("::"))
                    }
                };

                self.macros_inv.push(MacroInvocation {
                    name: i
                        .mac
                        .path
                        .segments
                        .last()
                        .map(|s| s.ident.to_string())
                        .unwrap_or_else(|| "<macro>".into()),
                    module_path: self.mod_stack.clone(),
                    path: full_path,
                    location: super::extract::span_to_location(&self.file_path, i.span()),
                    enclosing_fn: self.current_fn.clone(),
                    enclosing_public: self.current_fn_is_public,
                });
            }
            syn::visit::visit_item_macro(self, i);
        }

        fn visit_expr_macro(&mut self, i: &'ast syn::ExprMacro) {
            let full_path = {
                let segs: Vec<String> = i
                    .mac
                    .path
                    .segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .collect();
                if segs.is_empty() {
                    None
                } else {
                    Some(segs.join("::"))
                }
            };
            self.macros_inv.push(MacroInvocation {
                name: i
                    .mac
                    .path
                    .segments
                    .last()
                    .map(|s| s.ident.to_string())
                    .unwrap_or_else(|| "<macro>".into()),
                module_path: self.mod_stack.clone(),
                path: full_path,
                location: super::extract::span_to_location(&self.file_path, i.span()),
                enclosing_fn: self.current_fn.clone(),
                enclosing_public: self.current_fn_is_public,
            });
            syn::visit::visit_expr_macro(self, i);
        }

        fn visit_path(&mut self, p: &'ast syn::Path) {
            let s = p
                .segments
                .iter()
                .map(|x| x.ident.to_string())
                .collect::<Vec<_>>()
                .join("::");

            // reduce noise: only record “real” paths
            let keep = s.contains("::")
                || matches!(
                    s.as_str(),
                    "std" | "core" | "alloc" | "crate" | "self" | "super"
                );

            if keep {
                self.paths.push(PathOccurrence {
                    path: s,
                    module_path: self.mod_stack.clone(),
                    location: super::extract::span_to_location(&self.file_path, p.span()),
                    enclosing_fn: self.current_fn.clone(),
                    enclosing_public: self.current_fn_is_public,
                });
            }

            syn::visit::visit_path(self, p);
        }

        fn visit_expr_method_call(&mut self, m: &'ast syn::ExprMethodCall) {
            self.calls.push(CallOccurrence {
                callee: m.method.to_string(),
                module_path: self.mod_stack.clone(),
                location: super::extract::span_to_location(&self.file_path, m.span()),
                enclosing_fn: self.current_fn.clone(),
                enclosing_public: self.current_fn_is_public,
            });
            syn::visit::visit_expr_method_call(self, m);
        }

        fn visit_expr_call(&mut self, c: &'ast syn::ExprCall) {
            let callee = c.func.to_token_stream().to_string();
            self.calls.push(CallOccurrence {
                callee,
                module_path: self.mod_stack.clone(),
                location: super::extract::span_to_location(&self.file_path, c.span()),
                enclosing_fn: self.current_fn.clone(),
                enclosing_public: self.current_fn_is_public,
            });
            syn::visit::visit_expr_call(self, c);
        }
    }

    let mut v = V::default();
    v.crate_name = crate_name.to_string();
    v.file_path = file_path.to_path_buf();
    v.visit_file(ast);

    (v.macros_def, v.macros_inv, v.paths, v.calls)
}
