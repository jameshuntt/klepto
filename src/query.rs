use crate::klepto::Klepto;
use crate::model::*;
use regex::Regex;

pub struct ImportQuery<'k> {
    k: &'k Klepto,
    root: Option<String>,
    internal_only: bool,
    public_use_only: bool,
    full_prefix: Option<String>,
    origin: Option<ImportOrigin>,
    stdish_only: bool,
}

impl<'k> ImportQuery<'k> {
    pub(crate) fn new(k: &'k Klepto) -> Self {
        Self { k, root: None, internal_only: false, public_use_only: false, full_prefix: None, origin: None, stdish_only: false }
    }

    pub fn root(mut self, r: impl Into<String>) -> Self { self.root = Some(r.into()); self }
    pub fn internal_only(mut self) -> Self { self.internal_only = true; self }
    pub fn public_use_only(mut self) -> Self { self.public_use_only = true; self }
    pub fn full_path_starts_with(mut self, p: impl Into<String>) -> Self { self.full_prefix = Some(p.into()); self }

    pub fn filter<F>(self, f: F) -> Vec<StolenPath>
    where F: Fn(&StolenPath) -> bool
    {
        self.collect().into_iter().filter(|x| f(x)).collect()
    }

    pub fn collect(self) -> Vec<StolenPath> {
        let mut v = self.k.imports.clone();
        if let Some(r) = &self.root { v.retain(|p| &p.root == r); }
        if self.internal_only { v.retain(|p| p.is_internal); }
        if self.public_use_only { v.retain(|p| p.is_public_use); }
        if let Some(pref) = &self.full_prefix { v.retain(|p| p.full_path.starts_with(pref)); }
        v
    }

    pub fn origin(mut self, o: crate::model::ImportOrigin) -> Self {
        self.origin = Some(o);
        self
    }
    pub fn workspace_only(self) -> Self { self.origin(crate::model::ImportOrigin::WorkspaceMember) }
    pub fn deps_only(self) -> Self { self.origin(crate::model::ImportOrigin::Dependency) }
    pub fn stdish_only(mut self) -> Self {
        self.stdish_only = true;
        self
    }

}

pub struct FnQuery<'k> {
    k: &'k Klepto,

    public_only: bool,
    no_docs: bool,

    in_impl: Option<String>,
    impls_trait: Option<String>,
    in_trait: Option<String>,

    name_contains: Option<String>,
    name_regex: Option<Regex>,

    returns_contains: Option<String>,
    takes_arg_contains: Option<String>,

    is_async: Option<bool>,
    is_unsafe: Option<bool>,
    is_const: Option<bool>,
    is_generic: Option<bool>,

    has_attr: Option<String>,
}

impl<'k> FnQuery<'k> {
    pub(crate) fn new(k: &'k Klepto) -> Self {
        Self {
            k,
            public_only: false,
            no_docs: false,
            in_impl: None,
            impls_trait: None,
            in_trait: None,
            name_contains: None,
            name_regex: None,
            returns_contains: None,
            takes_arg_contains: None,
            is_async: None,
            is_unsafe: None,
            is_const: None,
            is_generic: None,
            has_attr: None,
        }
    }

    // presets
    pub fn public_only(mut self) -> Self { self.public_only = true; self }
    pub fn no_docs(mut self) -> Self { self.no_docs = true; self }

    // structure filters
    pub fn in_impl(mut self, ty: impl Into<String>) -> Self { self.in_impl = Some(ty.into()); self }
    pub fn impls_trait(mut self, tr: impl Into<String>) -> Self { self.impls_trait = Some(tr.into()); self }
    pub fn in_trait(mut self, tr: impl Into<String>) -> Self { self.in_trait = Some(tr.into()); self }

    // name filters
    pub fn named(mut self, n: impl Into<String>) -> Self { self.name_contains = Some(n.into()); self }
    pub fn name_contains(mut self, s: impl Into<String>) -> Self { self.name_contains = Some(s.into()); self }
    pub fn name_matches(mut self, re: &str) -> Self { self.name_regex = Some(Regex::new(re).unwrap()); self }

    // signature helpers
    pub fn returns(mut self, s: impl Into<String>) -> Self { self.returns_contains = Some(s.into()); self }
    pub fn takes_arg(mut self, s: impl Into<String>) -> Self { self.takes_arg_contains = Some(s.into()); self }

    // flags
    pub fn is_async(mut self, yes: bool) -> Self { self.is_async = Some(yes); self }
    pub fn is_unsafe(mut self, yes: bool) -> Self { self.is_unsafe = Some(yes); self }
    pub fn is_const(mut self, yes: bool) -> Self { self.is_const = Some(yes); self }
    pub fn is_generic(mut self, yes: bool) -> Self { self.is_generic = Some(yes); self }

    pub fn has_attr(mut self, a: impl Into<String>) -> Self { self.has_attr = Some(a.into()); self }

    pub fn filter<F>(self, f: F) -> Vec<CapturedFn>
    where F: Fn(&CapturedFn) -> bool
    {
        self.collect().into_iter().filter(|x| f(x)).collect()
    }

    pub fn collect(self) -> Vec<CapturedFn> {
        let mut v = self.k.functions.clone();

        if self.public_only { v.retain(|f| f.is_public); }
        if self.no_docs { v.retain(|f| !f.has_docs); }

        if let Some(ty) = &self.in_impl {
            v.retain(|f| matches!(&f.kind, FnKind::ImplMethod { self_ty, .. } if self_ty == ty));
        }
        if let Some(tr) = &self.impls_trait {
            v.retain(|f| matches!(&f.kind, FnKind::ImplMethod { trait_ty, .. } if trait_ty.as_deref() == Some(tr)));
        }
        if let Some(tr) = &self.in_trait {
            v.retain(|f| matches!(&f.kind, FnKind::TraitMethod { trait_name } if trait_name == tr));
        }

        if let Some(s) = &self.name_contains { v.retain(|f| f.name.contains(s)); }
        if let Some(re) = &self.name_regex { v.retain(|f| re.is_match(&f.name)); }

        if let Some(r) = &self.returns_contains {
            v.retain(|f| f.return_ty.as_deref().unwrap_or("").contains(r));
        }
        if let Some(a) = &self.takes_arg_contains {
            v.retain(|f| f.args.iter().any(|x| x.contains(a)));
        }

        if let Some(b) = self.is_async { v.retain(|f| f.is_async == b); }
        if let Some(b) = self.is_unsafe { v.retain(|f| f.is_unsafe == b); }
        if let Some(b) = self.is_const { v.retain(|f| f.is_const == b); }
        if let Some(b) = self.is_generic { v.retain(|f| f.is_generic == b); }

        if let Some(a) = &self.has_attr {
            v.retain(|f| f.attrs.iter().any(|x| x == a));
        }

        v
    }
}
