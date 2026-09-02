//! Method-call resolution: the receiver type of every method call, worked
//! out from the source alone.
//!
//! Each file's AST is walked again in the same order the occurrence visitor
//! used, so the n-th method call seen here is the n-th `is_method` call the
//! extractor recorded for that file (no line numbers involved). If the two
//! counts ever disagree the file is left untouched: never guess.
//!
//! Receivers are typed from a per-function environment of what is written —
//! `self`, typed parameters, annotated `let`s, constructor and free-function
//! return types, struct and variant literals — with references and
//! `Box`/`Rc`/`Arc` transparent, field access through the type
//! records, chaining via full return types, and generic receivers through
//! their bounds. The receiver typing adds the standard library's shapes (`Vec`, slices,
//! `String`, `Option`, `Result`, maps, sets, iterators, cells, tuples,
//! scalars) with a table of what their methods return, closure parameters
//! typed from the adaptor's receiver, pattern bindings (`let`, `if let`,
//! `match`, `for`), `?`, indexing, literals, `dyn`/`impl Trait` receivers
//! and the result of `map`-like closures. Everything else keeps
//! `Unresolved { MethodCall }` and no receiver type.
//!
//! Still out of scope: inference through arithmetic on
//! non-scalars, `IntoIterator` of scanned types, `Deref` of scanned types,
//! type aliases, `.into()`, `async` outputs, blanket impls, supertraits,
//! trait-provided methods on scanned types (`clone`-family excepted).

use crate::klepto::Klepto;
use crate::model::{CallOccurrence, CapturedFn, FnKind, Namespace, Resolution, UnresolvedReason};
use crate::resolve::{build_resolver, join_key, Ctx, CtxParts, Resolver};
use crate::typing::types::TypeKind;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;
use syn::visit::Visit;

/// What a receiver expression is, syntactically, with every local name
/// already substituted by what it was bound to.
#[derive(Clone)]
enum D<'a> {
    /// The `self` parameter.
    SelfRecv,
    /// A written type: parameter, `let x: T`, `as T`.
    Ty(&'a syn::Type),
    /// `Path(..)`: tuple-struct constructor, variant constructor or fn call,
    /// with its arguments already described (names mean what they
    /// meant where the expression is written).
    Call(&'a syn::Path, Vec<D<'a>>),
    /// `Path` as a value: unit struct or unit variant.
    Value(&'a syn::Path),
    /// `Path { .. }`.
    Lit(&'a syn::Path),
    /// `recv.method::<T>(args..)`.
    Method { recv: Box<D<'a>>, name: String, args: Vec<D<'a>>, turbofish: Option<&'a syn::Type> },
    /// `|params| body`, the body described with the parameters bound.
    Closure { body: Box<D<'a>>, ret: Option<&'a syn::Type> },
    /// `recv.field` / `recv.0`.
    Field(Box<D<'a>>, syn::Member),
    /// `expr?`
    Try(Box<D<'a>>),
    /// `expr[i]`
    Index(Box<D<'a>>),
    /// `(a, b)`
    Tuple(Vec<D<'a>>),
    /// `[a, b]` / `[a; n]`
    Array(Box<D<'a>>),
    /// A part of another value, bound by a pattern or a closure parameter.
    Proj(Box<D<'a>>, Proj),
    /// Already typed (literals, closure-result overlays).
    Typed(RecvTy),
    Unknown,
}

#[derive(Clone)]
enum Proj {
    OptElem,
    ResOk,
    ResErr,
    TupleIdx(usize),
    SeqElem,
    /// The item of `for _ in value`.
    IterItem,
    /// Parameter `param` of the closure passed as argument `arg` of `method`.
    ClosureParam { method: String, arg: usize, param: usize, arity: usize },
}

/// The standard-library shapes klepto knows the methods of.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Shape {
    Seq,
    Str,
    Opt,
    Res,
    Map,
    Set,
    Iter,
    Entry,
    Cell,
    Scalar,
    Other,
}

/// A typed receiver.
#[derive(Clone, Debug, PartialEq, Eq)]
enum RecvTy {
    /// A scanned type (has a `TypeDecl`).
    Local(String),
    /// An unscanned crate's type: methods are unverified.
    External(String),
    /// A generic parameter of the enclosing fn or impl.
    Generic(String),
    /// A standard-library type: `name` is its id, `args` what is known of
    /// its type arguments (element; key, value; ok, err; inner).
    Std { shape: Shape, name: String, args: Vec<Option<RecvTy>> },
    Tuple(Vec<Option<RecvTy>>),
}

fn std(shape: Shape, name: &str, args: Vec<Option<RecvTy>>) -> RecvTy {
    RecvTy::Std { shape, name: name.to_string(), args }
}
fn seq(e: Option<RecvTy>) -> RecvTy {
    std(Shape::Seq, "std::Vec", vec![e])
}
fn opt(e: Option<RecvTy>) -> RecvTy {
    std(Shape::Opt, "std::Option", vec![e])
}
fn res(ok: Option<RecvTy>, err: Option<RecvTy>) -> RecvTy {
    std(Shape::Res, "std::Result", vec![ok, err])
}
fn iter(e: Option<RecvTy>) -> RecvTy {
    std(Shape::Iter, "std::Iterator", vec![e])
}
fn string() -> RecvTy {
    std(Shape::Str, "std::String", Vec::new())
}
fn scalar(n: &str) -> RecvTy {
    std(Shape::Scalar, &format!("std::primitive::{n}"), Vec::new())
}
fn tuple2(a: Option<RecvTy>, b: Option<RecvTy>) -> RecvTy {
    RecvTy::Tuple(vec![a, b])
}

const PRIMITIVES: &[&str] = &[
    "bool", "char", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32", "f64", "str",
];

/// The shape of a std type by its last path segment.
fn std_of_name(last: &str) -> Option<(Shape, String)> {
    let (shape, name) = match last {
        "Vec" => (Shape::Seq, "std::Vec"),
        "VecDeque" => (Shape::Seq, "std::collections::VecDeque"),
        "LinkedList" => (Shape::Seq, "std::collections::LinkedList"),
        "BinaryHeap" => (Shape::Seq, "std::collections::BinaryHeap"),
        "String" => (Shape::Str, "std::String"),
        "str" => (Shape::Str, "std::primitive::str"),
        "Option" => (Shape::Opt, "std::Option"),
        "Result" => (Shape::Res, "std::Result"),
        "HashMap" => (Shape::Map, "std::collections::HashMap"),
        "BTreeMap" => (Shape::Map, "std::collections::BTreeMap"),
        "HashSet" => (Shape::Set, "std::collections::HashSet"),
        "BTreeSet" => (Shape::Set, "std::collections::BTreeSet"),
        "RefCell" => (Shape::Cell, "std::cell::RefCell"),
        "Cell" => (Shape::Cell, "std::cell::Cell"),
        "OnceCell" => (Shape::Cell, "std::cell::OnceCell"),
        "LazyCell" => (Shape::Cell, "std::cell::LazyCell"),
        "Mutex" => (Shape::Cell, "std::sync::Mutex"),
        "RwLock" => (Shape::Cell, "std::sync::RwLock"),
        "OnceLock" => (Shape::Cell, "std::sync::OnceLock"),
        "LazyLock" => (Shape::Cell, "std::sync::LazyLock"),
        p if PRIMITIVES.contains(&p) => return Some((Shape::Scalar, format!("std::primitive::{p}"))),
        _ => return None,
    };
    Some((shape, name.to_string()))
}

/// The shape of a resolved std id (`std::…`, `core::…`, `alloc::…`); the
/// id keeps the resolver's spelling so one type has one id everywhere.
fn std_of_id(id: &str) -> Option<(Shape, String)> {
    let first = id.split("::").next().unwrap_or_default();
    let last = id.rsplit("::").next().unwrap_or_default();
    if matches!(first, "std" | "core" | "alloc") {
        return Some((std_of_name(last).map(|(s, _)| s).unwrap_or(Shape::Other), id.to_string()));
    }
    if id.split("::").count() == 1 {
        return std_of_name(id);
    }
    None
}

/// Wrappers that are transparent for method calls (auto-deref).
const TRANSPARENT: &[&str] = &["Box", "Rc", "Arc", "Cow", "Pin", "ManuallyDrop", "Ref", "RefMut", "MutexGuard", "RwLockReadGuard", "RwLockWriteGuard"];

fn seg_path(p: &syn::Path) -> String {
    let mut s = String::new();
    if p.leading_colon.is_some() {
        s.push_str("::");
    }
    s.push_str(&p.segments.iter().map(|x| x.ident.to_string()).collect::<Vec<_>>().join("::"));
    s
}

fn generic_types(seg: &syn::PathSegment) -> Vec<&syn::Type> {
    match &seg.arguments {
        syn::PathArguments::AngleBracketed(a) => a
            .args
            .iter()
            .filter_map(|g| match g {
                syn::GenericArgument::Type(t) => Some(t),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// `Iterator<Item = T>` → `T`.
fn assoc_type<'t>(p: &'t syn::Path, name: &str) -> Option<&'t syn::Type> {
    let syn::PathArguments::AngleBracketed(a) = &p.segments.last()?.arguments else { return None };
    a.args.iter().find_map(|g| match g {
        syn::GenericArgument::AssocType(at) if at.ident == name => Some(&at.ty),
        _ => None,
    })
}

fn strip_generics(rt: Option<RecvTy>) -> Option<RecvTy> {
    match rt? {
        RecvTy::Generic(_) => None,
        RecvTy::Std { shape, name, args } => Some(RecvTy::Std { shape, name, args: args.into_iter().map(strip_generics).collect() }),
        RecvTy::Tuple(v) => Some(RecvTy::Tuple(v.into_iter().map(strip_generics).collect())),
        other => Some(other),
    }
}

fn arg0(args: &[Option<RecvTy>]) -> Option<RecvTy> {
    args.first().cloned().flatten()
}
fn arg1(args: &[Option<RecvTy>]) -> Option<RecvTy> {
    args.get(1).cloned().flatten()
}

/// The item of `for _ in value`.
fn iterable_elem(rt: &RecvTy) -> Option<RecvTy> {
    let RecvTy::Std { shape, args, .. } = rt else { return None };
    match shape {
        Shape::Seq | Shape::Set | Shape::Iter | Shape::Opt => arg0(args),
        Shape::Res => arg0(args),
        Shape::Map => Some(tuple2(arg0(args), arg1(args))),
        _ => None,
    }
}

/// What the closure passed as argument `arg` of `method` receives
/// as its parameter `param` (of `arity`).
fn closure_param_type(rt: &RecvTy, method: &str, arg: usize, param: usize, arity: usize) -> Option<RecvTy> {
    let RecvTy::Std { shape, args, .. } = rt else { return None };
    let elem = arg0(args);
    let second = arg1(args);
    match shape {
        Shape::Res => match method {
            "map_err" | "or_else" | "unwrap_or_else" | "inspect_err" | "is_err_and" => second,
            "map_or_else" => {
                if arg == 0 {
                    second
                } else {
                    elem
                }
            }
            _ => elem,
        },
        Shape::Opt => match method {
            "unwrap_or_else" | "or_else" | "ok_or_else" | "get_or_insert_with" => None,
            "map_or_else" => {
                if arg == 0 {
                    None
                } else {
                    elem
                }
            }
            _ => elem,
        },
        Shape::Map => match method {
            "retain" | "for_each" => {
                if arity == 2 {
                    if param == 0 {
                        elem
                    } else {
                        second
                    }
                } else {
                    Some(tuple2(elem, second))
                }
            }
            _ => None,
        },
        Shape::Entry => match method {
            "and_modify" => elem,
            _ => None,
        },
        Shape::Cell => None,
        Shape::Seq | Shape::Set | Shape::Iter => match method {
            "fold" | "try_fold" | "scan" | "rfold" | "try_rfold" => {
                if param == 0 {
                    None
                } else {
                    elem
                }
            }
            "for_each" | "map" | "filter" | "filter_map" | "find" | "find_map" | "any" | "all" | "position" | "rposition" | "flat_map"
            | "inspect" | "take_while" | "skip_while" | "map_while" | "retain" | "retain_mut" | "partition" | "dedup_by_key" | "sort_by_key"
            | "sort_unstable_by_key" | "sort_by_cached_key" | "max_by_key" | "min_by_key" | "binary_search_by" | "binary_search_by_key"
            | "group_by" | "chunk_by" | "is_sorted_by_key" | "try_for_each" | "extract_if" | "drain_filter" => elem,
            "sort_by" | "sort_unstable_by" | "max_by" | "min_by" | "dedup_by" | "is_sorted_by" | "partition_point" => elem,
            _ => {
                if arity == 1 {
                    elem
                } else {
                    None
                }
            }
        },
        Shape::Str => match method {
            "split" | "trim_matches" | "trim_start_matches" | "trim_end_matches" | "find" | "rfind" | "contains" | "starts_with" | "ends_with"
            | "matches" | "replace" | "splitn" | "rsplit" | "split_terminator" | "strip_prefix" | "strip_suffix" => Some(scalar("char")),
            _ => None,
        },
        Shape::Scalar | Shape::Other => None,
    }
}

/// Bindings made while describing a closure body or a block.
type Overlay<'a> = Vec<(String, D<'a>)>;

struct Frame<'a> {
    /// Innermost last; each scope is a list of bindings, latest last.
    scopes: Vec<Vec<(String, D<'a>)>>,
    /// The fn's generics plus those of the enclosing impl / trait.
    generics: Vec<&'a syn::Generics>,
}

/// The call site's resolution context.
struct FnCtx {
    module: String,
    package: String,
    enclosing_fn: Option<String>,
}

impl FnCtx {
    fn ctx(&self) -> Ctx<'_> {
        Ctx { module: &self.module, package: &self.package, enclosing_fn: self.enclosing_fn.as_deref(), ns: Namespace::Any, cfg: None }
    }
}

struct Idents(Vec<String>);

impl<'ast> Visit<'ast> for Idents {
    fn visit_pat_ident(&mut self, p: &'ast syn::PatIdent) {
        self.0.push(p.ident.to_string());
        syn::visit::visit_pat_ident(self, p);
    }
}

/// Every name a pattern binds.
fn pat_idents(p: &syn::Pat) -> Vec<String> {
    let mut v = Idents(Vec::new());
    v.visit_pat(p);
    v.0
}

fn last_segment(id: &str) -> &str {
    id.rsplit("::").next().unwrap_or(id)
}

fn unresolved(name: &str, reason: UnresolvedReason, candidates: Vec<String>) -> Resolution {
    Resolution::Unresolved { raw: name.to_string(), reason, candidates }
}

/// Where a closure sits: its receiver and adaptor, for typing its parameters.
#[derive(Clone)]
struct ClosureSite<'a> {
    recv: D<'a>,
    method: String,
    arg: usize,
}

fn span_key(c: &syn::ExprClosure) -> (usize, usize) {
    let s = c.or1_token.span.start();
    (s.line, s.column)
}

fn tail_expr(b: &syn::Block) -> Option<&syn::Expr> {
    match b.stmts.last()? {
        syn::Stmt::Expr(e, None) => Some(e),
        _ => None,
    }
}

struct W<'a, 'k> {
    k: &'k Klepto,
    r: &'k Resolver,
    fns: &'k HashMap<&'k str, &'k CapturedFn>,
    parts: &'k CtxParts,
    /// This file's method calls, in extraction order.
    calls: &'k [&'k CallOccurrence],
    /// Method calls seen so far in this walk.
    next: usize,
    /// `(receiver_type, resolution)` per method call, in walk order.
    out: Vec<(Option<Resolution>, Option<Resolution>)>,
    frames: Vec<Frame<'a>>,
    outer_generics: Vec<&'a syn::Generics>,
    /// Scrutinees of the `match` expressions being visited, innermost last.
    scrutinees: Vec<D<'a>>,
    /// Closure sites by position; `None` when two closures share a position
    /// (macro-expanded code) — then nothing is assumed.
    closure_sites: HashMap<(usize, usize), Option<ClosureSite<'a>>>,
    /// Temporary bindings while typing a closure's body.
    overlay: RefCell<Vec<Overlay<'a>>>,
    /// Nesting of `descriptor` / `type_of`; past `MAX_NESTING` the answer is
    /// Unknown rather than a stack overflow (deeply nested expressions).
    nesting: Cell<usize>,
}

const MAX_NESTING: usize = 256;

impl<'a, 'k> W<'a, 'k> {
    // ---- environment ----

    fn enter(&mut self, sig: &'a syn::Signature) {
        let mut generics = self.outer_generics.clone();
        generics.push(&sig.generics);
        let mut scope = Vec::new();
        for input in &sig.inputs {
            match input {
                syn::FnArg::Receiver(_) => scope.push(("self".to_string(), D::SelfRecv)),
                syn::FnArg::Typed(pt) => match &*pt.pat {
                    syn::Pat::Ident(pi) => scope.push((pi.ident.to_string(), D::Ty(&pt.ty))),
                    other => {
                        self.frames.push(Frame { scopes: vec![scope], generics: generics.clone() });
                        self.bind_pattern(other, D::Ty(&pt.ty));
                        let f = self.frames.pop();
                        scope = f.map(|f| f.scopes.into_iter().next().unwrap_or_default()).unwrap_or_default();
                    }
                },
            }
        }
        self.frames.push(Frame { scopes: vec![scope], generics });
    }

    fn leave(&mut self) {
        self.frames.pop();
    }

    fn push_scope(&mut self) {
        if let Some(f) = self.frames.last_mut() {
            f.scopes.push(Vec::new());
        }
    }

    fn pop_scope(&mut self) {
        if let Some(f) = self.frames.last_mut() {
            f.scopes.pop();
        }
    }

    fn bind(&mut self, name: String, d: D<'a>) {
        if let Some(scope) = self.frames.last_mut().and_then(|f| f.scopes.last_mut()) {
            scope.push((name, d));
        }
    }

    /// Names bound by a pattern shadow whatever they shadow with Unknown.
    fn bind_unknown(&mut self, pat: &syn::Pat) {
        for n in pat_idents(pat) {
            self.bind(n, D::Unknown);
        }
    }

    /// Binds a pattern's names to parts of `d`, syntactically.
    fn bind_pattern(&mut self, pat: &'a syn::Pat, d: D<'a>) {
        for (n, d) in pattern_bindings(pat, d) {
            self.bind(n, d);
        }
    }

    fn lookup(&self, name: &str) -> Option<D<'a>> {
        for layer in self.overlay.borrow().iter().rev() {
            if let Some((_, d)) = layer.iter().rev().find(|(n, _)| n == name) {
                return Some(d.clone());
            }
        }
        let f = self.frames.last()?;
        f.scopes.iter().rev().find_map(|s| s.iter().rev().find(|(n, _)| n == name).map(|(_, d)| d.clone()))
    }

    /// The receiver expression with locals substituted.
    fn descriptor(&self, e: &'a syn::Expr) -> D<'a> {
        let n = self.nesting.get();
        if n > MAX_NESTING {
            return D::Unknown;
        }
        self.nesting.set(n + 1);
        let d = self.descriptor_inner(e);
        self.nesting.set(n);
        d
    }

    fn descriptor_inner(&self, e: &'a syn::Expr) -> D<'a> {
        match e {
            syn::Expr::Path(p) if p.qself.is_none() => {
                if p.path.leading_colon.is_none() && p.path.segments.len() == 1 && p.path.segments[0].arguments.is_none() {
                    let name = p.path.segments[0].ident.to_string();
                    self.lookup(&name).unwrap_or(D::Value(&p.path))
                } else {
                    D::Value(&p.path)
                }
            }
            syn::Expr::Call(c) => match &*c.func {
                syn::Expr::Path(p) if p.qself.is_none() => D::Call(&p.path, c.args.iter().map(|a| self.describe_arg(a, None)).collect()),
                _ => D::Unknown,
            },
            syn::Expr::Struct(s) if s.qself.is_none() => D::Lit(&s.path),
            syn::Expr::MethodCall(m) => {
                let recv = self.descriptor(&m.receiver);
                let name = m.method.to_string();
                let args = m.args.iter().enumerate().map(|(i, a)| self.describe_arg(a, Some((&recv, &name, i)))).collect();
                let turbofish = m.turbofish.as_ref().and_then(|t| {
                    t.args.iter().find_map(|g| match g {
                        syn::GenericArgument::Type(t) => Some(t),
                        _ => None,
                    })
                });
                D::Method { recv: Box::new(recv), name, args, turbofish }
            }
            syn::Expr::Field(f) => D::Field(Box::new(self.descriptor(&f.base)), f.member.clone()),
            syn::Expr::Reference(r) => self.descriptor(&r.expr),
            syn::Expr::Paren(p) => self.descriptor(&p.expr),
            syn::Expr::Group(g) => self.descriptor(&g.expr),
            syn::Expr::Unary(u) => match u.op {
                syn::UnOp::Deref(_) | syn::UnOp::Neg(_) | syn::UnOp::Not(_) => self.descriptor(&u.expr),
                _ => D::Unknown,
            },
            syn::Expr::Cast(c) => D::Ty(&c.ty),
            syn::Expr::Try(t) => D::Try(Box::new(self.descriptor(&t.expr))),
            syn::Expr::Index(i) => D::Index(Box::new(self.descriptor(&i.expr))),
            syn::Expr::Tuple(t) => D::Tuple(t.elems.iter().map(|e| self.descriptor(e)).collect()),
            syn::Expr::Array(a) => a.elems.first().map(|e| D::Array(Box::new(self.descriptor(e)))).unwrap_or(D::Typed(seq(None))),
            syn::Expr::Repeat(r) => D::Array(Box::new(self.descriptor(&r.expr))),
            syn::Expr::Range(_) => D::Typed(std(Shape::Iter, "std::ops::Range", vec![Some(scalar("usize"))])),
            syn::Expr::Lit(l) => match &l.lit {
                syn::Lit::Str(_) => D::Typed(scalar("str")),
                syn::Lit::ByteStr(_) => D::Typed(std(Shape::Seq, "std::primitive::slice", vec![Some(scalar("u8"))])),
                syn::Lit::Byte(_) => D::Typed(scalar("u8")),
                syn::Lit::Char(_) => D::Typed(scalar("char")),
                syn::Lit::Int(i) => D::Typed(scalar(if i.suffix().is_empty() { "i32" } else { i.suffix() })),
                syn::Lit::Float(f) => D::Typed(scalar(if f.suffix().is_empty() { "f64" } else { f.suffix() })),
                syn::Lit::Bool(_) => D::Typed(scalar("bool")),
                _ => D::Unknown,
            },
            syn::Expr::Binary(b) => match b.op {
                syn::BinOp::Eq(_) | syn::BinOp::Ne(_) | syn::BinOp::Lt(_) | syn::BinOp::Le(_) | syn::BinOp::Gt(_) | syn::BinOp::Ge(_) | syn::BinOp::And(_) | syn::BinOp::Or(_) => {
                    D::Typed(scalar("bool"))
                }
                _ => self.descriptor(&b.left),
            },
            syn::Expr::Block(b) => self.block_descriptor(&b.block),
            syn::Expr::Unsafe(u) => self.block_descriptor(&u.block),
            syn::Expr::If(i) => self.block_descriptor(&i.then_branch),
            syn::Expr::Closure(_) => self.describe_arg(e, None),
            syn::Expr::Macro(m) => match m.mac.path.segments.last().map(|s| s.ident.to_string()).as_deref() {
                Some("format" | "concat" | "stringify" | "env" | "include_str" | "module_path" | "file") => D::Typed(string()),
                Some("vec") => D::Typed(seq(None)),
                Some("matches") => D::Typed(scalar("bool")),
                Some("line" | "column") => D::Typed(scalar("u32")),
                _ => D::Unknown,
            },
            _ => D::Unknown,
        }
    }

    /// A call argument: closures are described with their parameters bound
    /// to items of `site` so their body's result can be typed later.
    fn describe_arg(&self, a: &'a syn::Expr, site: Option<(&D<'a>, &str, usize)>) -> D<'a> {
        let syn::Expr::Closure(c) = a else { return self.descriptor(a) };
        let arity = c.inputs.len();
        let mut layer: Overlay<'a> = Vec::new();
        for (i, p) in c.inputs.iter().enumerate() {
            let (pat, base): (&'a syn::Pat, D<'a>) = match p {
                syn::Pat::Type(pt) => (&pt.pat, D::Ty(&pt.ty)),
                other => match site {
                    Some((recv, method, arg)) => (other, D::Proj(Box::new(recv.clone()), Proj::ClosureParam { method: method.to_string(), arg, param: i, arity })),
                    None => (other, D::Unknown),
                },
            };
            layer.extend(pattern_bindings(pat, base));
        }
        self.overlay.borrow_mut().push(layer);
        let body = self.descriptor(&c.body);
        self.overlay.borrow_mut().pop();
        let ret = match &c.output {
            syn::ReturnType::Type(_, t) => Some(&**t),
            syn::ReturnType::Default => None,
        };
        D::Closure { body: Box::new(body), ret }
    }

    /// A block's tail expression, with the block's own `let`s in scope.
    fn block_descriptor(&self, b: &'a syn::Block) -> D<'a> {
        let mut layer: Overlay<'a> = Vec::new();
        self.overlay.borrow_mut().push(Vec::new());
        for s in &b.stmts {
            if let syn::Stmt::Local(l) = s {
                let init = l.init.as_ref().map(|i| self.descriptor(&i.expr)).unwrap_or(D::Unknown);
                let binds = match &l.pat {
                    syn::Pat::Type(pt) => pattern_bindings(&pt.pat, D::Ty(&pt.ty)),
                    other => pattern_bindings(other, init),
                };
                layer.extend(binds.iter().cloned());
                if let Some(top) = self.overlay.borrow_mut().last_mut() {
                    top.extend(binds);
                }
            }
        }
        let d = tail_expr(b).map(|e| self.descriptor(e)).unwrap_or(D::Unknown);
        self.overlay.borrow_mut().pop();
        let _ = layer;
        d
    }

    fn fn_ctx(&self, call: &CallOccurrence) -> FnCtx {
        let (package, module) = self.parts.parts(&call.location.path, &call.module_path);
        FnCtx { module, package, enclosing_fn: call.enclosing_fn.clone() }
    }

    // ---- typing ----

    /// A resolved type name becomes a receiver type only when klepto knows
    /// it: scanned types (also when spelled as another workspace member)
    /// are `Local`, std shapes `Std`, anything else resolvable `External`.
    fn fold(&self, res: Resolution) -> Option<RecvTy> {
        match res {
            Resolution::Local(l) => self.k.type_decl(&l).map(|_| RecvTy::Local(l)),
            Resolution::External(e) => Some(if self.k.type_decl(&e).is_some() {
                RecvTy::Local(e)
            } else if let Some((shape, name)) = std_of_id(&e) {
                std(shape, &name, Vec::new())
            } else {
                RecvTy::External(e)
            }),
            _ => None,
        }
    }

    fn type_of_syn(&self, t: &syn::Type, cx: &Ctx) -> Option<RecvTy> {
        match t {
            syn::Type::Reference(r) => self.type_of_syn(&r.elem, cx),
            syn::Type::Paren(p) => self.type_of_syn(&p.elem, cx),
            syn::Type::Group(g) => self.type_of_syn(&g.elem, cx),
            syn::Type::Ptr(p) => self.type_of_syn(&p.elem, cx),
            syn::Type::Slice(s) => Some(std(Shape::Seq, "std::primitive::slice", vec![self.type_of_syn(&s.elem, cx)])),
            syn::Type::Array(a) => Some(std(Shape::Seq, "std::primitive::array", vec![self.type_of_syn(&a.elem, cx)])),
            syn::Type::Tuple(tu) => Some(if tu.elems.is_empty() { scalar("unit") } else { RecvTy::Tuple(tu.elems.iter().map(|e| self.type_of_syn(e, cx)).collect()) }),
            syn::Type::TraitObject(o) => self.first_bound(o.bounds.iter(), cx),
            syn::Type::ImplTrait(i) => self.first_bound(i.bounds.iter(), cx),
            syn::Type::Path(p) if p.qself.is_none() => self.type_of_path(&p.path, cx),
            _ => None,
        }
    }

    /// `dyn A + B` / `impl A + B`: the first trait; iterator
    /// traits become the iterator shape with their `Item`.
    fn first_bound<'b>(&self, bounds: impl Iterator<Item = &'b syn::TypeParamBound>, cx: &Ctx) -> Option<RecvTy> {
        let path = bounds
            .filter_map(|b| match b {
                syn::TypeParamBound::Trait(t) => Some(&t.path),
                _ => None,
            })
            .next()?;
        let last = path.segments.last()?.ident.to_string();
        let tcx = Ctx { module: cx.module, package: cx.package, enclosing_fn: cx.enclosing_fn, ns: Namespace::Type, cfg: None };
        match self.r.resolve_path(&seg_path(path), &tcx) {
            Resolution::Local(s) | Resolution::External(s) if self.k.type_decl(&s).is_some() => Some(RecvTy::Local(s)),
            Resolution::Local(s) => Some(RecvTy::Local(s)),
            Resolution::External(s) => {
                if matches!(last.as_str(), "Iterator" | "IntoIterator" | "DoubleEndedIterator" | "ExactSizeIterator" | "FusedIterator") {
                    Some(iter(assoc_type(path, "Item").and_then(|t| self.type_of_syn(t, cx))))
                } else {
                    Some(RecvTy::External(s))
                }
            }
            _ => match last.as_str() {
                "Iterator" | "IntoIterator" | "DoubleEndedIterator" | "ExactSizeIterator" | "FusedIterator" => {
                    Some(iter(assoc_type(path, "Item").and_then(|t| self.type_of_syn(t, cx))))
                }
                _ => None,
            },
        }
    }

    fn type_of_path(&self, path: &syn::Path, cx: &Ctx) -> Option<RecvTy> {
        let last = path.segments.last()?;
        let name = last.ident.to_string();
        let type_args = generic_types(last);
        let tcx = Ctx { module: cx.module, package: cx.package, enclosing_fn: cx.enclosing_fn, ns: Namespace::Type, cfg: None };
        let res = self.r.resolve_path(&seg_path(path), &tcx);
        let scanned = matches!(&res, Resolution::Local(s) | Resolution::External(s) if self.k.type_decl(s).is_some());
        if !scanned && TRANSPARENT.contains(&name.as_str()) {
            if let Some(inner) = type_args.first() {
                return self.type_of_syn(inner, cx);
            }
        }
        let base = match res {
            Resolution::Unresolved { raw, reason: UnresolvedReason::GenericParam, .. } => return Some(RecvTy::Generic(raw)),
            Resolution::Local(_) | Resolution::External(_) => self.fold(res),
            _ => None,
        };
        let base = match base {
            Some(b) => b,
            None => {
                let (shape, id) = std_of_name(&name)?;
                std(shape, &id, Vec::new())
            }
        };
        match base {
            RecvTy::Std { shape, name, .. } if !matches!(shape, Shape::Scalar | Shape::Str) => {
                let args = type_args.iter().map(|t| self.type_of_syn(t, cx)).collect();
                Some(RecvTy::Std { shape, name, args })
            }
            other => Some(other),
        }
    }

    /// `self`: the impl's self type, or the trait inside a default method.
    fn self_type(&self, cx: &FnCtx) -> Option<RecvTy> {
        let fq = cx.enclosing_fn.as_deref()?;
        if let Some(rt) = self.fold(self.r.resolve_path("Self", &cx.ctx())) {
            return Some(rt);
        }
        let f = self.fns.get(fq)?;
        match &f.kind {
            FnKind::TraitMethod { .. } => {
                let (owner, _) = fq.rsplit_once("::")?;
                self.k.type_decl(owner).map(|_| RecvTy::Local(owner.to_string()))
            }
            _ => None,
        }
    }

    /// A struct / union id, or an enum variant id, as a receiver type.
    fn type_of_value(&self, id: &str) -> Option<RecvTy> {
        if let Some(td) = self.k.type_decl(id) {
            return matches!(td.kind, TypeKind::Struct | TypeKind::Union).then(|| RecvTy::Local(id.to_string()));
        }
        let (owner, variant) = id.rsplit_once("::")?;
        let td = self.k.type_decl(owner)?;
        (td.kind == TypeKind::Enum && td.variants.iter().any(|v| v.name == variant)).then(|| RecvTy::Local(owner.to_string()))
    }

    /// The written return type of a scanned fn, resolved in the callee's
    /// own context (so `Self` and its module's names apply). The callee's
    /// generic parameters mean nothing at the call site: dropped.
    fn return_type_of(&self, fq: &str) -> Option<RecvTy> {
        let f = self.fns.get(fq)?;
        let ty: syn::Type = syn::parse_str(f.return_ty.as_deref()?).ok()?;
        let package = fq.split("::").next().unwrap_or_default().to_string();
        let module = join_key(&package, &f.module_path);
        let cx = Ctx { module: &module, package: &package, enclosing_fn: Some(fq), ns: Namespace::Any, cfg: None };
        strip_generics(self.type_of_syn(&ty, &cx))
    }

    fn type_of_callee(&self, id: &str) -> Option<RecvTy> {
        if let Some(rt) = self.type_of_value(id) {
            return Some(rt);
        }
        if self.fns.contains_key(id) {
            return self.return_type_of(id);
        }
        None
    }

    /// A path resolving to something klepto scanned, by id.
    fn scanned_id(&self, p: &syn::Path, cx: &FnCtx) -> Option<String> {
        match self.r.resolve_path(&seg_path(p), &cx.ctx()) {
            Resolution::Local(s) | Resolution::External(s) => Some(s),
            _ => None,
        }
    }

    /// The declared type of a field of `owner`, by name or index.
    fn type_of_field(&self, owner: &str, member: &syn::Member) -> Option<RecvTy> {
        let td = self.k.type_decl(owner)?;
        let f = match member {
            syn::Member::Named(i) => {
                let n = i.to_string();
                td.fields.iter().find(|f| f.name.as_deref() == Some(n.as_str()))?
            }
            syn::Member::Unnamed(i) => td.fields.iter().find(|f| f.index == i.index as usize)?,
        };
        let ty: syn::Type = syn::parse_str(&f.ty.raw).ok()?;
        let cx = Ctx { module: &td.module, package: &td.package, enclosing_fn: Some(&td.id), ns: Namespace::Any, cfg: None };
        strip_generics(self.type_of_syn(&ty, &cx))
    }

    /// Std constructors — `Some(x)`, `Ok(x)`, `Err(e)`, `None`,
    /// `Vec::new()`, `String::from(..)`, `HashMap::with_capacity(..)`, and
    /// the transparent `Box::new(x)`.
    fn std_call(&self, p: &syn::Path, args: &[D<'a>], cx: &FnCtx) -> Option<RecvTy> {
        let last = p.segments.last()?.ident.to_string();
        let first_arg = || args.first().and_then(|a| self.type_of(a, cx));
        if p.segments.len() == 1 {
            return match last.as_str() {
                "Some" => Some(opt(first_arg())),
                "Ok" => Some(res(first_arg(), None)),
                "Err" => Some(res(None, first_arg())),
                _ => None,
            };
        }
        let owner = &p.segments[p.segments.len() - 2];
        let owner_name = owner.ident.to_string();
        if TRANSPARENT.contains(&owner_name.as_str()) && matches!(last.as_str(), "new" | "pin" | "from") {
            return first_arg();
        }
        let mut owner_path = p.clone();
        owner_path.segments.pop();
        if let Some(pair) = owner_path.segments.pop() {
            owner_path.segments.push_value(pair.into_value());
        }
        let owner_ty = self.type_of_path(&owner_path, &cx.ctx())?;
        let RecvTy::Std { shape, name, args: targs } = owner_ty else { return None };
        match last.as_str() {
            "new" | "with_capacity" | "default" | "from" | "from_iter" | "from_str" | "with_hasher" | "with_capacity_and_hasher" | "from_utf8_lossy" => {
                if matches!(shape, Shape::Res) && last == "from_str" {
                    return Some(res(Some(RecvTy::Std { shape, name, args: targs }), None));
                }
                let args = if targs.iter().all(Option::is_none) && matches!(shape, Shape::Seq | Shape::Set) && last == "from" {
                    vec![first_arg().and_then(|t| iterable_elem(&t))]
                } else {
                    targs
                };
                Some(RecvTy::Std { shape, name, args })
            }
            "from_utf8" => Some(res(Some(RecvTy::Std { shape, name, args: targs }), None)),
            _ => None,
        }
    }

    fn type_of(&self, d: &D<'a>, cx: &FnCtx) -> Option<RecvTy> {
        let n = self.nesting.get();
        if n > MAX_NESTING {
            return None;
        }
        self.nesting.set(n + 1);
        let rt = self.type_of_inner(d, cx);
        self.nesting.set(n);
        rt
    }

    fn type_of_inner(&self, d: &D<'a>, cx: &FnCtx) -> Option<RecvTy> {
        match d {
            D::SelfRecv => self.self_type(cx),
            D::Ty(t) => self.type_of_syn(t, &cx.ctx()),
            D::Typed(rt) => Some(rt.clone()),
            D::Call(p, args) => {
                if let Some(id) = self.scanned_id(p, cx) {
                    if let Some(rt) = self.type_of_callee(&id) {
                        return Some(rt);
                    }
                }
                self.std_call(p, args, cx)
            }
            D::Value(p) | D::Lit(p) => {
                if let Some(id) = self.scanned_id(p, cx) {
                    if let Some(rt) = self.type_of_value(&id) {
                        return Some(rt);
                    }
                }
                (p.segments.len() == 1 && p.is_ident("None")).then(|| opt(None))
            }
            D::Method { recv, name, args, turbofish } => {
                // One level per call, left to right
                let rt = self.type_of(recv, cx)?;
                self.method_result(&rt, name, args, *turbofish, cx)
            }
            D::Closure { .. } => None,
            D::Field(base, m) => match self.type_of(base, cx)? {
                RecvTy::Local(t) => self.type_of_field(&t, m),
                RecvTy::Tuple(elems) => match m {
                    syn::Member::Unnamed(i) => elems.get(i.index as usize).cloned().flatten(),
                    _ => None,
                },
                _ => None,
            },
            D::Try(inner) => match self.type_of(inner, cx)? {
                RecvTy::Std { shape: Shape::Opt | Shape::Res, args, .. } => arg0(&args),
                _ => None,
            },
            D::Index(base) => match self.type_of(base, cx)? {
                RecvTy::Std { shape: Shape::Seq, args, .. } => arg0(&args),
                RecvTy::Std { shape: Shape::Map, args, .. } => arg1(&args),
                RecvTy::Std { shape: Shape::Str, .. } => Some(scalar("str")),
                _ => None,
            },
            D::Tuple(elems) => Some(RecvTy::Tuple(elems.iter().map(|e| self.type_of(e, cx)).collect())),
            D::Array(elem) => Some(std(Shape::Seq, "std::primitive::array", vec![self.type_of(elem, cx)])),
            D::Proj(base, proj) => {
                let rt = self.type_of(base, cx)?;
                match proj {
                    Proj::OptElem => match rt {
                        RecvTy::Std { shape: Shape::Opt, args, .. } => arg0(&args),
                        _ => None,
                    },
                    Proj::ResOk => match rt {
                        RecvTy::Std { shape: Shape::Res, args, .. } => arg0(&args),
                        _ => None,
                    },
                    Proj::ResErr => match rt {
                        RecvTy::Std { shape: Shape::Res, args, .. } => arg1(&args),
                        _ => None,
                    },
                    Proj::TupleIdx(i) => match rt {
                        RecvTy::Tuple(elems) => elems.get(*i).cloned().flatten(),
                        _ => None,
                    },
                    Proj::SeqElem => match rt {
                        RecvTy::Std { shape: Shape::Seq, args, .. } => arg0(&args),
                        _ => None,
                    },
                    Proj::IterItem => iterable_elem(&rt),
                    Proj::ClosureParam { method, arg, param, arity } => closure_param_type(&rt, method, *arg, *param, *arity),
                }
            }
            D::Unknown => None,
        }
    }

    /// What the closure passed as argument `i` produces — its written
    /// `-> T`, else its body (whose parameters were bound when described).
    fn closure_result(&self, args: &[D<'a>], i: usize, cx: &FnCtx) -> Option<RecvTy> {
        match args.get(i)? {
            D::Closure { body, ret } => match ret {
                Some(t) => self.type_of_syn(t, &cx.ctx()),
                None => self.type_of(body, cx),
            },
            _ => None,
        }
    }

    fn turbofish_type(&self, turbofish: Option<&'a syn::Type>, cx: &FnCtx) -> Option<RecvTy> {
        self.type_of_syn(turbofish?, &cx.ctx())
    }

    /// What `recv.method(..)` evaluates to.
    fn method_result(&self, rt: &RecvTy, name: &str, args: &[D<'a>], turbofish: Option<&'a syn::Type>, cx: &FnCtx) -> Option<RecvTy> {
        let same = || Some(rt.clone());
        match rt {
            RecvTy::Std { shape, args: targs, .. } => self.std_method(rt, shape, targs, name, args, turbofish, cx),
            RecvTy::Tuple(_) => match name {
                "clone" | "to_owned" | "as_ref" | "borrow" => same(),
                _ => None,
            },
            RecvTy::Local(_) | RecvTy::External(_) | RecvTy::Generic(_) => {
                if let Resolution::Local(id) = self.lookup_method(rt, name, cx) {
                    if let Some(out) = self.return_type_of(&id) {
                        return Some(out);
                    }
                }
                match name {
                    "clone" | "to_owned" | "as_ref" | "as_mut" | "borrow" | "borrow_mut" | "deref" | "deref_mut" | "cloned" | "copied" => same(),
                    "to_string" => Some(string()),
                    _ => None,
                }
            }
        }
    }

    /// The standard library's method results.
    #[allow(clippy::too_many_arguments)]
    fn std_method(&self, rt: &RecvTy, shape: &Shape, targs: &[Option<RecvTy>], name: &str, args: &[D<'a>], turbofish: Option<&'a syn::Type>, cx: &FnCtx) -> Option<RecvTy> {
        let elem = arg0(targs);
        let second = arg1(targs);
        let same = || Some(rt.clone());
        let closure = |i: usize| -> Option<RecvTy> { self.closure_result(args, i, cx) };
        // common to every shape
        match name {
            "clone" | "to_owned" | "as_ref" | "as_mut" | "by_ref" => return same(),
            // cells hand out their content on these
            "borrow" | "borrow_mut" | "deref" | "deref_mut" if !matches!(shape, Shape::Cell) => return same(),
            "to_string" => return Some(string()),
            _ => {}
        }
        match shape {
            Shape::Seq => Some(match name {
                "len" | "capacity" => scalar("usize"),
                "is_empty" | "contains" | "starts_with" | "ends_with" | "is_sorted" => scalar("bool"),
                "get" | "first" | "last" | "pop" | "get_mut" | "first_mut" | "last_mut" | "pop_front" | "pop_back" | "front" | "back" | "peek" => opt(elem),
                "remove" | "swap_remove" => elem?,
                "as_slice" | "as_mut_slice" | "to_vec" | "into_boxed_slice" | "into_vec" | "into_sorted_vec" => seq(elem),
                "iter" | "iter_mut" | "into_iter" | "drain" => iter(elem),
                "windows" | "chunks" | "chunks_exact" | "rchunks" | "split" | "splitn" | "chunks_mut" => iter(Some(seq(elem))),
                "binary_search" | "binary_search_by" | "binary_search_by_key" => res(Some(scalar("usize")), Some(scalar("usize"))),
                "split_at" | "split_at_mut" => tuple2(Some(seq(elem.clone())), Some(seq(elem))),
                "split_first" | "split_last" => opt(Some(tuple2(elem.clone(), Some(seq(elem))))),
                "concat" | "join" => match &elem {
                    Some(RecvTy::Std { shape: Shape::Str, .. }) => string(),
                    Some(RecvTy::Std { shape: Shape::Seq, args, .. }) => seq(arg0(args)),
                    _ => return None,
                },
                "partition_point" => scalar("usize"),
                _ => return None,
            }),
            Shape::Str => Some(match name {
                "len" | "capacity" => scalar("usize"),
                "is_empty" | "contains" | "starts_with" | "ends_with" | "eq_ignore_ascii_case" | "is_char_boundary" | "is_ascii" => scalar("bool"),
                "trim" | "trim_start" | "trim_end" | "trim_matches" | "trim_start_matches" | "trim_end_matches" | "to_lowercase" | "to_uppercase"
                | "to_ascii_lowercase" | "to_ascii_uppercase" | "as_str" | "replace" | "replacen" | "repeat" | "into_boxed_str" | "as_mut_str"
                | "into_string" | "to_str" | "escape_default" | "escape_debug" => string(),
                "strip_prefix" | "strip_suffix" | "get" | "get_mut" => opt(Some(string())),
                "chars" => iter(Some(scalar("char"))),
                "bytes" => iter(Some(scalar("u8"))),
                "lines" | "split" | "split_whitespace" | "rsplit" | "splitn" | "rsplitn" | "split_terminator" | "matches" | "split_inclusive"
                | "split_ascii_whitespace" => iter(Some(string())),
                "char_indices" => iter(Some(tuple2(Some(scalar("usize")), Some(scalar("char"))))),
                "parse" => res(self.turbofish_type(turbofish, cx), None),
                "find" | "rfind" => opt(Some(scalar("usize"))),
                "split_once" | "rsplit_once" => opt(Some(tuple2(Some(string()), Some(string())))),
                "as_bytes" | "into_bytes" | "as_bytes_mut" => std(Shape::Seq, "std::primitive::slice", vec![Some(scalar("u8"))]),
                "split_at" => tuple2(Some(string()), Some(string())),
                "pop" => opt(Some(scalar("char"))),
                "encode_utf16" => iter(Some(scalar("u16"))),
                _ => return None,
            }),
            Shape::Opt => Some(match name {
                "unwrap" | "expect" | "unwrap_or" | "unwrap_or_default" | "unwrap_or_else" | "unwrap_unchecked" | "get_or_insert" | "get_or_insert_with"
                | "insert" | "get_or_insert_default" => elem?,
                "as_deref" | "as_deref_mut" | "cloned" | "copied" | "take" | "replace" | "filter" | "or" | "or_else" | "xor" | "inspect" | "take_if" => opt(elem),
                "map" => opt(closure(0)),
                "and_then" => match closure(0) {
                    Some(RecvTy::Std { shape: Shape::Opt, args, .. }) => opt(arg0(&args)),
                    _ => opt(None),
                },
                "and" => opt(None),
                "ok_or" | "ok_or_else" => res(elem, None),
                "is_some" | "is_none" | "is_some_and" | "is_none_or" => scalar("bool"),
                "iter" | "iter_mut" | "into_iter" => iter(elem),
                "unzip" => match elem {
                    Some(RecvTy::Tuple(e)) => tuple2(Some(opt(e.first().cloned().flatten())), Some(opt(e.get(1).cloned().flatten()))),
                    _ => tuple2(Some(opt(None)), Some(opt(None))),
                },
                "zip" => opt(Some(tuple2(elem, None))),
                "flatten" => match elem {
                    Some(RecvTy::Std { shape: Shape::Opt, args, .. }) => opt(arg0(&args)),
                    _ => opt(None),
                },
                "map_or" | "map_or_else" => closure(1)?,
                _ => return None,
            }),
            Shape::Res => Some(match name {
                "unwrap" | "expect" | "unwrap_or" | "unwrap_or_default" | "unwrap_or_else" | "unwrap_unchecked" => elem?,
                "unwrap_err" | "expect_err" | "unwrap_err_unchecked" => second?,
                "ok" => opt(elem),
                "err" => opt(second),
                "map" => res(closure(0), second),
                "and_then" => match closure(0) {
                    Some(RecvTy::Std { shape: Shape::Res, args, .. }) => res(arg0(&args), second),
                    _ => res(None, second),
                },
                "map_err" => res(elem, closure(0)),
                "or_else" => match closure(0) {
                    Some(RecvTy::Std { shape: Shape::Res, args, .. }) => res(elem, arg1(&args)),
                    _ => res(elem, None),
                },
                "cloned" | "copied" | "inspect" | "inspect_err" | "as_deref" | "as_deref_mut" => res(elem, second),
                "and" => res(None, second),
                "or" => res(elem, None),
                "is_ok" | "is_err" | "is_ok_and" | "is_err_and" => scalar("bool"),
                "iter" | "iter_mut" | "into_iter" => iter(elem),
                "map_or" | "map_or_else" => closure(1)?,
                "flatten" => match elem {
                    Some(RecvTy::Std { shape: Shape::Res, args, .. }) => res(arg0(&args), second),
                    _ => res(None, second),
                },
                _ => return None,
            }),
            Shape::Map => Some(match name {
                "get" | "get_mut" | "remove" | "insert" => opt(second),
                "get_key_value" | "remove_entry" | "first_key_value" | "last_key_value" | "pop_first" | "pop_last" => opt(Some(tuple2(elem, second))),
                "contains_key" | "is_empty" => scalar("bool"),
                "len" | "capacity" => scalar("usize"),
                "keys" | "into_keys" => iter(elem),
                "values" | "values_mut" | "into_values" => iter(second),
                "iter" | "iter_mut" | "into_iter" | "drain" | "range" | "range_mut" => iter(Some(tuple2(elem, second))),
                "entry" => std(Shape::Entry, "std::collections::hash_map::Entry", vec![second]),
                _ => return None,
            }),
            Shape::Entry => Some(match name {
                "or_insert" | "or_insert_with" | "or_default" | "or_insert_with_key" => elem?,
                "and_modify" => rt.clone(),
                _ => return None,
            }),
            Shape::Set => Some(match name {
                "contains" | "is_empty" | "insert" | "remove" | "is_subset" | "is_superset" | "is_disjoint" => scalar("bool"),
                "len" | "capacity" => scalar("usize"),
                "iter" | "into_iter" | "drain" | "difference" | "union" | "intersection" | "symmetric_difference" | "range" => iter(elem),
                "get" | "take" | "first" | "last" | "pop_first" | "pop_last" | "replace" => opt(elem),
                _ => return None,
            }),
            Shape::Iter => Some(match name {
                "next" | "last" | "nth" | "min" | "max" | "find" | "max_by_key" | "min_by_key" | "max_by" | "min_by" | "reduce" | "next_back" | "peek"
                | "nth_back" | "peek_mut" | "next_if" | "next_if_eq" | "rfind" => opt(elem),
                "position" | "rposition" => opt(Some(scalar("usize"))),
                "find_map" => match closure(0) {
                    Some(RecvTy::Std { shape: Shape::Opt, args, .. }) => opt(arg0(&args)),
                    _ => opt(None),
                },
                "map" => iter(closure(0)),
                "filter_map" | "map_while" => match closure(0) {
                    Some(RecvTy::Std { shape: Shape::Opt, args, .. }) => iter(arg0(&args)),
                    _ => iter(None),
                },
                "flat_map" => iter(closure(0).and_then(|t| iterable_elem(&t))),
                "scan" => iter(match closure(1) {
                    Some(RecvTy::Std { shape: Shape::Opt, args, .. }) => arg0(&args),
                    _ => None,
                }),
                "filter" | "skip" | "take" | "rev" | "chain" | "skip_while" | "take_while" | "step_by" | "peekable" | "fuse" | "inspect" | "cloned"
                | "copied" | "cycle" | "into_iter" | "iter" | "sorted" | "dedup" | "intersperse" => iter(elem),
                "enumerate" => iter(Some(tuple2(Some(scalar("usize")), elem))),
                "zip" => iter(Some(tuple2(elem, args.first().and_then(|a| self.type_of(a, cx)).and_then(|t| iterable_elem(&t))))),
                "collect" => match self.turbofish_type(turbofish, cx) {
                    Some(RecvTy::Std { shape, name, args }) if args.iter().all(Option::is_none) => match shape {
                        Shape::Seq | Shape::Set => RecvTy::Std { shape, name, args: vec![elem] },
                        Shape::Map => match elem {
                            Some(RecvTy::Tuple(kv)) => RecvTy::Std { shape, name, args: kv },
                            _ => RecvTy::Std { shape, name, args },
                        },
                        Shape::Str => string(),
                        _ => RecvTy::Std { shape, name, args },
                    },
                    Some(t) => t,
                    None => return None,
                },
                "count" => scalar("usize"),
                "sum" | "product" => self.turbofish_type(turbofish, cx).or(elem)?,
                "any" | "all" | "is_sorted" | "eq" | "ne" | "lt" | "le" | "gt" | "ge" => scalar("bool"),
                "flatten" => iter(elem.as_ref().and_then(iterable_elem)),
                "unzip" => match elem {
                    Some(RecvTy::Tuple(e)) => tuple2(Some(seq(e.first().cloned().flatten())), Some(seq(e.get(1).cloned().flatten()))),
                    _ => tuple2(Some(seq(None)), Some(seq(None))),
                },
                "partition" => tuple2(Some(seq(elem.clone())), Some(seq(elem))),
                "fold" | "rfold" => args.first().and_then(|a| self.type_of(a, cx))?,
                "last_mut" => opt(elem),
                _ => return None,
            }),
            Shape::Cell => {
                let name_id = match rt {
                    RecvTy::Std { name, .. } => name.as_str(),
                    _ => "",
                };
                Some(match (name_id.rsplit("::").next().unwrap_or_default(), name) {
                    (_, "into_inner") => elem?,
                    ("RefCell", "borrow" | "borrow_mut" | "get_mut" | "take" | "replace") => elem?,
                    ("RefCell", "try_borrow" | "try_borrow_mut") => res(elem, None),
                    ("Cell", "get" | "take" | "replace" | "get_mut") => elem?,
                    ("Mutex" | "RwLock", "lock" | "read" | "write" | "try_lock" | "try_read" | "try_write") => res(elem, None),
                    ("Mutex" | "RwLock", "get_mut") => res(elem, None),
                    ("OnceCell" | "OnceLock", "get" | "get_mut" | "take") => opt(elem),
                    ("OnceCell" | "OnceLock", "get_or_init" | "get_or_try_init") => elem?,
                    ("LazyCell" | "LazyLock", "force" | "deref" | "deref_mut") => elem?,
                    ("RefCell" | "Cell" | "Mutex" | "RwLock" | "OnceCell" | "OnceLock" | "LazyCell" | "LazyLock", "borrow" | "borrow_mut" | "deref" | "deref_mut") => elem?,
                    _ => return None,
                })
            }
            Shape::Scalar => Some(match name {
                n if n.starts_with("checked_") => opt(Some(rt.clone())),
                n if n.starts_with("is_") => scalar("bool"),
                "overflowing_add" | "overflowing_sub" | "overflowing_mul" => tuple2(Some(rt.clone()), Some(scalar("bool"))),
                "to_digit" => opt(Some(scalar("u32"))),
                "len_utf8" | "len_utf16" | "count_ones" | "count_zeros" | "leading_zeros" | "trailing_zeros" | "ilog2" | "ilog10" => scalar("u32"),
                "cmp" | "partial_cmp" => return None,
                "as_bytes" => std(Shape::Seq, "std::primitive::slice", vec![Some(scalar("u8"))]),
                "chars" | "trim" | "split" | "lines" | "len" | "is_empty" | "parse" | "contains" | "starts_with" | "ends_with" | "to_lowercase"
                | "to_uppercase" | "bytes" | "split_whitespace" | "strip_prefix" | "strip_suffix" | "find" | "replace" | "repeat" | "as_str"
                    if name_is_str(rt) =>
                {
                    return self.std_method(&string(), &Shape::Str, &[], name, args, turbofish, cx)
                }
                _ => rt.clone(),
            }),
            Shape::Other => None,
        }
    }

    // ---- lookup ----

    /// Trait ids bounding generic parameter `g` in the current frame.
    fn generic_bounds(&self, g: &str, cx: &FnCtx) -> Vec<String> {
        let Some(f) = self.frames.last() else { return Vec::new() };
        let mut out = Vec::new();
        for gens in &f.generics {
            for p in &gens.params {
                if let syn::GenericParam::Type(tp) = p {
                    if tp.ident == g {
                        out.extend(self.bound_traits(tp.bounds.iter(), cx));
                    }
                }
            }
            if let Some(wc) = &gens.where_clause {
                for pred in &wc.predicates {
                    let syn::WherePredicate::Type(pt) = pred else { continue };
                    let syn::Type::Path(tp) = &pt.bounded_ty else { continue };
                    if tp.qself.is_none() && tp.path.is_ident(g) {
                        out.extend(self.bound_traits(pt.bounds.iter(), cx));
                    }
                }
            }
        }
        out
    }

    fn bound_traits<'b>(&self, bounds: impl Iterator<Item = &'b syn::TypeParamBound>, cx: &FnCtx) -> Vec<String> {
        bounds
            .filter_map(|b| match b {
                syn::TypeParamBound::Trait(tb) => match self.r.resolve_path(&seg_path(&tb.path), &cx.ctx()) {
                    Resolution::Local(s) | Resolution::External(s) => Some(s),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    fn lookup_method(&self, rt: &RecvTy, name: &str, cx: &FnCtx) -> Resolution {
        match rt {
            RecvTy::External(e) => Resolution::External(format!("{e}::{name}")),
            RecvTy::Std { name: id, .. } => Resolution::External(format!("{id}::{name}")),
            RecvTy::Tuple(_) => Resolution::External(format!("std::primitive::tuple::{name}")),
            RecvTy::Local(t) => {
                if let Some(td) = self.k.type_decl(t) {
                    if td.kind == TypeKind::Trait {
                        return td
                            .trait_methods
                            .iter()
                            .find(|m| m.name == name)
                            .map(|m| Resolution::Local(m.id.clone()))
                            .unwrap_or_else(|| unresolved(name, UnresolvedReason::MethodCall, Vec::new()));
                    }
                }
                let impls = self.k.impls_of(t);
                if let Some(m) = impls.iter().filter(|i| i.trait_.is_none()).flat_map(|i| i.methods.iter()).find(|m| last_segment(m) == name) {
                    return Resolution::Local(m.clone());
                }
                let cands: Vec<String> = impls
                    .iter()
                    .filter(|i| i.trait_.is_some())
                    .flat_map(|i| i.methods.iter())
                    .filter(|m| last_segment(m) == name)
                    .cloned()
                    .collect();
                match cands.len() {
                    0 => unresolved(name, UnresolvedReason::MethodCall, Vec::new()),
                    1 => Resolution::Local(cands.into_iter().next().unwrap_or_default()),
                    _ => unresolved(name, UnresolvedReason::Ambiguous, cands),
                }
            }
            RecvTy::Generic(g) => {
                let mut cands: Vec<String> = self
                    .generic_bounds(g, cx)
                    .iter()
                    .filter_map(|tr| self.k.type_decl(tr))
                    .filter(|td| td.kind == TypeKind::Trait)
                    .flat_map(|td| td.trait_methods.iter().filter(|m| m.name == name).map(|m| m.id.clone()))
                    .collect();
                cands.sort();
                cands.dedup();
                match cands.len() {
                    0 => unresolved(name, UnresolvedReason::MethodCall, Vec::new()),
                    1 => Resolution::Local(cands.into_iter().next().unwrap_or_default()),
                    _ => unresolved(name, UnresolvedReason::Ambiguous, cands),
                }
            }
        }
    }
}

fn name_is_str(rt: &RecvTy) -> bool {
    matches!(rt, RecvTy::Std { name, .. } if name == "std::primitive::str")
}

fn receiver_resolution(rt: &RecvTy) -> Resolution {
    match rt {
        RecvTy::Local(l) => Resolution::Local(l.clone()),
        RecvTy::External(e) => Resolution::External(e.clone()),
        RecvTy::Std { name, .. } => Resolution::External(name.clone()),
        RecvTy::Tuple(_) => Resolution::External("std::primitive::tuple".into()),
        RecvTy::Generic(g) => unresolved(g, UnresolvedReason::GenericParam, Vec::new()),
    }
}

impl<'a, 'k> Visit<'a> for W<'a, 'k> {
    // the occurrence visitor never descends into attributes
    fn visit_attribute(&mut self, _a: &'a syn::Attribute) {}

    fn visit_item_impl(&mut self, i: &'a syn::ItemImpl) {
        self.outer_generics.push(&i.generics);
        syn::visit::visit_item_impl(self, i);
        self.outer_generics.pop();
    }

    fn visit_item_trait(&mut self, i: &'a syn::ItemTrait) {
        self.outer_generics.push(&i.generics);
        syn::visit::visit_item_trait(self, i);
        self.outer_generics.pop();
    }

    fn visit_item_fn(&mut self, f: &'a syn::ItemFn) {
        self.enter(&f.sig);
        syn::visit::visit_item_fn(self, f);
        self.leave();
    }

    fn visit_impl_item_fn(&mut self, f: &'a syn::ImplItemFn) {
        self.enter(&f.sig);
        syn::visit::visit_impl_item_fn(self, f);
        self.leave();
    }

    fn visit_trait_item_fn(&mut self, f: &'a syn::TraitItemFn) {
        self.enter(&f.sig);
        syn::visit::visit_trait_item_fn(self, f);
        self.leave();
    }

    fn visit_block(&mut self, b: &'a syn::Block) {
        self.push_scope();
        syn::visit::visit_block(self, b);
        self.pop_scope();
    }

    fn visit_local(&mut self, l: &'a syn::Local) {
        // the initializer sees the bindings before this `let`
        syn::visit::visit_local(self, l);
        let init = l.init.as_ref().map(|init| self.descriptor(&init.expr)).unwrap_or(D::Unknown);
        match &l.pat {
            syn::Pat::Ident(pi) => self.bind(pi.ident.to_string(), init),
            syn::Pat::Type(pt) => match &*pt.pat {
                syn::Pat::Ident(pi) => self.bind(pi.ident.to_string(), D::Ty(&pt.ty)),
                other => self.bind_pattern(other, D::Ty(&pt.ty)),
            },
            other => self.bind_pattern(other, init),
        }
    }

    fn visit_expr_match(&mut self, m: &'a syn::ExprMatch) {
        let d = self.descriptor(&m.expr);
        self.scrutinees.push(d);
        syn::visit::visit_expr_match(self, m);
        self.scrutinees.pop();
    }

    fn visit_arm(&mut self, a: &'a syn::Arm) {
        self.push_scope();
        match self.scrutinees.last().cloned() {
            Some(d) => self.bind_pattern(&a.pat, d),
            None => self.bind_unknown(&a.pat),
        }
        syn::visit::visit_arm(self, a);
        self.pop_scope();
    }

    fn visit_expr_closure(&mut self, c: &'a syn::ExprClosure) {
        self.push_scope();
        let site = self.closure_sites.get(&span_key(c)).cloned().flatten();
        let arity = c.inputs.len();
        for (i, p) in c.inputs.iter().enumerate() {
            match p {
                syn::Pat::Type(pt) => match &*pt.pat {
                    syn::Pat::Ident(pi) => self.bind(pi.ident.to_string(), D::Ty(&pt.ty)),
                    other => self.bind_pattern(other, D::Ty(&pt.ty)),
                },
                other => match &site {
                    // `|x|` of `recv.map(..)` is an item of `recv`
                    Some(s) => {
                        let d = D::Proj(Box::new(s.recv.clone()), Proj::ClosureParam { method: s.method.clone(), arg: s.arg, param: i, arity });
                        self.bind_pattern(other, d)
                    }
                    None => self.bind_unknown(other),
                },
            }
        }
        syn::visit::visit_expr_closure(self, c);
        self.pop_scope();
    }

    fn visit_expr_for_loop(&mut self, f: &'a syn::ExprForLoop) {
        self.push_scope();
        let d = D::Proj(Box::new(self.descriptor(&f.expr)), Proj::IterItem);
        self.bind_pattern(&f.pat, d);
        syn::visit::visit_expr_for_loop(self, f);
        self.pop_scope();
    }

    fn visit_expr_let(&mut self, l: &'a syn::ExprLet) {
        syn::visit::visit_expr_let(self, l);
        // `if let` / `while let` bindings: shadow for the rest of the block
        let d = self.descriptor(&l.expr);
        self.bind_pattern(&l.pat, d);
    }

    fn visit_expr_method_call(&mut self, m: &'a syn::ExprMethodCall) {
        let recv = self.descriptor(&m.receiver);
        if let Some(call) = self.calls.get(self.next).copied() {
            let cx = self.fn_ctx(call);
            let d = recv.clone();
            let name = m.method.to_string();
            let result = match self.type_of(&d, &cx) {
                Some(rt) => (Some(receiver_resolution(&rt)), Some(self.lookup_method(&rt, &name, &cx))),
                None => (None, None),
            };
            self.out.push(result);
        }
        self.next += 1;
        // Remember where closure arguments sit
        for (idx, a) in m.args.iter().enumerate() {
            if let syn::Expr::Closure(c) = a {
                let site = ClosureSite { recv: recv.clone(), method: m.method.to_string(), arg: idx };
                self.closure_sites.entry(span_key(c)).and_modify(|e| *e = None).or_insert(Some(site));
            }
        }
        syn::visit::visit_expr_method_call(self, m);
    }
}

/// Fills `CallOccurrence::receiver_type` and, where
/// the method is found, replaces the call's `resolution`.
pub(crate) fn resolve_method_calls(k: &mut Klepto) {
    let r = build_resolver(k);
    let parts = CtxParts::new(k);
    let fns: HashMap<&str, &CapturedFn> = k.functions.iter().map(|f| (f.fq_name.as_str(), f)).collect();

    let mut by_file: HashMap<&Path, Vec<usize>> = HashMap::new();
    for (i, c) in k.calls.iter().enumerate() {
        if c.is_method {
            by_file.entry(c.location.path.as_path()).or_default().push(i);
        }
    }

    let mut results: Vec<(usize, Option<Resolution>, Option<Resolution>)> = Vec::new();
    for pf in &k.files {
        let idx = by_file.get(pf.path.as_path()).cloned().unwrap_or_default();
        let calls: Vec<&CallOccurrence> = idx.iter().map(|&i| &k.calls[i]).collect();
        let mut w = W {
            k,
            r: &r,
            fns: &fns,
            parts: &parts,
            calls: &calls,
            next: 0,
            out: Vec::new(),
            frames: Vec::new(),
            outer_generics: Vec::new(),
            scrutinees: Vec::new(),
            closure_sites: HashMap::new(),
            overlay: RefCell::new(Vec::new()),
            nesting: Cell::new(0),
        };
        w.visit_file(&pf.ast);
        if w.next != calls.len() {
            continue; // traversal disagreement: leave the file alone
        }
        for (j, (rt, res)) in w.out.into_iter().enumerate() {
            results.push((idx[j], rt, res));
        }
    }

    for (i, rt, res) in results {
        let c = &mut k.calls[i];
        c.receiver_type = rt;
        if let Some(res) = res {
            let bare_unknown = matches!(&res, Resolution::Unresolved { reason: UnresolvedReason::MethodCall, candidates, .. } if candidates.is_empty());
            if !bare_unknown {
                c.resolution = Some(res);
            }
        }
    }
}

/// The names a pattern binds, each to the part of `d` it names —
/// `Some(x)` / `Ok(x)` / `Err(e)`, tuple positions, struct and tuple-struct
/// fields, slice elements, references, `@` bindings, the first `|` case.
/// What cannot be projected binds Unknown.
fn pattern_bindings<'a>(pat: &'a syn::Pat, d: D<'a>) -> Vec<(String, D<'a>)> {
    let mut out = Vec::new();
    pattern_bindings_into(pat, d, &mut out);
    out
}

fn pattern_bindings_into<'a>(pat: &'a syn::Pat, d: D<'a>, out: &mut Vec<(String, D<'a>)>) {
    let unknown = |p: &syn::Pat, out: &mut Vec<(String, D<'a>)>| {
        for n in pat_idents(p) {
            out.push((n, D::Unknown));
        }
    };
    match pat {
        syn::Pat::Ident(pi) => {
            out.push((pi.ident.to_string(), d.clone()));
            if let Some((_, sub)) = &pi.subpat {
                pattern_bindings_into(sub, d, out);
            }
        }
        syn::Pat::Type(pt) => match &*pt.pat {
            syn::Pat::Ident(pi) => out.push((pi.ident.to_string(), D::Ty(&pt.ty))),
            other => pattern_bindings_into(other, D::Ty(&pt.ty), out),
        },
        syn::Pat::Reference(r) => pattern_bindings_into(&r.pat, d, out),
        syn::Pat::Paren(p) => pattern_bindings_into(&p.pat, d, out),
        syn::Pat::Or(o) => {
            if let Some(first) = o.cases.first() {
                pattern_bindings_into(first, d, out);
            }
        }
        syn::Pat::TupleStruct(ts) => {
            let last = ts.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default();
            let single = ts.path.segments.len() == 1;
            let proj = match last.as_str() {
                "Some" if single => Some(Proj::OptElem),
                "Ok" if single => Some(Proj::ResOk),
                "Err" if single => Some(Proj::ResErr),
                _ => None,
            };
            match proj {
                Some(p) if ts.elems.len() == 1 => pattern_bindings_into(&ts.elems[0], D::Proj(Box::new(d), p), out),
                _ => {
                    // a tuple struct of a scanned type: field i; a variant: nothing known
                    for (i, e) in ts.elems.iter().enumerate() {
                        if matches!(e, syn::Pat::Rest(_)) {
                            break;
                        }
                        let member = syn::Member::Unnamed(syn::Index { index: i as u32, span: proc_macro2::Span::call_site() });
                        pattern_bindings_into(e, D::Field(Box::new(d.clone()), member), out);
                    }
                }
            }
        }
        syn::Pat::Tuple(t) => {
            for (i, e) in t.elems.iter().enumerate() {
                if matches!(e, syn::Pat::Rest(_)) {
                    for rest in t.elems.iter().skip(i + 1) {
                        unknown(rest, out);
                    }
                    break;
                }
                pattern_bindings_into(e, D::Proj(Box::new(d.clone()), Proj::TupleIdx(i)), out);
            }
        }
        syn::Pat::Struct(s) => {
            for f in &s.fields {
                pattern_bindings_into(&f.pat, D::Field(Box::new(d.clone()), f.member.clone()), out);
            }
        }
        syn::Pat::Slice(s) => {
            for e in &s.elems {
                match e {
                    syn::Pat::Rest(_) => {}
                    syn::Pat::Ident(pi) if pi.subpat.as_ref().is_some_and(|(_, p)| matches!(**p, syn::Pat::Rest(_))) => {
                        out.push((pi.ident.to_string(), d.clone()));
                    }
                    other => pattern_bindings_into(other, D::Proj(Box::new(d.clone()), Proj::SeqElem), out),
                }
            }
        }
        syn::Pat::Wild(_) | syn::Pat::Lit(_) | syn::Pat::Range(_) | syn::Pat::Rest(_) | syn::Pat::Path(_) | syn::Pat::Const(_) => {}
        other => unknown(other, out),
    }
}
