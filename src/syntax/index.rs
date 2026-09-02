use crate::syntax::extract::{fq_name, impl_self_ty_name};
use crate::model::{FileLocation, FnKind};
use proc_macro2::Span;
use std::collections::HashMap;
use syn::{spanned::Spanned, visit::Visit};

#[derive(Debug, Clone)]
pub struct FnSpan {
    pub fq_name: String,
    pub is_public: bool,
    pub kind: FnKind,

    pub file: std::path::PathBuf,
    pub start: Option<(u32, u32)>, // (line, col)
    pub end: Option<(u32, u32)>,   // (line, col)
}

impl FnSpan {
    pub fn contains(&self, loc: &FileLocation) -> bool {
        if self.file != loc.path { return false; }
        let (line, col) = match (loc.line, loc.column) {
            (Some(l), Some(c)) => (l, c),
            _ => return false,
        };
        let (sl, sc) = match self.start { Some(x) => x, None => return false };
        let (el, ec) = match self.end { Some(x) => x, None => return false };

        // inclusive start, inclusive end
        (line > sl || (line == sl && col >= sc)) && (line < el || (line == el && col <= ec))
    }
}

/// Which function encloses a location. Only meaningful with the
/// `span-locations` feature (on by default).
#[derive(Debug, Default, Clone)]
pub struct EnclosingIndex {
    by_file: HashMap<std::path::PathBuf, Vec<FnSpan>>,
}

impl EnclosingIndex {
    /// `module` is the file's module path (segments without the package).
    pub fn build(package: &str, module: &[String], file_path: &std::path::Path, ast: &syn::File) -> Self {
        let mut v = Builder {
            package: package.to_string(),
            file_path: file_path.to_path_buf(),
            mod_stack: module.to_vec(),
            out: Vec::new(),
            impl_self_ty: None,
            impl_trait_ty: None,
            in_trait: None,
        };
        v.visit_file(ast);

        v.out.sort_by_key(|f| f.start.map(|x| x.0).unwrap_or(0));

        let mut idx = EnclosingIndex::default();
        idx.by_file.insert(file_path.to_path_buf(), v.out);
        idx
    }

    pub fn merge(mut self, other: EnclosingIndex) -> Self {
        for (k, mut v) in other.by_file {
            self.by_file.entry(k).or_default().append(&mut v);
        }
        self
    }

    pub fn enclosing<'a>(&'a self, loc: &FileLocation) -> Option<&'a FnSpan> {
        let v = self.by_file.get(&loc.path)?;
        // smallest span that contains loc (best match)
        v.iter()
            .filter(|f| f.contains(loc))
            .min_by_key(|f| {
                let sl = f.start.map(|x| x.0).unwrap_or(0);
                let el = f.end.map(|x| x.0).unwrap_or(u32::MAX);
                el.saturating_sub(sl)
            })
    }

    /// Apply the trait-canonicalisation renames of the resolver (see `resolve`).
    pub(crate) fn rename_fns(&mut self, map: &HashMap<String, String>) {
        for spans in self.by_file.values_mut() {
            for s in spans.iter_mut() {
                if let Some(n) = map.get(&s.fq_name) {
                    s.fq_name = n.clone();
                }
            }
        }
    }
}

struct Builder {
    package: String,
    file_path: std::path::PathBuf,
    mod_stack: Vec<String>,
    out: Vec<FnSpan>,

    impl_self_ty: Option<String>,
    impl_trait_ty: Option<String>,
    in_trait: Option<String>,
}

/// `(line, column)`; `None` without the `span-locations` feature.
type LineCol = Option<(u32, u32)>;

fn span_start_end(span: Span) -> (LineCol, LineCol) {
    #[cfg(feature = "span-locations")]
    {
        let s = span.start();
        let e = span.end();
        (Some((s.line as u32, s.column as u32)), Some((e.line as u32, e.column as u32)))
    }
    #[cfg(not(feature = "span-locations"))]
    {
        let _ = span;
        (None, None)
    }
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

impl Builder {
    fn push(&mut self, kind: FnKind, name: &str, is_public: bool, span: Span) {
        let fq = fq_name(&self.package, &self.mod_stack, &kind, name);
        let (start, end) = span_start_end(span);
        self.out.push(FnSpan {
            fq_name: fq,
            is_public,
            kind,
            file: self.file_path.clone(),
            start,
            end,
        });
    }
}

impl<'ast> Visit<'ast> for Builder {
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

        self.impl_self_ty = Some(impl_self_ty_name(&i.self_ty));
        self.impl_trait_ty = i.trait_.as_ref().map(|(_, p, _)| path_to_string(p));

        syn::visit::visit_item_impl(self, i);

        self.impl_self_ty = prev_self;
        self.impl_trait_ty = prev_trait;
    }

    fn visit_item_fn(&mut self, i: &'ast syn::ItemFn) {
        let kind = if let Some(tr) = &self.in_trait {
            FnKind::TraitMethod { trait_name: tr.clone() }
        } else {
            FnKind::FreeFn
        };
        let is_public = vis_is_public(&i.vis) || matches!(kind, FnKind::TraitMethod { .. });
        self.push(kind, &i.sig.ident.to_string(), is_public, i.span());
        syn::visit::visit_item_fn(self, i);
    }

    fn visit_impl_item_fn(&mut self, i: &'ast syn::ImplItemFn) {
        let self_ty = self.impl_self_ty.clone().unwrap_or_else(|| "<impl>".into());
        let kind = FnKind::ImplMethod { self_ty, trait_ty: self.impl_trait_ty.clone() };
        self.push(kind, &i.sig.ident.to_string(), vis_is_public(&i.vis), i.span());
        syn::visit::visit_impl_item_fn(self, i);
    }

    fn visit_trait_item_fn(&mut self, i: &'ast syn::TraitItemFn) {
        let tr = self.in_trait.clone().unwrap_or_else(|| "<trait>".into());
        let kind = FnKind::TraitMethod { trait_name: tr };
        self.push(kind, &i.sig.ident.to_string(), true, i.span());
        syn::visit::visit_trait_item_fn(self, i);
    }
}
