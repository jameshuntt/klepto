use crate::resolve::cfg::CfgPredicate;
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
    /// plus those of the enclosing impl or trait.
    #[serde(default)]
    pub generics: Vec<String>,
    /// Effective cfg: own `#[cfg]` and every enclosing one.
    #[serde(default)]
    pub cfg: CfgPredicate,
    /// The macro that generated this item (`name!`), if any.
    #[serde(default)]
    pub generated: Option<String>,
}

impl CapturedFn {
    pub fn is_public(&self) -> bool { self.is_public }
    pub fn has_docs(&self) -> bool { self.has_docs }
}

/// Every item declaration of every kind, as the
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
    /// The macro that generated this item (`name!`), if any.
    #[serde(default)]
    pub generated: Option<String>,
}

// ---------------------------------------------------------------------------
// Resolution
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
    /// A `.rs` file no `mod` declaration reaches.
    pub orphan: bool,
    /// Names defined directly in this module and what they are.
    pub items: BTreeMap<String, ItemKind>,
    /// Files claimed by other `#[cfg]`-gated declarations of the same module
    ///; the first declaration wins `file`.
    pub alternates: Vec<PathBuf>,
    /// `#[macro_use]` on this module's declaration: its `macro_rules!`
    /// names are visible in the parent module too.
    #[serde(default)]
    pub macro_use: bool,
    /// Effective cfg of the module: the `mod` declaration's `#[cfg]` and its
    /// parents'.
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
    /// Innermost enclosing item id; `None` at module level.
    #[serde(default)]
    pub enclosing_item: Option<String>,
    /// What the `use` target names.
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
    /// Twin coverage (see `PathOccurrence::covered`).
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
    /// Innermost enclosing item id; `None` at module level.
    #[serde(default)]
    pub enclosing_item: Option<String>,
    #[serde(default)]
    pub resolution: Option<Resolution>,
    #[serde(default)]
    pub cfg: CfgPredicate,
    /// Paths written inside the macro's tokens (`a::b` and single
    /// identifiers), as written: an unexpanded macro's arguments are tokens,
    /// so these are named there but never resolved.
    #[serde(default)]
    pub token_paths: Vec<String>,
}

/// The call (or method call) a literal or a path is written in the
/// arguments of: the innermost one, the argument expression searched whole.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArgOf {
    /// The callee as written (`std::env::var`, `env_or`), or the method name.
    pub callee: String,
    pub is_method: bool,
    /// The call's location, the same as its `CallOccurrence`'s.
    pub location: FileLocation,
    /// The call's resolution, copied from its `CallOccurrence` once resolved.
    #[serde(default)]
    pub resolution: Option<Resolution>,
}

/// A string literal (`"NODEPLAN_X"`, `r"..."`) in code: in an expression,
/// a `const`/`static` initialiser, or an unexpanded macro's tokens.
/// Attributes (doc comments included) are not code and are not recorded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiteralOccurrence {
    pub value: String,
    pub module_path: Vec<String>,
    pub location: FileLocation,
    #[serde(default)]
    pub enclosing_fn: Option<String>,
    /// Innermost enclosing item id (the `const` it initialises, the fn it
    /// sits in); `None` at module level.
    #[serde(default)]
    pub enclosing_item: Option<String>,
    #[serde(default)]
    pub arg_of: Option<ArgOf>,
    /// The macro whose tokens hold it (`format`, `println`), when klepto
    /// did not expand that macro.
    #[serde(default)]
    pub in_macro: Option<String>,
    #[serde(default)]
    pub cfg: CfgPredicate,
}

/// Which namespace a path is looked up in: a type position, a
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
    /// Resolved to one of cfg-twin bindings that together cover
    /// every build; the reference edge is exempt from cfg checking.
    #[serde(default)]
    pub covered: bool,
    pub module_path: Vec<String>,
    pub location: FileLocation,
    #[serde(default)]
    pub enclosing_fn: Option<String>,
    #[serde(default)]
    pub enclosing_public: Option<bool>,
    /// Innermost enclosing item id; `None` at module level.
    #[serde(default)]
    pub enclosing_item: Option<String>,
    #[serde(default)]
    pub resolution: Option<Resolution>,
    #[serde(default)]
    pub cfg: CfgPredicate,
    /// The namespace of the position the path is written in.
    #[serde(default)]
    pub ns: Namespace,
    /// The call it is written in the arguments of, if any.
    #[serde(default)]
    pub arg_of: Option<ArgOf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallOccurrence {
    pub callee: String,
    /// Twin coverage (see `PathOccurrence::covered`).
    #[serde(default)]
    pub covered: bool,
    pub module_path: Vec<String>,
    pub location: FileLocation,
    #[serde(default)]
    pub enclosing_fn: Option<String>,
    #[serde(default)]
    pub enclosing_public: Option<bool>,
    /// Innermost enclosing item id; `None` at module level.
    #[serde(default)]
    pub enclosing_item: Option<String>,
    /// `x.foo()` rather than `foo(..)` / `path::to::foo(..)`.
    #[serde(default)]
    pub is_method: bool,
    /// Resolution of the callee path for `path::to::f(..)` calls; for method
    /// calls, the method node when klepto can type the receiver, else
    /// `Unresolved { reason: MethodCall }`.
    #[serde(default)]
    pub resolution: Option<Resolution>,
    /// Method calls: the receiver's type when known.
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
    ///.
    pub arg_paths: Vec<String>,
    pub module_path: Vec<String>,
    pub location: FileLocation,
    #[serde(default)]
    pub enclosing_fn: Option<String>,
    #[serde(default)]
    pub enclosing_public: Option<bool>,
    /// Innermost enclosing item id; `None` at module level.
    #[serde(default)]
    pub enclosing_item: Option<String>,
    #[serde(default)]
    pub resolution: Option<Resolution>,
    /// Parallel to `arg_paths`.
    #[serde(default)]
    pub arg_resolutions: Vec<Resolution>,
    /// For `#[cfg(..)]` attributes: the parsed predicate.
    #[serde(default)]
    pub predicate: Option<CfgPredicate>,
    /// The argument tokens as written (`secret`, `"platform"`, `max = "2s"`),
    /// for a consumer that reads an attribute's values rather than its paths.
    #[serde(default)]
    pub args: String,
    /// Effective cfg at the attribute (for an attribute produced by
    /// `cfg_attr`, the `cfg_attr` condition is included).
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

/// A `match` expression: where it is, what it matches on as written, and
/// each arm's pattern paths with klepto's resolution of them (the enum
/// variants, constants and structs an arm names).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchOccurrence {
    pub module_path: Vec<String>,
    pub location: FileLocation,
    #[serde(default)]
    pub enclosing_fn: Option<String>,
    #[serde(default)]
    pub enclosing_item: Option<String>,
    /// The scrutinee's tokens, at most 160 characters.
    pub scrutinee: String,
    pub arms: Vec<MatchArm>,
    #[serde(default)]
    pub cfg: CfgPredicate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchArm {
    pub location: FileLocation,
    /// `_`, or a lowercase binding with no sub-pattern: the arm takes
    /// whatever the earlier arms did not.
    pub catch_all: bool,
    pub paths: Vec<ArmPath>,
}

/// A path written in an arm's pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArmPath {
    pub path: String,
    pub location: FileLocation,
    /// Copied from the path's own occurrence once resolved.
    #[serde(default)]
    pub resolution: Option<Resolution>,
}

/// The files a scan would read and their digests, without parsing: what
/// changed between two readings of the same code, cheaply.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Fingerprint {
    /// Every `.rs` file the scan reads and every manifest it reads, with the
    /// blake3 of its bytes (hex), sorted by path.
    pub files: Vec<(PathBuf, String)>,
    /// blake3 over `path NUL hash LF` of every file, in order.
    pub digest: String,
}

/// A workspace member as cargo metadata states it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageInfo {
    /// `-` → `_`, as every other record names packages.
    pub name: String,
    /// The name as `Cargo.toml` spells it.
    pub cargo_name: String,
    pub version: String,
    pub manifest_path: PathBuf,
    /// The directory holding the manifest.
    pub root: PathBuf,
    /// Lib, bin, test, example, bench and build-script targets.
    pub targets: Vec<TargetInfo>,
}

/// One cargo target of a package.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TargetInfo {
    pub name: String,
    /// `lib`, `bin`, `proc-macro`, `test`, `example`, `bench`, `custom-build`, ...
    pub kind: String,
    pub src_path: PathBuf,
}

impl PackageInfo {
    /// The names of its `bin` targets, sorted.
    pub fn bins(&self) -> Vec<String> {
        let mut v: Vec<String> = self.targets.iter().filter(|t| t.kind == "bin").map(|t| t.name.clone()).collect();
        v.sort();
        v
    }
}
pub type UseSitesCount = UseSites;
