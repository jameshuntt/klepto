# klepto Phase 4 — the gating planner and the compiler oracle

Goal: from a root spec of the *desired* feature layout, compute the
`#[cfg]` gates and `Cargo.toml` changes that make the code honest about it,
propagate gates through the item graph, and let `cargo check` across feature
combinations be the referee — as a loop that learns from compiler errors.
This is the automation James asked for: nobody walks the code by hand.

klepto's part is pure: plans are data, diagnostics are data, the loop is
generic over an `Oracle`. The one side-effecting piece — copy the workspace,
apply a plan, run cargo — is `code_passenger`'s `CargoOracle` (contract at
the end).

Definition of done: `cargo test` stays green (Phases 0–3) **and**
`cargo test --test phase4 -- --ignored` is green. Nothing in klepto writes
files or spawns cargo. `UseSite`, `UseSiteKind`, `UseSitesCount` and the
`KleptoBuilder` calls used by `code_passenger` do not change shape.

Where to implement: `src/spec.rs` (`apply_spec`, `manifest_edits`),
`src/plan.rs` (`plan_gates_with`), `src/oracle.rs` (`check_matrix`,
`parse_cargo_json`, `map_diagnostic`, `converge`). `CfgPredicate::render`
and `simplify` (`src/cfg.rs`), `FeatureSpec::parse`, `CheckJob::cargo_args`
and `FnOracle` are already real.

---

## S — the spec (`klepto.toml`)

- **S1 Format.** One TOML table per package name; inside it `features`
  (table: name → list, Cargo's `[features]` value syntax exactly, parsed with
  Phase 2's `parse_enable`), `optional` (list of dependency names to make
  optional), `default` (list replacing the `default` feature). Package and
  dependency names are normalised `-` → `_`. Unknown keys are an error.
- **S2 Apply: features.** Each listed feature replaces the same-named
  feature's list in the current model or adds a new feature. Features not
  mentioned are kept.
- **S3 Apply: optional and default.** Every name in `optional` marks that
  dependency `optional = true` (all entries with that `name`); a name that is
  not a dependency is ignored (there is nothing to make optional).
  `default = [..]` replaces the `default` feature's list (creating it if
  absent).
- **S4 Apply: implicit features.** After S2–S3, recompute rule F4: an
  implicit feature exists for an optional dependency only if no *other*
  feature uses `dep:<name>`; implicit features that no longer apply are
  removed, new ones are added, `DepInfo::implicit_feature` follows.
- **S5 Manifest edits** = `manifest_edits(current, desired)`, in this order:
  `MakeOptional{dep}` for each dependency optional in `desired` but not in
  `current` (manifest order); `SetFeature{name, enables}` for each
  non-implicit feature whose list differs or is new (sorted by name, the
  list spelled back in Cargo syntax: `dep:x`, `x/f`, `x?/f`, `f`);
  `SetDefault` when the `default` list differs. Implicit features never
  produce edits.

## P — the plan

- **P1 Inputs.** The package's *current* model (Phase 2), the *desired*
  model (S), the item graph (Phase 3), every use site with its cfg
  (`sites_with_cfg_and_package`), and any `ForcedGate`s from the loop.
- **P2 Manifest.** `plan.manifest = manifest_edits(current, desired)`.
- **P3 Required predicate** of a dependency = `desired.dep_predicate(dep)`,
  simplified (C8). `True` → the dependency never needs a gate.
- **P4 Site gates.** For every site of the package whose dependency has a
  required predicate `P ≠ True`: if `desired.closed_implies(site_cfg, P)` is
  `Always`, nothing; if it is `Unknown`, rule P10; otherwise propose a gate
  on the site's *carrier*:
  - the occurrence's `enclosing_item` when it has one — a fn, method, struct,
    const, static, type alias, or impl block (`GateKind::Item`, `item` = the
    node id, `location` = the node's location);
  - a module-level `use` (`StolenPath` with `enclosing_item == None`) →
    `GateKind::Use`, `item` = the module key, `location` = the `use` item's
    location;
  - anything else at module level (an attribute, a bare path) → P10.
  The gate's predicate is `P`; `existing_cfg` is the carrier's current cfg;
  reason `UngatedDep{dep}`.
- **P5 Cascade.** A carrier `X` gated under `P` ends up with cfg
  `X.cfg ∧ P`. Every dependent `Y` of `X` (graph Q2: `Reference`/`ReExport`
  edges, same package) whose cfg does not imply `X.cfg ∧ P` under closed
  assignments gets a gate under `P` with reason `Cascade{from: X}`; repeat
  until nothing changes. Dependents in other packages are not gated (they
  are listed in `reaches_public` if `X` is public — see P9b).
- **P6 Existing violations.** Every `cfg_edge_violations()` edge `A → B`
  computed against the *desired* model contributes a gate on `A` under
  `B.cfg` with reason `ExistingViolation{to: B}`, then cascades as in P5.
- **P7 Merge.** One `GateEdit` per carrier: predicates combine with `and`
  and are simplified (`all(feature = "a", test)`), reasons accumulate in the
  order they were found. A carrier that already has the predicate in its
  cfg is not re-gated.
- **P8 Order.** `gates` sorted by (`location.path`, `location.line`,
  `location.column`); `manifest` first as in S5.
- **P9 Text.** `GateEdit::attribute_text()` is `#[cfg(<render>)]`; the
  consumer inserts it on its own line directly above the carrier's
  declaration (before any existing attributes).
- **P9b Public reach.** `reaches_public` = sorted ids of gated carriers
  (`GateKind::Item`) whose node `is_public`. Informational: this is the
  functionality the feature will remove.
- **P10 Unknown.** A site whose cfg or required predicate is `Unknown`, or a
  module-level occurrence with no carrier, goes to `plan.unknown` with a
  `why`; no gate is proposed for it. A carrier whose existing cfg is
  `Unknown` is never gated (never claim).
- **P11 Forced.** Each `ForcedGate{item, predicate, reason}` is added as if
  it were a P4 gate on `item` (merged by P7, cascaded by P5).

## C — predicates (additions)

- **C7 `render()`** (real): `feature = "x"`, `unix`, `target_os = "linux"`,
  `all(a, b)`, `any(a, b)`, `not(a)`; single-element `All`/`Any` render as
  the member; `True` as `all()`; `Unknown(raw)` as `raw`.
- **C8 `simplify()`** (real): collapse single-element `All`/`Any`, flatten
  nested same-kind lists, drop duplicates keeping first occurrence, drop
  `True` from `All`, `True` inside `Any` makes it `True`.

## V — the check matrix

- **V1** From the desired model: `no_default` (no features, `--no-default-features`),
  `default`, each declared feature alone without defaults, and all declared
  features with defaults. Implicit features count as declared.
- **V2** When the model declares at most 4 features, use every subset of
  the declared features × {defaults on, off} instead, deduplicated by
  `active_features` closure (the first subset producing a closure wins;
  enumerate subsets by increasing size, defaults-off before defaults-on).
- **V3 `cargo_args()`** (real): `check -p <pkg> --message-format=json
  [--no-default-features] [--features a,b]`. The workspace root is passed
  to the oracle separately.

## X — diagnostics

- **X1 Parse** (`parse_cargo_json`): one JSON object per line; keep
  `reason == "compiler-message"` whose `message.level == "error"`; take
  `message.code.code`, `message.message`, and the first span with
  `is_primary == true` (`file_name`, `line_start`, `column_start`). Lines
  that are not JSON, or other reasons, are skipped.
- **X2 Mention**: the first backtick-quoted text in the message with the
  quotes removed (`alpha`, `crate::store::store_gated`).
- **X3 Map** (`map_diagnostic`): `file` is joined onto `root` and both
  sides canonicalised; the item is the innermost node of `package` whose
  location file matches and — for fns — whose `EnclosingIndex` span
  contains (line, column − 1); otherwise the node with the greatest start
  line ≤ the diagnostic line in that file. `predicate`: if the mention
  (last segment, `-` → `_`) names a dependency of `desired` →
  `desired.dep_predicate(mention)` simplified; else if it resolves to a
  local node of `package` (exact id, `crate::` → package, or a unique node
  with that name) whose cfg is not `True` → that cfg; else `None`.

## L — the loop

- **L1 `Oracle::check(root, job, plan)`**: compile `job` with `plan`
  applied; return `ok` and the diagnostics. Implementations must not touch
  `root` itself.
- **L2 `converge(root, package, desired, oracle, max_rounds)`**: round `r`:
  `plan = plan_gates_with(package, desired, forced)`; run every
  `check_matrix(desired)` job; map every diagnostic (X3). If all jobs pass →
  `Converged`. Else each `MissedGate` with `item` and `predicate` both
  `Some` becomes a `ForcedGate{reason: Forced{code, mention}}` (deduplicated
  by item + predicate); the rest go to `human`. If no new forced gate was
  added, or `r == max_rounds` → `GaveUp`. Every round is recorded.
- **L3** `plan` in the result is the last plan tried; `forced` accumulates
  across rounds.
- **L4** A missed gate whose `item` already carries that predicate (the
  oracle keeps failing the same way) does not count as new.

## The consumer contract (`code_passenger`, not part of this crate)

`CargoOracle::check` copies the workspace to a scratch directory, applies
the plan (`manifest` via `toml_edit`; each `GateEdit` by inserting
`attribute_text()` on its own line above the declaration at `location`,
last line first so earlier lines keep their numbers), runs
`cargo <job.cargo_args()>` in the copy, and returns
`parse_cargo_json(stdout)`. A `gate` subcommand: read `klepto.toml`, scan,
`converge`, print the plan (and `--write` to apply the final plan to the
real tree).

## Out of scope

Writing files or running cargo from klepto; removing or loosening gates;
gating across packages (a member gated by another member's features);
choosing `dep:` vs implicit features for the user; moving code between
modules; rewriting imports; `cfg_attr` proposals; anything the compiler
rejects for reasons other than unresolved names (type errors after gating
are reported to `human`).

## Fixture

`tests/fixtures/plan/` (app + optional `alpha`, optional `beta`, dev-dep
`devd`; `klepto.toml` moves `beta` under feature `a`) — one annotated line
per rule in `app/src/lib.rs` with stable line numbers, plus
`tests/fixtures/oracle/no_default.jsonl`, a recorded cargo run that blames
line 16 (`ok_dev`) for `alpha`. Assertions in `tests/phase4.rs`; add cases
there, do not weaken them.
