# code_passenger — the consumer side of automated feature gating

klepto collects and plans; `code_passenger` is the hand that touches the
tree. This phase gives it a `gate` command that turns a `klepto.toml` spec
into `#[cfg]` attributes and `Cargo.toml` edits, proves them with real
`cargo check` on a scratch copy, and only then writes; a `feature-map` /
`feature-sites` / `feature-probe` trio ported from the 42-step lineage
onto klepto receipts; and `receipt`, `lint` and `patterns` commands that
expose Phases 6–7. Existing commands (`scan`, `annotate`, `scaffold`,
`passenger`) keep their behaviour.

Definition of done: `cargo test` in `code_passenger` is green including
`tests/gate.rs` (13 tests, `#[ignore]` until implemented; run with
`cargo test --test gate -- --ignored`). klepto's `UseSite` contract is
untouched; klepto itself is not modified by this phase.

Dependencies: `klepto = { path = "../klepto" }` (0.2.0 is unpublished;
`cp_compat` proved the API compatible), `tempfile` promoted to a normal
dependency (scratch copies), `similar` (already present) for diffs.
Version → 0.2.0.

Where to implement: `src/gate.rs` (A rules), `src/oracle.rs` (O rules),
`src/commands/{gate,features,receipt,lint,patterns}.rs` (G, F, R rules),
the new subcommands in `src/cli.rs` (X rules).

---

## G — the gate command

- **G1** `gate --spec <klepto.toml> [--package P] [--write] [--check]
  [--max-rounds N] [--no-cargo]`. Package = `--package`, else the
  root manifest's `[package].name` (`-` → `_`), else an error naming the
  scanned packages. klepto is built with `scan_workspace_root(root)` +
  `exclude_generated()` (the calls `scan` already uses).
- **G2 Plan.** `current = k.feature_model(P)`, `desired =
  current.apply_spec(spec.package(P))`, `plan = k.plan_gates(P, &desired)`.
  `commands::gate::plan` returns all four for callers and tests.
- **G3 Dry run.** Without `--write` the command prints the plan (human:
  manifest edits, then one line per gate `FILE:LINE  ATTR  item  (reasons)`,
  then `reaches_public`, `unknown`) and the unified diff of the edits it
  would make. Nothing on disk changes.
- **G4 Prove.** Unless `--no-cargo`, `Klepto::converge(root, P, &desired,
  &mut CargoOracle, max_rounds)` runs (default 3 rounds); the plan used
  for the diff and for writing is `convergence.plan` (forced gates
  included). Human-needed diagnostics are printed under `human:`.
- **G5 Write.** `--write` applies the edits to the real tree only when the
  status is `Converged` (or `--no-cargo` was given) and `plan.unknown`
  is empty. Otherwise nothing is written and the exit code says why.
- **G6 Check.** `--check` never writes; exit 1 when the plan is non-empty
  (CI: "gating needed"), 0 when there is nothing to do.
- **G7 Idempotent.** After `--write`, the same command yields an empty
  plan and exit 0.

## A — applying a plan (`src/gate.rs`, pure until `write`)

- **A1 Item and use gates.** For each `GateEdit`, at `location.line` (the
  carrier's declaration start — its first doc comment or attribute) in
  the file: the carrier's leading lines are its doc comments and
  attributes (multi-line attributes tracked by bracket balance). If one
  of them is a `#[cfg(` attribute, replace it with
  `merged_attribute(existing_cfg, predicate)` = `#[cfg(existing ∧
  predicate)]` simplified — the planner's `predicate` is what to *add*,
  the existing condition must survive; otherwise insert
  `edit.attribute_text()` right above the declaration line, copying its
  leading whitespace. Gates in one file are applied from the highest
  line down, so line numbers stay valid. `feature-probe` shows the same
  merged attribute.
- **A2 Manifest.** The package's manifest is the `Cargo.toml` under the
  root whose `[package].name` (normalised) is the package; edited with
  `toml_edit` so everything else keeps its formatting. `MakeOptional` sets
  `optional = true` on the dependency in whichever table declares it
  (`dependencies`, `dev-`, `build-`, target tables), turning a bare
  version string into an inline table; `SetFeature` writes
  `[features] name = [..]` (creating the table); `SetDefault` writes
  `default = [..]`.
- **A3 EditSet.** `apply(&Klepto, root, plan) -> EditSet { files:
  Vec<FileEdit { path, before, after }>, manifest: Option<FileEdit> }`;
  `diff()` renders unified diffs (`similar`), `write()` writes every
  `after`, `write_to(from_root, to_root)` writes the same contents into
  another tree (the scratch copy). `is_empty()` when nothing changes.
- **A4 Scratch copy.** `scratch_copy(root) -> TempDir`: the whole tree
  minus `target/`, `.git/`, `.passenger/` and anything named `target`
  at any depth. Symlinks are not followed.

## O — the cargo oracle (`src/oracle.rs`)

- **O1** `CargoOracle::new(root)` implements `klepto::Oracle`. The first
  `check` makes one scratch copy (A4); every `check` rewrites, from the
  pristine copy plus the current plan's `EditSet`, every file the plan
  touches, and restores files a previous round touched but this plan
  does not. Restores and rewrites are byte writes, never `fs::copy`: a
  copied file keeps its old modification time (macOS clones), and cargo
  would then call the crate fresh and report nothing.
- **O2** It runs `cargo <job.cargo_args()> --message-format=json
  --offline` with the scratch root as the working directory (`CARGO` env
  honoured), captures stdout, and returns `OracleResult { ok:
  status.success(), diagnostics: parse_cargo_json(stdout) }`.
- **O3** The real tree is never written by the oracle.
- **O4** With `--no-cargo` no oracle runs; `GateOutcome.convergence` is
  `None`.

## F — feature commands (ported surface)

Headers keep the lineage's shape (`CODE PASSENGER :: FEATURE MAP` and a
rule line), then plain `key: value` lines; `--json` prints the struct.

- **F1 `feature-map`.** Per scanned package: features (name, enables as
  Cargo spells them, `default` marker), dependencies (name, kind,
  optional), use-site count per dependency, ungated-use count (KLEP010),
  count of items carrying a `#[cfg(feature)]`. Struct `FeatureMap`.
- **F2 `feature-sites <TARGET>`.** `TARGET` is a dependency name or a
  feature name (`-` → `_`); a feature expands to the dependencies it
  enables (`deps_enabled_by`). Sites grouped by file (relative to root),
  each `line, kind, path, scope, cfg`; `total`. Struct `FeatureSites`.
- **F3 `feature-probe <TARGET> [--spec FILE]`.** A review: with `--spec`,
  the desired model applies — `ungated` = sites whose cfg does not imply
  the desired dependency predicate (verdict word), `proposed` = the plan's
  gates whose reasons name the target's dependencies, `lost` = every item
  the plan gates plus `reaches_public`; without `--spec` the current
  model applies and `proposed` is empty. Always ends with `writes: false`.
  Struct `FeatureProbe`.

## R — receipt, lint, patterns

- **R1 `receipt [--out FILE] [--kg DIR]`.** Writes `Receipt::from_klepto`
  as pretty JSON (default `receipt.json` under the root) and, with
  `--kg`, `knowledge.ndjson` and `challenges.ndjson` from `kg::export`.
- **R2 `lint [--policy FILE]`.** Runs klepto's pipeline. Default policy:
  `KLEP010` and `KLEP011` deny, `KLEP001` and `KLEP012` allow, the rest by
  severity. Prints `Verdict::summary()`; the process exit code is the
  verdict's.
- **R3 `patterns [--min-tokens N] [--top N]`.** Prints candidates in
  weight order: kind, weight, members, note, parameters.

## X — exit codes

`0` nothing to do / converged (dry or written); `1` `--check` found work,
or the oracle gave up (nothing written); `2` the plan has unknown sites
or the oracle left human-needed diagnostics (nothing written); errors
are `Err` → the binary prints them and exits 1 as today.

## E — existing behaviour

- **E1** `scan`, `annotate`, `scaffold`, `passenger` are not changed by
  this phase; `run_scan` keeps its klepto builder calls.

## Out of scope (say so in comments)

Recording gate runs as passenger checkpoints; editing files inside macro
bodies; gating items whose carrier the planner could not name (`unknown`
is printed, a person decides); publishing klepto 0.2.0.

## Fixture

`tests/fixtures/gate/` — a workspace that **compiles**: `app` (uses
`beta` ungated in `uses_beta`, under `#[cfg(unix)]` in `unix_beta`, via a
cascade in `calls_uses_beta`, not at all in `plain`; `util.rs` has two
identical helpers and one unused private fn) + `beta`, and a
`klepto.toml` moving `beta` behind feature `b`. Tests that write do so on
a fresh copy (`tests/common/mod.rs` `fresh_copy`). Assertions in
`tests/gate.rs`; add cases there, do not weaken them.
