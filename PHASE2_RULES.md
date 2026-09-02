# klepto Phase 2 — cfg predicates + Cargo feature model

Goal: know, for every item and every dependency use, the condition under
which it is compiled; know, for every package, which features enable which
optional dependencies; and combine the two to answer "is this use of
dependency D gated by something that guarantees D is present?" — the
one-dimensional version of the feature linter. Phase 3 (item graph) adds
transitive effects; Phase 4 adds the compiler oracle.

Definition of done: `cargo test` stays green (Phases 0 and 1) **and**
`cargo test --test phase2 -- --ignored` is green. `UseSite`, `UseSiteKind`,
`UseSitesCount` and the `KleptoBuilder` calls used by `code_passenger` do not
change shape.

Guiding principle: **never assume absent.** Anything that cannot be parsed
becomes `CfgPredicate::Unknown`, which evaluates to "don't know" and turns
any judgement that depends on it into `Implication::Unknown`.

Where to implement: `src/cfg.rs` (`parse_str`, `parse_tokens`, `implies` —
stubs; `and`/`features`/`keys`/`eval` are real), `src/features.rs`
(`build_feature_models` and the query methods — stubs, wired into
`scan_workspace_root` and `Klepto`), and a cfg stack in the extractors
(`src/extract.rs` walkers and the occurrence visitor; `src/resolve.rs`
`decls_of` for module declarations). Fill the `cfg` fields declared in
`src/model.rs`.

---

## Data model (declared in `src/cfg.rs`, `src/features.rs`, `src/model.rs`)

- `CfgPredicate::{True, Feature, Key, KeyValue, All, Any, Not, Unknown}`.
- `CfgEnv { features, keys }` — keys hold `unix`/`test`/… and key-values
  spelled `target_os=linux`.
- `Implication::{Always, Never, Sometimes{counterexample}, Unknown}`.
- `FeatureModel { package, features: name → FeatureInfo{enables, implicit},
  dependencies: Vec<DepInfo> }`, `FeatureEnable::{Feature, Dep, DepFeature{dep, feature, weak}}`,
  `DepInfo { name, package, optional, kind, target, default_features, features, implicit_feature }`,
  `DepKind::{Normal, Dev, Build}`.
- `cfg: CfgPredicate` on `CapturedFn`, `ModuleInfo`, `StolenPath`,
  `ExportedSymbol`, `PathOccurrence`, `CallOccurrence`, `MacroInvocation`,
  `AttrOccurrence` (+ `predicate` for `#[cfg]` attributes), `ExternCrateOccurrence`.
- `Klepto::features`, `Klepto::feature_model`, `dep_use_sites_with_cfg`,
  `items_requiring_feature`, `ungated_dep_uses` → `UngatedUse`.

## C — cfg predicate grammar

Input: the tokens inside `#[cfg(..)]` / `#[cfg_attr(<here>, ..)]`, the
argument of `cfg!(..)`, or a cargo target string.

- **C1** `feature = "x"` → `Feature("x")`.
- **C2** `ident` → `Key(ident)`; `ident = "v"` (any ident but `feature`) →
  `KeyValue(ident, v)`.
- **C3** `all(p, …)` → `All`, `any(p, …)` → `Any`, `not(p)` → `Not`;
  trailing commas allowed; `all()` → `True`; `any()` → `Not(True)`.
- **C4** Cargo target strings: `cfg(<pred>)` → parse `<pred>`; a bare target
  triple `t` → `KeyValue("target", t)`.
- **C5** Anything else (unbalanced parens, unknown operators, literals in
  the wrong place, `version(..)`, `accessible(..)`) → `Unknown(<raw text>)`.
- **C6** `and` flattens nested `All` and drops `True`; no other
  simplification (do not rewrite `Not(Not(x))`, do not dedupe).

## E — effective cfg

The effective cfg of a thing is the conjunction (`and`) of its own
`#[cfg(..)]` attributes (several attributes on one item are all required)
and the effective cfg of everything that encloses it. Attach it to every
record listed above at extraction time; the resolver does not touch it.

- **E1 Items.** Free fns, impl blocks and their methods, trait blocks and
  their fns, structs/enums/unions/traits/consts/statics/type aliases, `use`
  items, `extern crate`, `macro_rules!`, inline `mod` blocks. An impl item
  inherits the impl block's cfg; a method's `cfg` is `impl.and(own)`.
- **E2 Modules.** `#[cfg(..)] mod x;` gates the whole file: every item and
  occurrence in `x.rs` starts from that predicate (times the declaring
  module's own). `ModuleInfo::cfg` holds it; `decls_of` must carry the
  attribute through `ModDecl`. Inline modules likewise. Roots and orphans
  start at `True`.
- **E3 Statements and expressions.** `#[cfg]` on a `let`, an expression
  statement, a block, a match arm, a struct field, an enum variant, a fn
  parameter, or a closure applies to everything recorded inside it. The
  occurrence visitor keeps a cfg stack: push on entering any node carrying
  cfg attributes, pop on leaving.
- **E4 Occurrences.** `PathOccurrence`, `CallOccurrence`, `MacroInvocation`,
  `AttrOccurrence`, block-level `use` items and `ExternCrateOccurrence` get
  the cfg in force where they appear (enclosing fn's cfg included).
- **E5 `cfg_attr`.** `#[cfg_attr(p, a, b)]` is expanded: record attributes
  `a` and `b` as `AttrOccurrence`s with `cfg = enclosing.and(p)`; nesting
  (`cfg_attr` inside `cfg_attr`) conjoins. The `cfg_attr` attribute itself
  need not be recorded. A `derive(dep::Thing)` produced this way therefore
  yields an `Attribute` use site gated by `p`.
- **E6 `cfg!`.** The `cfg!(..)` macro is an expression, not a gate: the
  occurrence is recorded with the enclosing cfg, and its argument is not
  applied to anything. (Recording the parsed predicate on the
  `MacroInvocation` is not required.)
- **E7 `#[cfg]` attributes are still recorded** as `AttrOccurrence`s with
  `path = "cfg"` and `predicate = Some(parsed)`, `arg_paths` as today.

## F — feature model (from `cargo metadata --no-deps`)

Built per scanned package in `scan_workspace_root`; `scan_in_folder` /
`scan_file` have no model and `feature_model()` returns `None`.

- **F1** `features`: every `[features]` entry, each string parsed as
  `dep:name` → `Dep`, `name/feat` → `DepFeature{weak: false}`,
  `name?/feat` → `DepFeature{weak: true}`, otherwise `Feature`.
- **F2** `dependencies`: one `DepInfo` per cargo dependency, in manifest
  order. `name` is the name used in source: the `package = ` rename when
  present (`cargo_metadata::Dependency::rename`), else the package name;
  `-` → `_`. `package` is the published name.
- **F3** `kind`: `Normal`, `Development` → `Dev`, `Build` → `Build`.
  `target`: `Some(parse_str(platform))` when the dependency is
  target-specific (`cargo_metadata::Dependency::target`, via `Display`),
  else `None`. `default_features`, `features`, `optional` copied.
- **F4 Implicit features.** For each optional dependency `d` (by `name`):
  if no feature's list contains `Dep(d)`, cargo creates feature `d` with
  `enables = [Dep(d)]`; add it with `implicit: true` and set
  `DepInfo::implicit_feature = true`. If some feature does use `dep:d`, no
  implicit feature exists and `implicit_feature = false`.
- **F5** A non-optional dependency is never enabled by features; `DepFeature`
  entries pointing at it only forward a feature.
- **F6** The same dependency may appear more than once (different targets or
  kinds); keep every entry. `dep(name)` returns the first.
- **F7** Workspace members that are dependencies of each other are modelled
  exactly like external dependencies.

## Q — queries

- **Q1 `active_features(requested, default_features)`**: start from
  `requested` plus `"default"` (if asked and declared); repeatedly add every
  `Feature(f)` reachable through `enables` and every implicit feature
  activated by a non-weak `DepFeature{dep}` whose `dep` is optional; stop at
  the fixed point. Names not declared are kept as given (do not error).
- **Q2 `deps_enabled_by(feature)`**: optional dependencies turned on by the
  closure of `{feature}` (no defaults): every `Dep(d)`, every non-weak
  `DepFeature{dep: d}` with `d` optional, and every implicit feature reached.
  Weak entries never enable a dependency.
- **Q3 `features_enabling(dep)`**: `{ f | dep ∈ deps_enabled_by(f) }` over
  all declared and implicit features; empty for a non-optional dependency.
- **Q4 `dep_predicate(dep)`** — the availability condition of a dependency:
  - not optional, `Normal`, no target → `True`;
  - optional → `Any([Feature(f) for f in features_enabling(dep)])`, sorted by
    name; an optional dependency no feature enables → `Any([])`
    (never available);
  - target-specific → `and` the parsed target predicate;
  - `Dev` → `and(Key("test"))` (approximation: dev-dependencies exist for
    tests, examples and benches — modelled as `cfg(test)`); `Build` →
    `and(Key("build_script"))`;
  - unknown `dep` → `Unknown(dep)`.
- **Q5 `a.implies(b, preferred)`** — variables (features and keys of both
  predicates) are independent booleans. Let `V` be their union.
  - If either predicate `is_unknown()` and an `Unknown` leaf decides the
    result for some assignment → `Unknown`. (Simplest correct rule: if
    either is unknown, return `Unknown`.)
  - If `|V| ≤ 16`: enumerate all assignments. If no assignment makes `a`
    and `b` true together → `Never` (this includes `a` unsatisfiable). Else
    if every assignment with `a` true has `b` true → `Always`. Otherwise
    `Sometimes{counterexample}` where the counterexample is `preferred` if it
    is one (evaluate it first), else the first failing assignment in
    enumeration order (features sorted, then keys sorted; all-false first).
  - If `|V| > 16`: return `Always` only when `b` is syntactically implied
    (`b == True`, or `b` is a `Feature`/`Key` that appears un-negated in a
    top-level `All` of `a`, or `a == b`); else `Unknown`.
- **Q6 `ungated_dep_uses()`** — for each package with a model, for each
  U1 site of a dependency `d` of that package whose `dep_predicate(d) != True`:
  the verdict is `site_cfg.implies(dep_predicate(d), Some(&model.default_env()))`
  computed over **closed** assignments: instead of independent feature
  variables, enumerate `active_features(R, defaults)` for every subset `R`
  of the package's declared features and both `defaults ∈ {true, false}`
  (cap: 12 declared features → 8192 requests; beyond that fall back to Q5),
  times all assignments of the keys involved (cap 4 keys). Report every site
  whose verdict is not `Always`. `counterexample` is `default_env()` when
  that fails (the common "breaks the default build" case), else the first
  failing closed assignment. Sites of dependencies from other packages, or
  with no model, are skipped.
- **Q7 `items_requiring_feature(package, feature)`** — fq_names of
  `CapturedFn`s in that package whose effective cfg is false in every closed
  assignment (as in Q6) where `feature` is not active — i.e. the item cannot
  exist with the feature off. Items with an `Unknown` cfg are excluded
  (never claim). Sorted by fq_name. Direct gating only; transitive effects
  are Phase 3.
- **Q8 `dep_use_sites_with_cfg(deps)`** — `dep_use_sites` paired with the
  effective cfg of the occurrence each site came from.

## Out of scope (say so in comments)

Feature unification across the workspace (cargo unifies the requested
features of one dependency across all its dependents in a build; the model
is per package as declared); build scripts and `cargo:rustc-cfg` output;
`--cfg` flags from `RUSTFLAGS` / `.cargo/config`; `cfg(version(..))`,
`cfg(accessible(..))`, `cfg(sanitize)`; `#[cfg]` on items produced by
macros; `cfg_attr(.., path = "..")` changing module files; `dev-dependencies`
per-target; profile-specific settings.

## Fixture

`tests/fixtures/features/` — workspace `app` + `core_dep`, `opt_a`, `opt_b`,
`devonly`, `unixonly`. `app/Cargo.toml` declares `default = ["fast"]`,
`net = ["dep:opt_b", "opt_a"]`, `full = ["net", "core_dep/extra"]`,
`weak = ["opt_b?/extra"]`, a dev-dependency and a `cfg(unix)` dependency.
`app/src/lib.rs` has one annotated line per E rule and three deliberately
ungated uses (`oops`, `fast_only`, `uses_dev`). Assertions in
`tests/phase2.rs`; add cases there, do not weaken them.
