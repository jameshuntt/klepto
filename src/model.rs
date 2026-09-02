use crate::cfg::CfgPredicate;
use crate::graph::NodeKind;
use globset::GlobSetBuilder;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct FileLocation {
    pub path: PathBuf,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FnKind {
    FreeFn,
    ImplMethod {
        self_ty: String,
        trait_ty: Option<String>,
    },
    TraitMethod {
        trait_name: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedFn {
    pub name: String,
    pub fq_name: String, // package::module::Type::method
    pub is_public: bool,
    pub has_docs: bool,

    pub is_async: bool,
    pub is_unsafe: bool,
    pub is_const: bool,
    pub is_generic: bool,

    pub args: Vec<String>,
    pub return_ty: Option<String>,

    pub kind: FnKind,
    pub module_path: Vec<String>,
    pub attrs: Vec<String>,
    pub signature: String,
    pub location: FileLocation,
    /// Type and const parameter names in scope inside the body: the fn's own
    /// plus those of the enclosing impl or trait (rule S5).
    #[serde(default)]
    pub generics: Vec<String>,
    /// Effective cfg: own `#[cfg]` and every enclosing one (rule E1).
    #[serde(default)]
    pub cfg: CfgPredicate,
    /// Rule X6: the macro that generated this item (`name!`), if any.
    #[serde(default)]
    pub generated: Option<String>,
}

impl CapturedFn {
    pub fn is_public(&self) -> bool { self.is_public }
    pub fn has_docs(&self) -> bool { self.has_docs }
}

/// Every item declaration of every kind (Phase 3, rule N1), as the
/// extractors see it; the graph builder turns these into nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemDecl {
    /// Canonical id, same spelling as graph node ids.
    pub id: String,
    pub kind: NodeKind,
    pub name: String,
    pub package: String,
    /// Module key (`app::store`).
    pub module: String,
    /// The enclosing impl / trait / fn id for nested declarations; `None` at
    /// module level.
    pub parent: Option<String>,
    pub location: FileLocation,
    pub cfg: CfgPredicate,
    pub is_public: bool,
    /// Rule X6: the macro that generated this item (`name!`), if any.
    #[serde(default)]
    pub generated: Option<String>,
}

// ---------------------------------------------------------------------------
// Resolution (Phase 1) — see PHASE1_RULES.md
// ---------------------------------------------------------------------------

/// What a path spelled in source actually names, once module tree and
/// `use` aliases are applied. Strings are canonical: `app::query::run`,
/// `regexish::Regex::new`, `std::println`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Resolution {
    /// An item of the scanned crate; the string starts with the package name.
    Local(String),
    /// An item of another crate (dependencies, `std`/`core`/`alloc`).
    External(String),
    /// Could not be resolved. `raw` is the source spelling; `candidates` are
    /// the canonical paths it might be (glob imports), possibly empty.
    Unresolved {
        raw: String,
        reason: UnresolvedReason,
        candidates: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum UnresolvedReason {
    /// The name can only come from a glob import whose target is not indexed.
    GlobImport,
    /// More than one glob import could supply the name.
    Ambiguous,
    /// Nothing in scope binds the first segment.
    UnknownName,
    /// `Self` outside a known impl.
    SelfType,
    /// The first segment is a generic parameter in scope.
    GenericParam,
    /// A method call (`x.foo()`): needs the receiver's type.
    MethodCall,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ItemKind {
    Fn,
    Struct,
    Enum,
    Union,
    Trait,
    TypeAlias,
    Const,
    Static,
    Mod,
    Macro,
    /// A name bound by a `use` item in this module (re-export or private).
    Use,
}

/// One module of a scanned package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleInfo {
    /// Canonical path: `app`, `app::query`, `app::engine::parts`.
    pub path: String,
    pub package: String,
    /// `path` without the package: `["engine", "parts"]`; empty for the root.
    pub segments: Vec<String>,
    /// The file holding the module's items (for inline modules: the file the
    /// `mod x { }` block sits in).
    pub file: PathBuf,
    pub inline: bool,
    /// The file containing the `mod x;` / `mod x { }` declaration; `None`
    /// for package roots and orphans.
    pub declared_in: Option<PathBuf>,
    pub is_pub: bool,
    /// A `.rs` file no `mod` declaration reaches (rule M5).
    pub orphan: bool,
    /// Names defined directly in this module and what they are.
    pub items: BTreeMap<String, ItemKind>,
    /// Files claimed by other `#[cfg]`-gated declarations of the same module
    /// (rule M6); the first declaration wins `file`.
    pub alternates: Vec<PathBuf>,
    /// `#[macro_use]` on this module's declaration (rule R12): its `macro_rules!`
    /// names are visible in the parent module too.
    #[serde(default)]
    pub macro_use: bool,
    /// Effective cfg of the module: the `mod` declaration's `#[cfg]` and its
    /// parents' (rule E2).
    #[serde(default)]
    pub cfg: CfgPredicate,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModuleTree {
    /// Keyed by canonical path (`app::query`).
    pub modules: BTreeMap<String, ModuleInfo>,
}

impl ModuleTree {
    pub fn get(&self, path: &str) -> Option<&ModuleInfo> {
        self.modules.get(path)
    }

    /// The (non-inline) module a file holds.
    pub fn for_file(&self, file: &Path) -> Option<&ModuleInfo> {
        self.modules.values().find(|m| !m.inline && m.file == file)
    }
}

// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum UseKind {
    Name,
    Glob,
    Rename { alias: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StolenPath {
    pub root: String,
    pub segments: Vec<String>,

    pub module_path: Vec<String>,
    pub is_internal: bool,
    pub is_public_use: bool,
    pub kind: UseKind,
    pub full_path: String,
    pub location: FileLocation,

    #[serde(default)]
    pub origin: Option<ImportOrigin>,
    #[serde(default)]
    pub is_absolute: Option<bool>,
    /// Set when the `use` item sits inside a function body (or any block)
    /// rather than at module level.
    #[serde(default)]
    pub enclosing_fn: Option<String>,
    /// Innermost enclosing item id (rule N5); `None` at module level.
    #[serde(default)]
    pub enclosing_item: Option<String>,
    /// What the `use` target names (Phase 1).
    #[serde(default)]
    pub resolution: Option<Resolution>,
    #[serde(default)]
    pub cfg: CfgPredicate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedSymbol {
    pub exported_as: String,     // name visible in public surface
    pub source_path: String,     // crate::x::y or external::path::Thing
    pub module_path: Vec<String>,
    pub location: FileLocation,
    #[serde(default)]
    pub resolution: Option<Resolution>,
    #[serde(default)]
    pub cfg: CfgPredicate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroDef {
    pub name: String,
    pub module_path: Vec<String>,
    pub location: FileLocation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroInvocation {
    pub name: String,
    /// Rule R14 (see `PathOccurrence::covered`).
    #[serde(default)]
    pub covered: bool,
    pub module_path: Vec<String>,
    #[serde(default)]
    pub path: Option<String>,
    pub location: FileLocation,
    #[serde(default)]
    pub enclosing_fn: Option<String>,
    #[serde(default)]
    pub enclosing_public: Option<bool>,
    /// Innermost enclosing item id (rule N5); `None` at module level.
    #[serde(default)]
    pub enclosing_item: Option<String>,
    #[serde(default)]
    pub resolution: Option<Resolution>,
    #[serde(default)]
    pub cfg: CfgPredicate,
}

/// Which namespace a path is looked up in (rule R15): a type position, a
/// value position (expression, tuple-struct pattern), or unknown.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum Namespace {
    #[default]
    Any,
    Type,
    Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathOccurrence {
    pub path: String,
    /// Rule R14: resolved to one of cfg-twin bindings that together cover
    /// every build; the reference edge is exempt from cfg checking.
    #[serde(default)]
    pub covered: bool,
    pub module_path: Vec<String>,
    pub location: FileLocation,
    #[serde(default)]
    pub enclosing_fn: Option<String>,
    #[serde(default)]
    pub enclosing_public: Option<bool>,
    /// Innermost enclosing item id (rule N5); `None` at module level.
    #[serde(default)]
    pub enclosing_item: Option<String>,
    #[serde(default)]
    pub resolution: Option<Resolution>,
    #[serde(default)]
    pub cfg: CfgPredicate,
    /// Rule R15: the namespace of the position the path is written in.
    #[serde(default)]
    pub ns: Namespace,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallOccurrence {
    pub callee: String,
    /// Rule R14 (see `PathOccurrence::covered`).
    #[serde(default)]
    pub covered: bool,
    pub module_path: Vec<String>,
    pub location: FileLocation,
    #[serde(default)]
    pub enclosing_fn: Option<String>,
    #[serde(default)]
    pub enclosing_public: Option<bool>,
    /// Innermost enclosing item id (rule N5); `None` at module level.
    #[serde(default)]
    pub enclosing_item: Option<String>,
    /// `x.foo()` rather than `foo(..)` / `path::to::foo(..)`.
    #[serde(default)]
    pub is_method: bool,
    /// Resolution of the callee path for `path::to::f(..)` calls; for method
    /// calls, the method node when Phase 5 can type the receiver, else
    /// `Unresolved { reason: MethodCall }`.
    #[serde(default)]
    pub resolution: Option<Resolution>,
    /// Method calls: the receiver's type when known (rules M1–M6).
    #[serde(default)]
    pub receiver_type: Option<Resolution>,
    #[serde(default)]
    pub cfg: CfgPredicate,
}

/// An attribute such as `#[derive(serde::Serialize)]`, `#[serde(rename = "x")]`
/// or `#[tokio::main]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttrOccurrence {
    /// The attribute's own path: `derive`, `serde`, `tokio::main`.
    pub path: String,
    /// Paths found inside the attribute's arguments, e.g. `serde::Serialize`
    /// in `#[derive(serde::Serialize)]`, including single identifiers
    /// (rule O4).
    pub arg_paths: Vec<String>,
    pub module_path: Vec<String>,
    pub location: FileLocation,
    #[serde(default)]
    pub enclosing_fn: Option<String>,
    #[serde(default)]
    pub enclosing_public: Option<bool>,
    /// Innermost enclosing item id (rule N5); `None` at module level.
    #[serde(default)]
    pub enclosing_item: Option<String>,
    #[serde(default)]
    pub resolution: Option<Resolution>,
    /// Parallel to `arg_paths`.
    #[serde(default)]
    pub arg_resolutions: Vec<Resolution>,
    /// For `#[cfg(..)]` attributes: the parsed predicate (rule C1).
    #[serde(default)]
    pub predicate: Option<CfgPredicate>,
    /// Effective cfg at the attribute (for an attribute produced by
    /// `cfg_attr`, the `cfg_attr` condition is included — rule E5).
    #[serde(default)]
    pub cfg: CfgPredicate,
}

/// `extern crate foo;` / `extern crate foo as bar;`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternCrateOccurrence {
    pub name: String,
    pub alias: Option<String>,
    pub module_path: Vec<String>,
    pub location: FileLocation,
    #[serde(default)]
    pub cfg: CfgPredicate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warn,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub severity: Severity,
    pub code: String,           // stable rule code: KLEP001
    pub message: String,
    pub location: FileLocation,
    pub extra: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocCoverage {
    pub public_total: usize,
    pub public_documented: usize,
    pub percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicSurface {
    pub exports: Vec<ExportedSymbol>,
}

/// Newtype so `KleptoBuilder` can derive `Default`.
pub(crate) struct KleptoGlobSetBuilder(pub GlobSetBuilder);

impl Default for KleptoGlobSetBuilder {
    fn default() -> Self {
        Self(GlobSetBuilder::new())
    }
}

impl std::ops::Deref for KleptoGlobSetBuilder {
    type Target = GlobSetBuilder;
    fn deref(&self) -> &Self::Target { &self.0 }
}
impl std::ops::DerefMut for KleptoGlobSetBuilder {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ImportOrigin {
    Internal,
    Std,
    Core,
    Alloc,
    WorkspaceMember,
    Dependency,
    UnknownExternal,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum UseSiteKind {
    UseStmt,       // `use dep::foo`
    ExternCrate,   // `extern crate dep;`
    Attribute,     // `#[dep::something]`, `#[derive(dep::Thing)]`
    MacroCall,     // `dep::foo!()`
    Path,          // everything else `dep::foo::bar`
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UseSite {
    pub dep: String,     // "regex"
    pub path: String,    // "regex::RegexSet"
    pub head: String,    // "RegexSet" (first segment after dep::)
    pub kind: UseSiteKind,
    pub location: FileLocation,     // 1-based
    pub scope: String,   // enclosing fn fq_name, else module path ("app::query")
}
pub type UseSites = BTreeMap<String, BTreeMap<String, usize>>;
pub type UseSitesCount = UseSites;
