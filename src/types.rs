//! Phase 5: type records — structs, enums, unions, traits, type aliases and
//! impl blocks with resolved fields, variants, generics, bounds, derives and
//! associated items. Rules T1–T8 and N1 in `PHASE5_RULES.md`.
//!
//! The occurrence visitor in `extract.rs` emits the records with every
//! mention as an `Unresolved` placeholder; `resolve_types` resolves them in
//! the declaring module with the item's generic parameters in scope and
//! fills the impl back-links.

use crate::cfg::CfgPredicate;
use crate::klepto::Klepto;
use crate::model::{FileLocation, Namespace, Resolution};
use crate::resolve::{build_resolver, Ctx, Resolver};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TypeKind {
    Struct,
    Enum,
    Union,
    Trait,
    TypeAlias,
}

/// A type as written, plus what its paths name (rule T1).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TypeRef {
    /// Normalised spelling (rule N1): `Vec<Option<store::Engine>>`, `&'a str`.
    pub raw: String,
    /// Every path inside the type, in source order, resolved; the head path
    /// comes first. Primitives are not listed.
    pub mentions: Vec<Resolution>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum GenericKind {
    Lifetime,
    Type,
    Const,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenericParam {
    /// `'a`, `T`, `N`.
    pub name: String,
    pub kind: GenericKind,
    /// Inline bounds plus `where` predicates on exactly this parameter.
    pub bounds: Vec<TypeRef>,
    /// The const parameter's type.
    pub ty: Option<TypeRef>,
    pub default: Option<TypeRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FieldDecl {
    /// Position among the fields; tuple fields are addressed by it.
    pub index: usize,
    /// `None` for tuple fields.
    pub name: Option<String>,
    pub ty: TypeRef,
    pub is_public: bool,
    pub cfg: CfgPredicate,
    pub attrs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VariantDecl {
    pub name: String,
    pub fields: Vec<FieldDecl>,
    /// `= 0` as written.
    pub discriminant: Option<String>,
    pub cfg: CfgPredicate,
}

/// A trait's method declaration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MethodSig {
    /// `app::Runner::run`
    pub id: String,
    pub name: String,
    pub has_default: bool,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeDecl {
    /// Graph node id: `app::Config`.
    pub id: String,
    pub kind: TypeKind,
    pub name: String,
    pub package: String,
    pub module: String,
    pub location: FileLocation,
    pub cfg: CfgPredicate,
    pub is_public: bool,
    pub generics: Vec<GenericParam>,
    /// Structs and unions.
    pub fields: Vec<FieldDecl>,
    /// Enums.
    pub variants: Vec<VariantDecl>,
    /// Traits: supertrait bounds.
    pub supertraits: Vec<TypeRef>,
    /// Traits: method declarations.
    pub trait_methods: Vec<MethodSig>,
    /// Traits: associated type names.
    pub assoc_types: Vec<String>,
    /// Traits: associated const names.
    pub assoc_consts: Vec<String>,
    /// Type aliases: the target.
    pub aliased: Option<TypeRef>,
    /// `#[derive(..)]` arguments, resolved.
    pub derives: Vec<Resolution>,
    pub attrs: Vec<String>,
    /// Ids of impl blocks whose self type is this type (rule T8), sorted.
    pub impls: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImplDecl {
    /// Graph node id: `app::<impl Config<'a,T,N>>`, `app::<impl app::Runner for Config<'a,T,N>>`.
    pub id: String,
    pub package: String,
    pub module: String,
    pub location: FileLocation,
    pub cfg: CfgPredicate,
    pub generics: Vec<GenericParam>,
    pub self_ty: TypeRef,
    pub trait_: Option<TypeRef>,
    /// Method node ids, in source order.
    pub methods: Vec<String>,
    pub assoc_types: Vec<(String, TypeRef)>,
    pub assoc_consts: Vec<String>,
}

/// Rule N1: one canonical spelling for a type's tokens.
pub fn normalize_type_spelling(tokens: &str) -> String {
    let pieces: Vec<&str> = tokens.split_whitespace().collect();
    let mut out = String::new();
    for (i, piece) in pieces.iter().enumerate() {
        if i > 0 {
            let prev = out.chars().last().unwrap_or(' ');
            let next = piece.chars().next().unwrap_or(' ');
            let tight_after_prev = matches!(prev, '<' | '(' | '[' | '&' | '*' | ':');
            let tight_before_next = matches!(next, '>' | ')' | ']' | ',' | ';' | ':');
            let open_after_name = matches!(next, '<' | '(') && (prev.is_alphanumeric() || prev == '_' || prev == '>');
            if !(tight_after_prev || tight_before_next || open_after_name) {
                out.push(' ');
            }
        }
        out.push_str(piece);
    }
    out
}


fn generic_names(g: &[GenericParam]) -> Vec<String> {
    g.iter().filter(|p| p.kind != GenericKind::Lifetime).map(|p| p.name.clone()).collect()
}

/// The single-segment name R5 expects for `Self`: `Config<'a, T, N>` → `Config`.
fn self_name(raw: &str) -> String {
    let head = raw.split('<').next().unwrap_or(raw);
    head.rsplit("::").next().unwrap_or(head).trim_start_matches('&').trim().to_string()
}

fn resolve_mentions(r: &Resolver, ms: &mut [Resolution], cx: &Ctx) {
    for m in ms.iter_mut() {
        if let Resolution::Unresolved { raw, .. } = m {
            let raw = raw.clone();
            *m = r.resolve_path(&raw, cx);
        }
    }
}

fn resolve_ref(r: &Resolver, tr: &mut TypeRef, cx: &Ctx) {
    resolve_mentions(r, &mut tr.mentions, cx);
}

fn resolve_generic(r: &Resolver, g: &mut GenericParam, cx: &Ctx) {
    for b in g.bounds.iter_mut().chain(g.ty.iter_mut()).chain(g.default.iter_mut()) {
        resolve_ref(r, b, cx);
    }
}

/// Rules T1, T6, T8: resolves every mention in the declaring module with
/// the item's generics and `Self` in scope, then fills `TypeDecl::impls`.
pub(crate) fn resolve_types(k: &mut Klepto) {
    let mut r = build_resolver(k);
    for t in &k.types {
        r.add_item_context(&t.id, generic_names(&t.generics), Some(t.name.clone()));
    }
    for i in &k.impls {
        r.add_item_context(&i.id, generic_names(&i.generics), Some(self_name(&i.self_ty.raw)));
    }

    for t in k.types.iter_mut() {
        let (module, package, id) = (t.module.clone(), t.package.clone(), t.id.clone());
        let cx = Ctx { module: &module, package: &package, enclosing_fn: Some(&id), ns: Namespace::Type, cfg: None };
        for g in t.generics.iter_mut() {
            resolve_generic(&r, g, &cx);
        }
        for f in t.fields.iter_mut() {
            resolve_ref(&r, &mut f.ty, &cx);
        }
        for v in t.variants.iter_mut() {
            for f in v.fields.iter_mut() {
                resolve_ref(&r, &mut f.ty, &cx);
            }
        }
        for s in t.supertraits.iter_mut() {
            resolve_ref(&r, s, &cx);
        }
        if let Some(a) = t.aliased.as_mut() {
            resolve_ref(&r, a, &cx);
        }
        resolve_mentions(&r, &mut t.derives, &cx);
    }
    for i in k.impls.iter_mut() {
        let (module, package, id) = (i.module.clone(), i.package.clone(), i.id.clone());
        let cx = Ctx { module: &module, package: &package, enclosing_fn: Some(&id), ns: Namespace::Type, cfg: None };
        for g in i.generics.iter_mut() {
            resolve_generic(&r, g, &cx);
        }
        resolve_ref(&r, &mut i.self_ty, &cx);
        if let Some(t) = i.trait_.as_mut() {
            resolve_ref(&r, t, &cx);
        }
        for (_, t) in i.assoc_types.iter_mut() {
            resolve_ref(&r, t, &cx);
        }
    }

    // T8: back-links, byte-order sorted
    let mut by_type: HashMap<String, Vec<String>> = HashMap::new();
    for i in &k.impls {
        if let Some(Resolution::Local(l)) = i.self_ty.mentions.first() {
            by_type.entry(l.clone()).or_default().push(i.id.clone());
        }
    }
    for t in k.types.iter_mut() {
        if let Some(v) = by_type.get_mut(&t.id) {
            v.sort();
            t.impls = v.clone();
        }
    }
}

impl Klepto {
    pub fn type_decl(&self, id: &str) -> Option<&TypeDecl> {
        self.types.iter().find(|t| t.id == id)
    }

    /// Impl blocks whose self type is `type_id`.
    pub fn impls_of(&self, type_id: &str) -> Vec<&ImplDecl> {
        self.impls
            .iter()
            .filter(|i| matches!(i.self_ty.mentions.first(), Some(Resolution::Local(l)) if l == type_id))
            .collect()
    }
}
