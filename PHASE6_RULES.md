# klepto Phase 6 — receipt, knowledge-graph export, analysis frame

Goal: klepto's output becomes a document other things consume. Today a
consumer must hold a `Klepto` in memory and call methods; after Phase 6 it
can read one versioned, digested, relocatable **receipt**, feed the same
facts to the world model as **knowledge-graph records and challenges** in
langbrain's vocabulary, and run every analysis through one **pipeline**
that yields findings and an exit code. klepto stays a leaf crate: it
returns strings and values, never writes files, never runs cargo.

Definition of done: `cargo test` stays green (Phases 0–5) **and**
`cargo test --test phase6 -- --ignored` is green. `UseSite`, `UseSiteKind`,
`UseSitesCount` and the `KleptoBuilder` calls used by `code_passenger` do not
change shape. langbrain is never a dependency; only its record shape is
matched (see `crates/kg` there: `kind`/`id`/`description` on every record,
`from`/`to` on edges, challenges with `class`/`prompt`/`expected`/`digest`/
`split`, SHA-256 digests, heldout = first digest nibble in `0..=3`).

Where to implement: `src/receipt.rs` `Receipt::from_klepto` (R1–R7; the
JSON, digest, `stale_files` and `diff` code is real already), `src/kg.rs`
`export` (K1–K9; digest/split/NDJSON helpers real), `src/rules/phase6.rs`
the six rules (A1–A2; the frame in `src/pipeline.rs` and `rules/mod.rs` is
real). `Klepto::root` is set by the builder already.

---

## R — receipt

- **R1 Header.** `version = RECEIPT_VERSION` (1), `tool = "klepto"`,
  `tool_version` = the crate version, `language = "rust"`, `crate_name`,
  `root` as given. A consumer refuses a receipt whose `version` it does not
  know; additive fields never bump the version.
- **R2 Digest.** `digest` = blake3 hex of the compact JSON of the receipt
  with `digest` empty. `verify_digest` recomputes it. Two runs over the
  same tree produce byte-identical JSON and the same digest (R3 makes the
  order canonical).
- **R3 Canonical order and relocatable paths.** Every list is sorted:
  files by `file`; packages by name; modules by path; items by `(id,
  location)`; edges by `(from, to, kind, location, cfg)`; unresolved by
  `(from, raw, location)`; functions by `(fq_name, location)`; types and
  impls by id; imports, exports, calls, macros, attrs, extern crates by
  `(location, main string)`; dep uses by `(package, site.location,
  site.path)`; skipped by file. Location order is `(path, line, column)`.
  Every `FileLocation.path` (in items, edges, functions, types, impls,
  imports, exports, calls, macros, attrs, extern crates, modules,
  unresolved, dep uses) is rewritten relative to `Klepto::root`,
  `/`-separated; a path not under the root is kept as is.
- **R4 Files.** One `FileRecord` per parsed file: relative `file`, its
  package, the module it holds (`None` when the tree did not reach it),
  `blake3` of its bytes, `bytes`. Skipped files appear only in
  `skipped_files` (`file`, parser message).
- **R5 Layers.** The receipt carries the facts of every phase as their
  existing records: `packages` (name + sorted declared deps), `modules`
  (`ModuleInfo`), `items` (`ItemDecl`), `edges` and `unresolved` (the
  graph's), `functions`, `types`, `impls`, `imports`, `exports`, `calls`
  (with Phase 5 receiver types), `macros_def`, `macros_inv`, `attrs`,
  `extern_crates`, `features` (the `FeatureModel` per package), `dep_uses`
  (`dep_use_sites_with_cfg` over each package's declared deps, tagged with
  the package). Raw `PathOccurrence`s are **not** carried: edges plus
  `unresolved` plus `dep_uses` are their reduced form, and this is the one
  deliberate omission — say so in the record's doc comment.
- **R6 Language neutrality.** `ItemDecl` (`id, kind, name, package, module,
  parent, location, cfg, is_public`) and `ItemEdge` (`from, to, kind,
  location, cfg`) are the cross-language contract: a collector for another
  language emits the same two shapes with its own `language`. Everything
  else is Rust detail a consumer may ignore.
- **R7 Staleness.** `stale_files(&old)` = files added, removed or whose
  `blake3` changed, sorted. A consumer with a stored receipt re-runs klepto
  when the list is non-empty (klepto itself always re-parses everything;
  incremental parsing is out of scope).
- **R8 Diff.** `diff(&old)` = items added / removed / changed (same id,
  different kind, cfg, visibility or parent), edges added / removed,
  functions whose `signature` changed, files added / removed / changed.
  Supersedes `Snapshot` for consumers; `Snapshot` stays as is.

## K — knowledge graph

- **K1 Records.** `export(&Klepto) -> KgBundle { knowledge, challenges }`,
  each a `serde_json::Value` object with string `kind`, `id`,
  `description` (plain language, self-describing). `to_ndjson` renders one
  record per line. Export is deterministic: records sorted by `id`, the
  same tree gives the same bundle. Ids are unique within a bundle.
- **K2 Nodes.** `package` (`pkg/<name>`; `name`, `modules`, `items`),
  `module` (`mod/<module key>`; `package`, `file`, `inline`, `orphan`,
  `cfg` rendered), `item` (`item/<node id>` for every graph node that is
  not a module; `item_kind` = the `NodeKind` lower-cased, `external` for
  External nodes; `name`, `package`, `module`, `file`, `line`, `cfg`,
  `public`), `feature` (`feat/<pkg>/<name>`; `package`, `name`,
  `default`, `declared` — `false` for a feature only a `#[cfg]` names),
  `crate` (`crate/<name>` for every dependency that is not a scanned
  package, and for the root of every External node), `constant`
  (`const/klepto/receipt_version`, `const/<pkg>/items`, `const/<pkg>/edges`,
  `const/<pkg>/unresolved`, `const/<pkg>/files`; `name`, `value`).
- **K3 Files.** Every `file` field is relative to `Klepto::root` (R3).
- **K4 Edges.** `{kind, id, from, to, description, occurrences, file,
  line}` with `id = <kind>/<from bare id>/<to bare id>` (bare = without
  the `pkg/`, `mod/`, `item/`, `feat/`, `crate/` prefix). Several graph
  edges with the same `(kind, from, to)` fold into one record with
  `occurrences` = their count and the first location. Kinds:
  - `contains`: `pkg → root module`, and every graph `Contains` edge;
  - `references`, `imports`, `reexports`: the graph's `Reference`,
    `Import`, `ReExport` edges;
  - `implements`: impl block → its trait (`ImplDecl.trait_` head mention,
    Local or External); `impl_of`: impl block → its self type (Local only);
  - `depends_on`: `pkg → pkg` or `pkg → crate` per declared dependency
    (`optional`, `kind` on the record);
  - `enables`: `feat → feat` and `feat → pkg|crate` from `FeatureInfo`'s
    enables (`dep:` and plain dependency names alike);
  - `gated_by`: item → feature for every feature name in the item's own
    cfg (`CfgPredicate::features()`), with the full predicate rendered on
    the record.
- **K5 Closure.** Every `from` and `to` names a node record in the same
  bundle. Missing targets are created on demand (`crate/…`, `item/…` of
  kind `external`, `feat/…` with `declared = false`) rather than dropped.
- **K6 External items.** External graph nodes become `item` records of
  `item_kind = "external"` with `package` = their root crate; the crate
  gets a `crate/` node and a `contains` edge from it.
- **K7 Challenges.** `{kind: "challenge", class, id, package, language:
  "rust", prompt, expected, digest, split, frontier: "available",
  description}` plus class-specific fields; `id = chal/<class>/<pkg>/<first
  8 hex of digest>`; `digest = sha256("klepto|<class>|<pkg>|<prompt>|
  <expected>")`; `split = split_for(digest)`. `expected` is a string —
  for lists, the JSON array text of the sorted ids. Classes:
  - `dependents_of` (`item`): for every item with 1–20 direct dependents
    (`ItemGraph::dependents_of`), expected = the sorted dependents;
  - `resolve_path` (`module`, `path`): for every module-level `use` whose
    resolution is Local/External, expected = the canonical name;
  - `lost_without_feature` (`feature`): for every declared feature with a
    non-empty `affected_by_feature`, expected = sorted union of `gated`,
    `uses_lost_dep`, `transitive`;
  - `receiver_type` (`function`, `method`, `line`): for every method call
    with a `Local` receiver type and a line, expected = the type id.
- **K8 Determinism of challenges.** Same tree → same challenges, same
  digests, same splits. Prompts are plain language and mention the package
  and the location the answer is about.
- **K9 No langbrain.** Nothing here reads langbrain's corpus or code.

## A — analysis frame

- **A1 One interface.** Every analysis is a `Rule` (`code`, `name`,
  `run(&Klepto) -> Vec<Finding>`). `rules::all_rules()` lists the Phase 0
  lints (KLEP001–004) and the Phase 6 rules; `RuleRunner::with_all_rules()`
  runs them all; `all_codes()` is the closed list a policy may name.
- **A2 Phase 6 rules.** Each finding carries a `location`, a message and an
  `extra` object with the ids it is about:
  - `KLEP010 ungated-dep-use` — one per `ungated_dep_uses()` entry whose
    verdict is `Never`/`Sometimes`; Deny; extra `{dep, path, scope,
    site_cfg, dep_predicate, verdict}`.
  - `KLEP019 dep-use-unknown` — the entries whose verdict is `Unknown`
    (an Unknown cfg is involved, e.g. a module declared inside a macro);
    Warn; same extra. *Scale-run amendment:* undecidable is not a
    violation, and a policy must be able to deny the definite ones alone.
  - `KLEP011 cfg-edge-violation` — one per `cfg_edge_violations()` entry
    whose verdict is `Never`/`Sometimes`; Deny; extra `{from, to,
    edge_cfg, to_cfg, verdict}`.
  - `KLEP018 cfg-edge-unknown` — the entries whose verdict is `Unknown`;
    Warn; same extra.
  - `KLEP012 unresolved-reference` — one per `graph.unresolved`; Info;
    extra `{from, raw, reason, candidates}`.
  - `KLEP013 unreferenced-item` — Info; a non-public item of kind Fn,
    Method, Struct, Enum, Union, Trait, TypeAlias, Const, Static or Macro
    in a scanned package with no incoming Reference, Import or ReExport
    edge (Contains edges do not count), excluding `main`,
    methods of trait impls (called through the trait) and items carrying
    `#[test]`, `#[no_mangle]`, `#[export_name]` or `#[used]`; extra
    `{item, kind}`. Over-approximation is fine at Info.
  - `KLEP014 orphan-file` — one per module with `orphan = true`; Warn;
    extra `{file, module}`.
  - `KLEP015 skipped-file` — one per `skipped_files` entry; Warn; extra
    `{file, error}`.
- **A3 Order.** `RuleRunner::run` returns findings sorted by `(code,
  location.path, line, column, message)`.
- **A4 Policy.** `Policy::parse(toml)`: `[levels]` code → `allow | warn |
  deny`, `[max]` code → count; unknown codes and unknown keys are errors.
  `Policy::permissive()` allows every code.
- **A5 Verdict.** `run_pipeline(&Klepto, &Policy)`: the level of a finding
  is the policy's for its code, else from its severity (Deny → deny, Warn →
  warn, Info → allow). Allowed findings are dropped; the rest are the
  verdict's `findings` with `counts` per code. A violation is any code at
  deny level with count > 0, or any code whose count exceeds its `max`.
  `exit_code` is 1 when there is a violation, else 0.
- **A6 Summary.** `Verdict::summary()` = one line per counted code —
  `CODE name: count (level)` — then `exit N`. Stable text.

## Out of scope (say so in comments)

Incremental re-parsing; receipts for other languages (only the contract
R6); reading langbrain; gate-proposal challenges (they need the Phase 4
plan and a spec file — later); a CLI for the pipeline (`code_passenger`'s
job); rule auto-fixes.

## Fixture

`tests/fixtures/receipt/` — workspace `app` + `optx` (optional dep behind
feature `x`) + registry dep `outside` (never resolved: klepto runs
`cargo metadata --no-deps`, so it is the one truly external crate; note
that klepto *does* scan path dependencies even when they are not
workspace members). `app/src/lib.rs` has one
line per fact: a re-export, a `use std::collections::HashMap`, a trait and
an impl of it, a typed method call, a correctly gated use of `optx`, an
ungated call into the gated item (KLEP011), an ungated use of `optx`
(KLEP010), a call into `outside` (external node), a never-referenced
private fn (KLEP013), an untyped method call (KLEP012); `orphan.rs`
(KLEP014) and `bad.rs` (unparsable, KLEP015; the fixture is built with
`ignore_parse_errors(true)`). Assertions in `tests/phase6.rs`; add cases
there, do not weaken them.
