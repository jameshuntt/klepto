# klepto

klepto walks a Rust crate or workspace with `syn` and collects everything a
tool would want to know about it — items, imports, calls, attributes, the
module tree, what every path actually names, cfg predicates, the Cargo
feature model, the dependency graph between items, types and impls, method
receivers, repeated code — and hands it over as plain data. It never writes
a file, never runs cargo, never guesses. Consumers such as
[`code_passenger`](https://crates.io/crates/code_passenger) do the acting.

Two rules run through the whole crate: **over-approximate, never drop**, and
**never claim on unknown**. A path klepto cannot resolve is reported as
unresolved with its reason and candidates; it is not silently skipped and
not turned into a guess.

## Quick start

```rust,no_run
use klepto::Klepto;

let k = Klepto::new("app")
    .scan_workspace_root(".")?      // cargo metadata --no-deps; no network
    .exclude_generated()?           // *.pb.rs, bindings.rs, ...
    .parse()?;

println!("{} files, {} fns, {} items, {} edges",
    k.files.len(), k.functions.len(), k.items.len(), k.graph.edges.len());
# Ok::<(), klepto::KleptoError>(())
```

`scan_in_folder` and `scan_file` work without a manifest; then there is no
feature model and no dependency list, and use sites of other crates are
reported as *possible* rather than certain.

## What it collects

**Before layer 0 — expansion.** `macro_rules!` invocations are expanded
(local definitions, `cfg_if!`, and `#[macro_export]` macros of declared
dependencies read from the cargo registry when the lockfile resolves
offline), so generated items, impls, modules and calls are ordinary records
marked `generated: Some("name!")`. `Klepto::expansions` lists every
invocation with its outcome (`Expanded`, `Unmatched`, `ParseError`,
`ProcMacroOrUnknown`, `Builtin`); procedural macros stay opaque.

**Layer 0 — occurrences.** Functions with signatures, `use` items (module
level and inside bodies), macro definitions and invocations, paths, calls,
attributes with their argument paths, `extern crate`. Unparsable files are
skipped and listed when you ask for that (`ignore_parse_errors(true)`).

**String literals.** Every string literal in code (`"NODEPLAN_X"`, raw
strings, the tokens of a macro klepto did not expand), with its enclosing
function and item (a `const` it initialises is its item) and `arg_of`: the
innermost call or method call it is an argument of, with that call's
resolution. Paths carry `arg_of` too, and every macro invocation lists the
paths written in its tokens (`token_paths`), named there but never
resolved. Doc comments and attribute arguments are not code and are not
recorded. This is what a reader of an env key needs: `std::env::var(KEY)`
and `env_or("KEY", ..)` are the literal or the constant, and the call.

**Packages.** `Klepto::packages`: every workspace member as cargo metadata
states it — its name both ways, version, manifest, directory and targets
(`bins()` for its binaries).

**Layer 1 — resolution.** The module tree (`mod x;`, `#[path]`, inline
modules, `mod.rs` and `x.rs` styles, orphans), per-module alias tables, and
one `Resolution` per occurrence: `Local("app::store::Engine")`,
`External("serde::Serialize")`, or `Unresolved { raw, reason, candidates }`.
Ids are canonical: `package::module::name`, `module::Ty::method`,
`module::<Ty as Trait>::method`.

**Layer 2 — cfg and features.** Every record carries its effective
`CfgPredicate` (its own `#[cfg]` and every enclosing one, `cfg_attr`
expanded). The Cargo feature model per package (features, `dep:`, weak
features, implicit features, optional and dev and target dependencies) with
queries such as `dep_predicate`, `deps_enabled_by`, `implies`, and
`ungated_dep_uses` — the places where a dependency is used under a cfg that
does not guarantee it is there.

**Layer 3 — the item graph.** One node per item of every kind, edges for
references, imports, re-exports and containment, with `dependents_of`,
`dependencies_of`, transitive closures, `affected_by_feature`,
`affected_by_dep`, and `cfg_edge_violations` (an edge whose condition does
not imply its target's).

**Layer 4 — gate planning.** A `klepto.toml` says what the feature model
should become; `plan_gates` turns the difference into `#[cfg]` attributes
per carrier plus `Cargo.toml` edits, cascading through the graph. A check
matrix and an `Oracle` trait let a consumer prove the plan with real
`cargo check` and feed compiler errors back as forced gates
(`converge`).

**Layer 5 — types and receivers.** Structs, enums, unions, traits, aliases
and impl blocks with fields, variants, generics and bounds, derives and
associated items, all resolved; and method-call receivers typed from what
is written (`self`, typed parameters and lets, constructors, field access,
one level of chaining, generic bounds). Roughly a third of method calls in
ordinary code get a receiver type this way; the rest stay unresolved.

**Layer 6 — the receipt.** `Receipt::from_klepto` is one versioned,
digested, relocatable JSON document with every layer above, in a canonical
order (same tree, same bytes). `stale_files` and `diff` compare receipts.
`kg::export` emits the same facts as knowledge-graph records and
challenges with exact answers, in a vocabulary shared with other collectors.
`run_pipeline` runs every analysis under a `Policy` and returns findings,
counts and an exit code.

**Layer 7 — patterns.** Function shapes at five abstraction levels (exact,
renamed locals, literals, types, callees), clusters of same-shaped
functions, the slots that differ inside a cluster, and one proposal per
cluster: extract a function, pass the literals, make it generic (with the
methods the type parameter needs), introduce a trait, or write a
`macro_rules!`. Plus shared method sets across types, repeated trait impls,
and per-function metrics.

```rust,no_run
use klepto::{receipt::kg, run_pipeline, Klepto, Policy, Receipt};

let k = Klepto::new("app").scan_workspace_root(".")?.parse()?;

// the facts, as a document
let receipt = Receipt::from_klepto(&k);
assert!(receipt.verify_digest());

// the facts, for a knowledge graph
let bundle = kg::export(&k);
let ndjson = kg::to_ndjson(&bundle.knowledge);

// the analyses, as a check
let policy = Policy::parse("[levels]\nKLEP010 = \"deny\"\n")?;
let verdict = run_pipeline(&k, &policy);
println!("{}", verdict.summary());

// repeated code
for c in k.patterns().candidates.iter().take(5) {
    println!("{:?} {}: {}", c.kind, c.members.join(", "), c.note);
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Rules

- `KLEP001`–`KLEP004` — undocumented public API, `unwrap`/`expect` in
  public API, panicking macros in public fns, `std` in a `no_std` crate.
- `KLEP010` ungated dependency use, `KLEP011` cfg-edge violation,
  `KLEP012` unresolved reference, `KLEP013` unreferenced private item,
  `KLEP014` orphan file, `KLEP015` skipped file, `KLEP016` duplicate
  function, `KLEP017` repeated trait impl, `KLEP018` / `KLEP019` the
  cfg-edge and dependency-use cases klepto cannot decide (an unknown cfg
  is involved) — reported apart so a policy can deny the definite ones.

A policy gives each code a level (`allow`, `warn`, `deny`) and optional
counts; the verdict's exit code is 1 when a denied code has findings or a
count is exceeded.

## What it will not do

- Guess. Method calls on receivers it cannot type, paths only a glob
  import could supply, anything a procedural macro would generate — all
  reported as unresolved (or opaque), never resolved by luck.
- Run code. `macro_rules!` is expanded (local ones, `cfg_if!`, and the
  `#[macro_export]` ones of dependencies whose sources are on disk), so
  what those macros generate exists and is marked `generated`; derive,
  attribute and function-like proc macros are recorded as opaque
  (`Klepto::expansions` lists every invocation with its outcome).
- Type-check. Receivers are typed from what is written — parameters,
  `let` annotations, constructor and method return types, fields, `self`,
  patterns (`let`/`if let`/`match`/`for`), `?`, indexing, closure
  parameters of iterator/option adaptors, `dyn`/`impl Trait` — plus a
  table of what the standard library's containers, `Option`/`Result`,
  iterators, cells and scalars return. Arithmetic on your own types,
  `.into()`, type aliases, `Deref`/`IntoIterator` of your own types,
  blanket impls and supertraits are not followed: those calls stay
  unresolved.
- Write or run anything. Applying a gate plan, editing `Cargo.toml`, and
  running cargo belong to the consumer.

Three things it handles on purpose rather than by luck: twin definitions
under complementary cfgs (`feature = "x"` and `not(feature = "x")`) are
one node that is always present, and so is a module whose one file is
declared twice that way (`pub mod ser;` / `mod ser;` in serde_json);
modules declared inside macro invocations (`cfg_rt! { pub mod runtime; }`,
`cfg_if!`) get the exact cfg the expanded macro writes, and when the macro
cannot be expanded they exist with an *unknown* cfg — nothing inside them
is then claimed gated or reported as a violation; and the
undecidable cases get their own rule codes (`KLEP018`, `KLEP019`) so a
policy can deny the definite ones alone.

Tried on syn, serde, serde_json, regex, regex-syntax, hyper, clap,
rayon, rustls, egui and tokio: every file parses, receipts are
deterministic, and a crate of a few hundred files takes seconds.

## Compatibility

`UseSite`, `UseSiteKind` and `UseSitesCount`, and the builder calls
`scan_workspace_root`, `exclude_generated`, `parse`, `dep_use_sites`,
`internal_use_sites`, `imports` are the contract earlier consumers rely on
and are kept stable. Everything since 0.1.3 is additive.
