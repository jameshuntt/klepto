# klepto Phase 3 — the item graph

Goal: one node per item of every kind, one edge per resolved reference, and
closure queries on top: who uses what, what is transitively lost when a
dependency or a feature goes away, and the item-to-item version of the
Phase 2 linter (`cfg(A) ⇒ cfg(B)` on every reference edge). This is the
"what functionality is lost" answer made transitive.

Definition of done: `cargo test` stays green (Phases 0–2) **and**
`cargo test --test phase3 -- --ignored` is green. `UseSite`, `UseSiteKind`,
`UseSitesCount` and the `KleptoBuilder` calls used by `code_passenger` do not
change shape.

Guiding principle unchanged: **over-approximate, never drop, never claim on
Unknown.** An occurrence that cannot become an edge is listed in
`graph.unresolved` with its candidates; a cfg that is `Unknown` never makes
an item "required" or "lost".

Where to implement: `src/graph.rs` (`build_item_graph` and the three `Klepto`
queries are stubs; the pure graph queries Q1–Q4, `resolve_id` and
`external_roots` are real), the occurrence visitor in `src/extract.rs`
(`ItemDecl` emission and `enclosing_item`), and the F1 rename pass in
`src/resolve.rs` (impl ids and `enclosing_item` strings must be renamed
alongside trait-impl `fq_name`s).

---

## Data model (declared in `src/graph.rs`, `src/model.rs`)

- `NodeKind::{Fn, Method, Struct, Enum, Union, Trait, TypeAlias, Const,
  Static, Macro, Module, Impl, External}`; `EdgeKind::{Reference, Import,
  ReExport, Contains}`.
- `ItemNode { id, kind, package, module, name, location, cfg, is_public }`,
  `ItemEdge { from, to, kind, location, cfg }`,
  `UnresolvedReference { from, raw, resolution, location }`,
  `ItemGraph { nodes, edges, unresolved, duplicates }` + queries,
  `Affected { gated, uses_lost_dep, transitive }`, `CfgEdgeViolation`.
- `ItemDecl` — every declaration the extractors see (`Klepto::items`).
- `enclosing_item: Option<String>` on `PathOccurrence`, `CallOccurrence`,
  `MacroInvocation`, `AttrOccurrence`, `StolenPath`.
- `Klepto::graph` (built in `parse()` after `resolve_all`),
  `Klepto::item_graph()`, `affected_by_dep`, `affected_by_feature`,
  `cfg_edge_violations`.

## N — nodes

- **N1 One node per declared item** in every scanned package:
  - fns and methods: id = `fq_name` (Phase 1 F1 spelling, trait-impl methods
    `pkg::mod::<Ty as Trait>::m`); kind `Fn` for free fns, `Method` for impl
    and trait methods;
  - structs, enums, unions, traits, type aliases, consts, statics,
    `macro_rules!` with a name: id = `<module key>::<Name>`;
  - modules: id = the module key (`app::store`), one per `ModuleInfo`;
  - impl blocks: id = `<module key>::<impl Ty>` for inherent impls and
    `<module key>::<impl Trait for Ty>` for trait impls, `Ty` spelled as
    `impl_self_ty_name` (generics kept, no spaces) and `Trait` canonical when
    it resolves (the same rename pass as F1), else as written.
- **N2 Fields.** `package = Some(package)`, `module` = the module key the
  declaration sits in, `name` = last segment (for impls the whole `<impl …>`
  text), `location` = the declaration's span start, `cfg` = effective cfg
  (Phase 2 E rules), `is_public` = `pub` visibility (trait methods `true`;
  impl blocks take the self type's visibility if known, else `false`;
  modules use `ModuleInfo::is_pub`).
- **N3 External nodes** are created on demand: one per distinct `External`
  canonical string that is the target of an edge and whose crate is **not a
  scanned package** (`std::collections::HashMap`, `serde::Serialize`).
  `kind = External`, `package = None`, `module` = the crate root name (first
  segment), `name` = last segment, `location = None`, `cfg = True`,
  `is_public = true`. An `External` resolution into a scanned workspace
  member (`dep_x::x` when `dep_x` is a member) lands on that member's own
  local node via D4 — the same node its own package sees.
- **N4 Nested items.** Items declared inside a function body (or another
  item's body) get id `<enclosing id>::<Name>`, `parent = Some(enclosing id)`,
  and a `Contains` edge from the enclosing item.
- **N5 `enclosing_item`.** Every occurrence records the innermost item on the
  visitor's item stack: the fn for anything in a fn body or signature, the
  struct for field types, the impl block for `impl` header paths, the const
  / static / type alias for their types and initialisers, the trait for its
  supertraits. `None` means module level (a bare `use`, a module attribute);
  such occurrences belong to the module node. Inside a fn,
  `enclosing_item == enclosing_fn`.
- **N6 Duplicates.** If two declarations produce the same id (cfg
  alternates without distinguishing generics, `#[cfg]` twins), the first in
  file order is kept and the id is appended to `graph.duplicates`.
  *Amended (twins fixture):* when the declarations' cfgs together cover
  every configuration — `any(cfg₁, …, cfgₙ)` is always true, e.g.
  `feature = "x"` and `not(feature = "x")` — the node's cfg is `True`: the
  item is always present, so ungated callers are not violations. Twins
  that do not provably cover everything (`unix` / `windows`;
  `fast_arithmetic = "32"` / `"64"` set by a build script) make the node's
  cfg `Unknown("twins: …")`: which declaration applies is undecidable,
  and the first twin's cfg would be a claim. Edges into such nodes are
  KLEP018, never KLEP011.
- **N7 Macro-generated items do not exist** (out of scope), and neither do
  closures.

## D — edges

- **D1 Reference.** For each `PathOccurrence`, `MacroInvocation` and
  `AttrOccurrence` (own path — unless it is a bare built-in attribute
  name such as `cfg`, `inline`, `allow` — and each `arg_resolutions`
  entry of a `derive` or any argument path with two or more segments;
  `feature = "x"`, `rename = "y"`, `any(..)` are attribute syntax, not
  names — scale-run amendment) whose
  resolution is `Local` or `External`: an edge from the occurrence's
  `enclosing_item` (or its module key when `None`) to the node chosen by D4,
  `kind = Reference`, `cfg` = the occurrence's cfg, `location` = the
  occurrence's location. `CallOccurrence`s are not used (their callee paths
  are already `PathOccurrence`s). Identical `(from, to, kind, location)`
  edges are deduplicated.
- **D2 Import / ReExport.** For each `StolenPath` with a `Local`/`External`
  resolution: from the module key (module-level `use`) or the enclosing fn
  (body `use`) to the node chosen by D4; `kind = ReExport` when
  `is_public_use`, else `Import`. A glob import's target is the module node
  (`Local`) or the External node for the glob target itself (`dep_x`).
- **D3 Contains.** module → each child module; module → each item declared
  directly in it (fns, structs, impl blocks, …); impl → each of its methods;
  trait → each of its method declarations; fn or other item → items declared
  inside it (N4). `cfg` = the child's cfg, `location` = the child's location.
- **D4 Target lookup** (`ItemGraph::resolve_id`, real): the exact node id;
  else, if the canonical path has at least two segments, the node for the
  path without its last segment, accepted only when that node is neither a
  `Module` nor `External` (an associated item, an enum variant, an unknown
  method — over-approximated to the type); else none. For `External`
  resolutions there is no prefix logic: the External node for the full
  canonical is created (N3) and used.
- **D5 Unresolved.** Every occurrence whose resolution is `Unresolved` (any
  reason, `MethodCall` included) and every `Local` resolution D4 cannot place
  goes to `graph.unresolved` with `from` as in D1. No edge. *Amended by
  Phase 5 G1:* a method call whose receiver Phase 5 could type carries a
  `Local`/`External` resolution and becomes a `Reference` edge like any
  path; only the still-untyped ones stay here.
- **D6 Self-edges** (`from == to`, e.g. recursion) are dropped.
- **D7 Trait implementations are not linked to the trait's method
  declarations** (`<Config as Runner>::run` has no edge to `Runner::run`);
  the impl block's edge to the trait (D8) carries that relation.
- **D8 Impl headers.** `impl Trait for Ty` references both `Trait` and `Ty`
  from the impl node; `impl Ty` references `Ty`. These come out of D1 with
  `enclosing_item` = the impl id (the self type and trait paths are visited
  while the impl is on the item stack).
- **D9 Attributes on items** (`#[derive(serde::Serialize)]`) reference from
  the item they annotate, since attributes are visited with the item on the
  stack.

## Q — queries

- **Q1 `dependencies_of(id)`** (real): targets of `Reference`, `Import`,
  `ReExport` edges out of `id`.
- **Q2 `dependents_of(id)`** (real): sources of `Reference` and `ReExport`
  edges into `id`. Imports are not dependents (an unused `use` is not
  functionality); a re-export is.
- **Q3 `transitive_dependents(seeds)`** (real): fixed point over Q2,
  seeds excluded from the result.
- **Q4 `transitive_dependencies(id)`** (real): fixed point over Q1, `id`
  excluded.
- **Q5 `affected_by_dep(dep)`**: the *targets* of `dep` are every node
  whose `package == dep` (a scanned member) plus every External node whose
  crate root is `dep` (normalised). `uses_lost_dep` = local nodes outside
  `dep` with a `Reference` or `ReExport` edge to a target; `transitive` =
  `transitive_dependents(uses_lost_dep)` minus `uses_lost_dep` and minus the
  targets; `gated` empty. Works without a feature model.
- **Q6 `affected_by_feature(package, feature)`**: with `package`'s feature
  model, over closed assignments (Phase 2 Q6 enumeration):
  - `gated` = local nodes of `package` whose cfg is false in every closed
    assignment where `feature` is inactive (cfg `Unknown` → never gated);
  - a dependency `D` of `package` is *lost* when `dep_predicate(D)` is false
    in every closed assignment where `feature` is inactive;
  - `uses_lost_dep` = local nodes of `package` not in `gated` with a
    `Reference`/`ReExport` edge to a target (as in Q5) of a lost `D`;
  - `transitive` = `transitive_dependents(gated ∪ uses_lost_dep)` minus both.
  - No model for `package` → `Affected::default()`.
- **Q7 `cfg_edge_violations()`**: for each `Reference` or `ReExport` edge
  whose endpoints are both local nodes of the *same* package with a feature
  model, and whose target cfg is not `True`: `verdict =
  model.closed_implies(edge.cfg, to.cfg)`; report when not `Always`.
  Edges to External nodes are Phase 2's `ungated_dep_uses` and are not
  repeated; `Contains` and `Import` edges and cross-package edges are
  skipped.

## Out of scope (say so in comments)

Method-call resolution (needs types); linking impl methods to trait
declarations; generic instantiation edges; closures and macro-generated
items as nodes; cross-package feature coupling (a workspace member gated by
another member's features); edge weights or call counts; dead-code
detection (though `dependents_of(id).is_empty()` gives a first cut).

## Fixture

`tests/fixtures/graph/` — workspace `app` + `dep_x` + optional `opt_y`
(feature `y = ["dep:opt_y"]`, no defaults). `app/src/lib.rs`, `store.rs`,
`api.rs` have one annotated line per rule: a field-type edge, a return-type
edge, impl header edges, a const→const edge, a type alias edge, a macro
edge, a chain of three fns ending in `dep_x::x2`, an island, recursion, a
re-export, a module-level import, an external glob, a method call, and two
deliberate cfg-edge violations (`uses_gated`, `store_uses_gated`) beside one
correct gated caller. Assertions in `tests/phase3.rs`; add cases there, do
not weaken them.

## Phase 8 amendment (2026-09-02)

- **Q8 — custom cfg keys.** A counterexample to `a ⇒ b` is definite only when
  every cfg key it sets is one the toolchain sets (`test`,
  `debug_assertions`, `doc`, `miri`, `unix`, `windows`, `target_*`, `panic`,
  `proc_macro`, …). If the only counterexamples set a key that only a build
  script or `RUSTFLAGS` could set (`no_serde_derive`, `docsrs`), the verdict
  is `Unknown` (KLEP018 / KLEP019, never KLEP011 / KLEP010): klepto does not
  know who sets it. A plain build has every such key unset, so a target that
  *requires* a custom key is still a definite violation.
- **Q10 — enumerate what the question mentions.** Closed assignments are
  enumerated over the features the two predicates mention (closed under the
  model's implications, with and without defaults), not over every declared
  feature: a feature the question does not mention can only matter by
  implying one it does, and that assignment is reached by requesting the
  implied feature directly. So a package with twenty features (clap) is
  judged exactly instead of falling back to independent booleans, which
  ignored `wrap_help = ["help"]`.
