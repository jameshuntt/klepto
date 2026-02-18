use globset::GlobSetBuilder;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub fq_name: String, // crate::mod::Type::method
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
}

impl CapturedFn {
    pub fn is_public(&self) -> bool { self.is_public }
    pub fn has_docs(&self) -> bool { self.has_docs }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum UseKind {
    Name,
    Glob,
    Rename { alias: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StolenPathV1 {
    pub root: String,
    pub segments: Vec<String>,
    pub is_internal: bool,
    pub is_public_use: bool, // pub use?
    pub kind: UseKind,
    pub full_path: String,
    pub location: FileLocation,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedSymbol {
    pub exported_as: String,     // name visible in public surface
    pub source_path: String,     // crate::x::y or external::path::Thing
    pub module_path: Vec<String>,
    pub location: FileLocation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroDef {
    pub name: String,
    pub module_path: Vec<String>,
    pub location: FileLocation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroInvocationV1 {
    pub name: String,
    pub module_path: Vec<String>,
    pub location: FileLocation,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroInvocation {
    pub name: String,
    pub module_path: Vec<String>,
        #[serde(default)]
    pub path: Option<String>,
    pub location: FileLocation,
    #[serde(default)]
    pub enclosing_fn: Option<String>,
    #[serde(default)]
    pub enclosing_public: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathOccurrenceV1 {
    pub path: String,            // std::sync::Arc
    pub module_path: Vec<String>,
    pub location: FileLocation,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathOccurrence {
    pub path: String,
    pub module_path: Vec<String>,
    pub location: FileLocation,
    #[serde(default)]
    pub enclosing_fn: Option<String>,
    #[serde(default)]
    pub enclosing_public: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallOccurrenceV1 {
    pub callee: String,          // unwrap / expect / foo / bar::baz
    pub module_path: Vec<String>,
    pub location: FileLocation,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallOccurrence {
    pub callee: String,
    pub module_path: Vec<String>,
    pub location: FileLocation,
    #[serde(default)]
    pub enclosing_fn: Option<String>,
    #[serde(default)]
    pub enclosing_public: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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




pub struct KleptoGlobSetBuilder(pub GlobSetBuilder);

impl Default for KleptoGlobSetBuilder {
    fn default() -> Self {
        Self(GlobSetBuilder::new())
    }
}

// Optional ergonomics:
impl std::ops::Deref for KleptoGlobSetBuilder {
    type Target = GlobSetBuilder;
    fn deref(&self) -> &Self::Target { &self.0 }
}
impl std::ops::DerefMut for KleptoGlobSetBuilder {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
}

// 
// #[derive(Debug, Clone, Serialize, Deserialize)]
// pub enum ImportOrigin {
//     Internal,          // crate/self/super
//     Std,               // std
//     Core,              // core
//     Alloc,             // alloc
//     WorkspaceMember,   // root matches workspace package name
//     Dependency,        // root matches dependency crate name
//     UnknownExternal,   // anything else
// }
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




#[derive(Debug, Clone, Serialize, Deserialize,
// added Copy for the scanning module
Copy)]
pub enum UseSiteKind {
    UseStmt,       // `use dep::foo`
    ExternCrate,   // `extern crate dep;`
    Attribute,     // `#[dep::something]`
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
    pub scope: String,   // "fn run" / "impl Foo" / "file"
}
use ::std::collections::BTreeMap;
pub type UseSites = BTreeMap<String, BTreeMap<String, usize>>;
pub type UseSitesCount = UseSites;
