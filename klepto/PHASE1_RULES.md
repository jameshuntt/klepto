# klepto Phase 1 — module tree + alias resolution

Goal: every item gets its real module path, and every path spelled in source
gets a **canonical** name (`app::query::run`, `regexish::Regex::new`), so that
`dep_use_sites` reports `use regexish::Regex; … Regex::new()` as a site
**inside the function that calls it**, and same-named items in different files
stop colliding.

Definition of done: `cargo test` stays green (Phase 0) **and**
`cargo test --test phase1 -- --ignored` is green. Do not change the shape of
`UseSite`, `UseSiteKind`, `UseSitesCount`, or the `KleptoBuilder` calls used by
`code_passenger` (`scan_workspace_root`, `exclude_generated`, `parse`,
`dep_use_sites`, `internal_use_sites`, `imports`).

Guiding principle: **over-approximate, never drop.** When a rule cannot decide,
emit `Resolution::Unresolved { raw, reason, candidates }` with every candidate
you know of. Do not implement a type checker; no trait resolution, no
inference. The compiler is the oracle downstream (Phase 4).

Where to implement: `src/resolve.rs` (`build_module_tree`, `resolve_all` are
stubs wired into `KleptoBuilder::parse`). Extractors in `src/extract.rs` and
`src/index.rs` must start their `mod_stack` from the file's module segments
instead of `[]` (change their signatures as needed; they are `pub` but nothing
outside this crate calls them). Fill the `resolution` fields declared in
`src/model.rs`.

---

## Data model (already declared in `src/model.rs`)

- `Resolution::{Local(String), External(String), Unresolved{raw, reason, candidates}}`
  - `Local` strings start with the **package name**: `app::query::run`.
  - `External` strings start with the crate name, `-` normalised to `_`:
    `regexish::Regex::new`, `std::collections::HashMap`, `std::println`.
- `ModuleTree { modules: BTreeMap<String, ModuleInfo> }`, keyed by canonical
  module path; `ModuleInfo` fields documented inline.
- `ParsedFile::module: Option<String>` — set for every file.
- `resolution: Option<Resolution>` on `PathOccurrence`, `CallOccurrence`,
  `MacroInvocation`, `AttrOccurrence` (+ `arg_resolutions`), `StolenPath`
  (the `use` **target**), `ExportedSymbol`. After Phase 1 none of these is
  `None`.
- `Klepto::modules`, `Klepto::possible_dep_use_sites`.

## Canonical spelling

- Package name = `pkg.name` from cargo metadata for the package whose target
  root reaches the file; for `scan_in_folder` / `scan_file` it is the
  `Klepto::new(name)` argument. Hyphens become underscores.
- Module path = package name + module segments joined by `::`: `app`,
  `app::query`, `app::engine::parts`.
- `fq_name` (F1): `<module path>::<name>` for free fns;
  `<module path>::<SelfTy>::<name>` for inherent methods;
  `<module path>::<<SelfTy> as <Trait>>::<name>` for trait impl methods, where
  `<Trait>` is the trait path **as resolved** (canonical) when resolution
  succeeds, else as written.
  `SelfTy` is the last path segment of the impl target **including generic
  arguments without spaces** (`G<u8>`, `Vec<T>`); non-path self types keep
  their token spelling with spaces removed.
  So the two `fmt` impls in the fixture are
  `app::engine::<Engine as std::fmt::Display>::fmt` and
  `app::engine::<Engine as std::fmt::Debug>::fmt`.
- `UseSite::scope` (F2): the enclosing function's `fq_name`, else the module
  path (`app::query`). The literal `"file"` is no longer produced.
- `UseSite::dep` (F3): first segment of the canonical path (package name for
  `Local`, crate name for `External`). `UseSite::path` is the canonical path.
- `CapturedFn::module_path`, `StolenPath::module_path` etc.: the module
  segments **without** the package (`["engine", "parts"]`), unchanged shape.

---

## M — module tree

- **M1 Roots.** Workspace scan: each `(package, root file)` in
  `package_roots` is a root with segments `[]`. If a package has both a lib
  and a bin target with the same name, the bin's package name is
  `<name>_bin`; other bin targets use the target name. `scan_in_folder(dir)`:
  root = `dir/lib.rs`, else `dir/main.rs`, else `dir/mod.rs`; if none exists,
  every `.rs` file directly in `dir` is an orphan root (M5). `scan_file(f)`:
  `f` is a root.
- **M2 `mod x;`** declared in file `F` for module `M`: the child file is
  `D/x.rs`, else `D/x/mod.rs`, where `D` is:
  - `F`'s directory if `F` is a root file or is named `mod.rs`
    ("mod-rs style");
  - `F`'s directory joined with `F`'s file stem otherwise
    (`query.rs` → `query/`, "non-mod-rs style").
  If neither file exists, record the module with `file` = the expected
  `D/x.rs` path and mark it `orphan: true` with no items (never fail the
  scan).
- **M3 `#[path = "p"]`** on `mod x;` overrides M2: the file is `p` relative to
  `F`'s directory (for a declaration inside an inline module, relative to
  `F`'s directory as well — this is the simplification, note it in a comment).
- **M4 Inline `mod x { … }`** in file `F`: `inline: true`, `file: F`,
  `declared_in: Some(F)`. A `mod y;` declared inside it looks in `D/x/y.rs`
  then `D/x/y/mod.rs`, with `D` from M2 for `F`. Nested inline modules extend
  the directory the same way.
- **M5 Orphans.** Any scanned `.rs` file no declaration reaches becomes module
  `<package>::<file stem>` with `orphan: true`, `declared_in: None`. Its items
  are still extracted and resolved. Files under `tests/`, `examples/`,
  `benches/` (only present with `include_tests` etc.) are orphans.
- **M6 `#[cfg]` duplicates.** Two declarations of the same module name in the
  same parent (typically `#[cfg(unix)] #[path] mod sys;` / `#[cfg(windows)]
  …`): the first in source order wins `file`; the other files go to
  `alternates` and are parsed as orphans (M5) so nothing is lost. Phase 2
  evaluates cfg.
- **M7 Items.** `ModuleInfo::items` lists every item name declared directly in
  the module body: fns, structs, enums, unions, traits, type aliases, consts,
  statics, child modules, `macro_rules!` names (as `Macro`), and every name
  bound by a `use` item in that module (as `Use`, including globs recorded
  separately — see S3). Namespaces are ignored: one map, later entries do not
  overwrite earlier ones.
- **M8 Macro-declared modules** (scale run, fixture `macromods`). A
  `[pub] mod name;` sequence inside the tokens of a module-level macro
  invocation (`cfg_rt! { pub mod runtime; }` in tokio, `cfg_feature!` in
  hyper, `cfg_if!` branches, serde's `crate_root!`) is a module declaration
  like any other — its file is found by M2, it is not an orphan — with cfg
  `Unknown("<macro>!")`: the macro decides, klepto does not guess, so the
  module's items never count as gated or as violations. Inline `mod x { }`
  inside macros and every other macro-generated item stay out of scope (N7).
- **M6 amendment** (scale run, fixture `twins`). When a second `mod x;`
  names the *same file* under another cfg (`#[cfg(feature = "std")] pub
  mod ser;` and `#[cfg(not(feature = "std"))] mod ser;` in serde_json), the
  module is present under either: its cfg becomes `True` when the
  declarations' cfgs cover every configuration and `Unknown("twins: …")`
  when they do not (undecidable, never the first one's cfg), and it is
  public if any declaration is. A second declaration naming a *different*
  file stays an alternate as before.

## S — scope tables (one per module, plus overlays)

- **S1 Bindings.** For module `M`, `scope(M)` maps a name to a *target path*:
  - `use a::b::C;` → `C ↦ a::b::C`
  - `use a::b::C as D;` → `D ↦ a::b::C`
  - `use a::b::{self, C};` → `b ↦ a::b`, `C ↦ a::b::C`
  - `pub use …` binds the same way (and is also an export).
  - `extern crate foo as bar;` → `bar ↦ ::foo`; plain `extern crate foo;` → `foo ↦ ::foo`.
  - every item declared in `M` → `name ↦ <M>::name` (canonical, already Local).
  Explicit bindings shadow glob-supplied names (S3). If the same name is bound
  twice explicitly, the first wins (real Rust would reject the code).
- **S2 Target resolution.** A target path is itself resolved with the R rules
  (with `M` as the current module), so `use super::engine::Engine` binds
  `Engine ↦ app::engine::Engine` and `pub use query::Query` binds
  `Query ↦ app::query::Query`. Chains through other `use` bindings are
  followed; guard cycles with a depth limit of 32 and yield
  `Unresolved{UnknownName}` past it.
- **S3 Globs.** `use a::b::*;` appends the resolved target `a::b` to
  `globs(M)` in source order. If the target is `Local`, the glob supplies every
  name in that module's `items` (including its own `Use` bindings that are
  `pub`). If the target is `External`, its names are unknown: any lookup that
  falls through to an external glob yields `Unresolved{GlobImport, candidates}`
  with `candidates = [<target>::<name>]` for **each** external glob in `M`; if
  more than one glob (local or external) could supply the name, reason is
  `Ambiguous` and `candidates` lists all of them, source order.
- **S4 Function-body overlay.** `use` items inside a block (recorded with
  `enclosing_fn = Some(f)`) form an overlay scope for occurrences whose
  `enclosing_fn == f`: look the name up in the overlay first, then in
  `scope(M)`. (Nested blocks are not distinguished — one overlay per function.)
- **S5 Generics.** Type parameters of the enclosing fn, impl, trait or struct
  are collected as a set per occurrence context; a first segment that matches
  one resolves to `Unresolved{GenericParam}`.

## R — resolving a path `s0::s1::…::sn` in module `M` (optionally inside fn `f`)

Apply the first rule that matches. `rest` = `s1::…::sn` (may be empty).

- **R1 `crate::rest`** → `Local(<package of M>::rest)`.
- **R2 `self::rest`** → `Local(<M>::rest)`.
- **R3 `super::…`** → strip each leading `super`, popping one segment of `M`
  per `super` (at the root: `Unresolved{UnknownName}`), then as R2.
- **R4 `::x::rest`** (leading `::`, `is_absolute`) → `External(x::rest)`.
- **R5 `Self`** → if inside an `impl` block, substitute the impl target's path
  (resolved with these rules) and continue with `rest`; otherwise
  `Unresolved{SelfType}`.
- **R6 Overlay** (S4): `s0` bound in the fn overlay → replace `s0` with the
  binding's target and resolve again (depth guard 32).
- **R7 Module scope** (S1): `s0` bound in `scope(M)` → same substitution.
  Because items and `use` bindings both live here, `helpers::help` resolves to
  `app::helpers::help` and `Query::new` to `app::query::Query::new`.
- **R8 Generic param** (S5) → `Unresolved{GenericParam}`.
- **R9 Extern prelude.** `s0` is a dependency of the package (names from
  cargo metadata, `-`→`_`), or `std` / `core` / `alloc` → `External(s0::rest)`.
  If **no dependency list is known** (`scan_in_folder`, `scan_file`), any `s0`
  not resolved by R1–R8 that is not in the std prelude table is treated as an
  external crate: `External(s0::rest)` (this keeps today's behaviour for fully
  qualified paths).
- **R10 Std prelude.** `s0` in the prelude table below → `External(std::s0[::rest])`.
- **R11 Globs** (S3): the name is supplied by exactly one local glob →
  `Local(<target>::s0::rest)`; otherwise `Unresolved{GlobImport|Ambiguous, candidates}`.
- **R12 Macros.** A macro path `p!` is resolved with R1–R11 like any path
  (`regexish::mk!` → R9; `mk!` after `use regexish::mk;` → R7 →
  `External(regexish::mk)`; `println!` → R10 → `External(std::println)`).
  A single-segment macro that resolves nowhere is `Unresolved{UnknownName}`
  (it may be a `macro_rules!` from another file — textual scoping is out of
  scope).
- **R13 Otherwise** → `Unresolved{UnknownName}` (single segments here are
  local variables, fields, primitive types; see O1).

When a substitution produces a path through modules (`app::query::Query::new`),
you may verify segments against `ModuleInfo::items`; on failure keep the
canonical string and leave the resolution `Local` — Phase 3 checks existence.

Std prelude table (Rust 2021, names only): `Option Some None Result Ok Err Box
String Vec ToString ToOwned Clone Copy Send Sync Sized Unpin Drop Fn FnMut
FnOnce AsRef AsMut Into From TryInto TryFrom Iterator IntoIterator
DoubleEndedIterator ExactSizeIterator Extend Default PartialEq Eq PartialOrd
Ord Hash Debug drop` and the macros `println print eprintln eprint format vec
panic assert assert_eq assert_ne debug_assert debug_assert_eq debug_assert_ne
todo unimplemented unreachable write writeln matches dbg include_str
include_bytes include concat stringify env option_env cfg line file column
module_path compile_error format_args thread_local`.
Primitive type names (`u8 … u128 i8 … i128 usize isize f32 f64 bool char str`)
are never recorded (O1).

## O — what gets recorded and resolved

- **O1 Single-segment paths.** Today `visit_path` keeps only multi-segment
  paths (plus bare `std`/`core`/`alloc`). Phase 1 also records a
  single-segment path **when R6/R7/R11 resolve it** (a `use` binding, a module
  item, or a glob candidate) — that is how `helper()` after `use mydep::helper`
  and `Rx::new` become sites. Single segments that reach R8/R10/R13
  (generics, prelude names, locals, primitives) are **not** recorded as
  `PathOccurrence`s (noise). Exception: `MacroInvocation` and
  `AttrOccurrence::arg_paths` keep prelude singles (`println`, `Clone`).
- **O2 Calls.** `CallOccurrence::resolution` for `ExprCall` is the resolution
  of the callee when the callee is a path (`Regex::new(..)`,
  `helper(..)`); any other callee expression and every `ExprMethodCall` get
  `Unresolved{MethodCall}` with `raw` = the callee string.
- **O3 Use targets and exports.** `StolenPath::resolution` is the resolution
  of the target (S2). `ExportedSymbol::resolution` likewise.
- **O4 Attributes.** `AttrOccurrence::resolution` resolves the attribute's own
  path (`derive` → `Unresolved{UnknownName}` is acceptable; `tokio::main` →
  `External(tokio::main)`; `serde` in `#[serde(..)]` → `External(serde)` when
  serde is a dependency). `arg_paths` now also lists **single identifiers**
  found in the argument tokens, and `arg_resolutions` is parallel to
  `arg_paths`: `#[derive(Clone, regexish::Derive)]` →
  `arg_paths = ["Clone", "regexish::Derive"]`,
  `arg_resolutions = [External(std::Clone), External(regexish::Derive)]`.
  Keys of `key = value` pairs (`rename` in `#[serde(rename = "x")]`) are
  listed too and resolve to `Unresolved{UnknownName}`.

## U — use sites after Phase 1

- **U1** `dep_use_sites(D)`: a site for every occurrence (use target, path,
  macro, attribute own path and arg path, extern crate) whose resolution is
  `External(D::…)` (D normalised). `path` is the canonical string, `head` the
  segment after `D`, `dep` = `D`.
- **U2** `internal_use_sites()`: every occurrence whose resolution is
  `Local(<package>::…)` for the package owning the occurrence's file. `dep`
  is the package name (no longer `"crate"`), `path` canonical.
- **U3** `possible_dep_use_sites(D)`: occurrences whose resolution is
  `Unresolved` with at least one candidate starting with `D::` — one site per
  matching candidate. Disjoint from U1.
- **U4** Scope strings per F2.

## Out of scope (state it in code comments, do not attempt)

Namespaces (type/value/macro); local-variable shadowing of imports;
`macro_rules!` textual scoping across files; `#[macro_use]`; edition 2015 path
semantics (treat as 2018); `cfg` evaluation (M6 only records); trait method
resolution and anything needing types; proc-macro-generated items.

## Fixture

`tests/fixtures/resolve/` — workspace `app` (+ deps `regexish`, `other`).
Every rule above is exercised by at least one comment-annotated line there and
asserted in `tests/phase1.rs`. Add cases there; do not weaken assertions.

## Phase 8 amendments (2026-09-02)

- **O1 — local bindings.** A single identifier that names a local binding in
  scope (fn parameter, `let`, `if let` / `while let`, `match` arm, closure
  parameter, `for` pattern; lowercase-initial names) is a value, never a
  reference to an item of the same name (syn's `meta` locals beside
  `mod meta`). Upper-case-initial pattern identifiers (`None`, consts) stay
  item references.
- **R12 — macro visibility.** A `macro_rules!` name is visible in the module
  that defines it and its descendants. `#[macro_use]` on a module's
  declaration lifts its macros to the parent, and so on up to the crate root
  (then the whole crate). A macro defined in `mod tests` is not what the same
  name means elsewhere (regex-syntax's own `assert_eq!`). The expander scopes
  definitions inside inline modules the same way (rule X2).
- **R15 — namespaces.** Every path occurrence records the namespace of its
  position (`PathOccurrence::ns`): type positions (`Type`), expressions and
  tuple-struct patterns (`Value`), else `Any`. When a value-namespace item
  (`fn`, `const`, `static`) and a `use` binding share a name in a module, a
  type position resolves to the `use` (syn's peek-marker `fn Ident` beside
  `use proc_macro2::Ident`); a type-only item (`trait`, `mod`, `type`) does not
  answer a value position. Two items of different namespaces with one name
  in one module are not distinguished (rare; their ids coincide).
- **R14 — cfg-twin bindings.** A name bound more than once in a module under
  different cfgs (`#[cfg(test)] use mocks::X; #[cfg(not(test))] use real::X;`
  in tokio; rustls's `#[cfg(feature = "logging")] use log;` beside
  `#[cfg(not(feature = "logging"))] mod log`) resolves, for an occurrence
  whose cfg is known, to the binding that cfg implies when exactly one does;
  otherwise to the binding active in the build the reference is compiled
  in (the package's default features plus the features the occurrence's
  own cfg asks for, no cfg keys set), else the first. When the twins' cfgs together cover every build, the occurrence is
  marked `covered` and its reference edge is exempt from cfg checking
  (`ItemEdge::covered`): the name is there in every build, whichever twin
  provides it. Not modelled: which twin a *test* build sees for references
  outside test code (the edge goes to the default build's twin).
- **R16 — descending through modules.** After a prefix that resolves to a
  scanned module, the next segment is looked up in that module's scope —
  items, `use` bindings (rules R14 / R15 apply, in the namespace of the
  position) and globs — instead of being spelled onto the module key. So
  `crate::ident::Ident` in a type position follows syn's
  `pub use proc_macro2::Ident` rather than naming the peek-marker `fn Ident`.
  A prefix that is not a module (an enum, `Self`) still appends.
