# klepto Phase 7 — pattern mining

Goal: find repeated structure and say what it could become. Functions are
reduced to shapes at five abstraction levels; functions sharing a shape
form a cluster; the slots whose fillers differ across a cluster are its
parameters; each cluster becomes one candidate — extract a function, pass
the literals, make it generic, introduce a trait, or write a
`macro_rules!`. Impl blocks are mined for shared method sets and repeated
trait impls. Every function gets a few cheap metrics. Patterns join the
knowledge graph in langbrain's own `pattern` / `realizes` vocabulary.

klepto proposes; it never rewrites. Everything here is deterministic and
pure. Mining needs `span-locations` (the default feature): the shaper
classifies tokens by span, so without it only the `Exact` level exists.

Definition of done: `cargo test` stays green (Phases 0–6) **and**
`cargo test --test phase7 -- --ignored` is green. `UseSite`, `UseSiteKind`,
`UseSitesCount` and the `KleptoBuilder` calls used by `code_passenger` do not
change shape.

Where to implement: `src/patterns.rs` `mine` (everything below), the two
rules in `src/rules/patterns.rs`, and the Phase 7 additions to
`kg::export` (K10). `PatternReport`'s helpers are real already.

---

## S — shapes

- **S1 Levels** (`ShapeLevel`; the pipeline's `Level` is a different
  thing). Every function (free fns, inherent and trait-impl methods,
  trait default methods, nested fns) with at least `min_tokens`
  significant tokens gets one shape per level, strictest first:
  `Exact`, `Renamed`, `Literals`, `Types`, `Callees`. Each level
  includes every abstraction of the levels before it. A shape covers the
  **signature inputs and output plus the body**, never attributes, doc
  comments or the fn name.
- **S2 Tokens.** The shape is the significant tokens joined by single
  spaces: identifiers, keywords, literals, punctuation (joint punctuation
  stays one token: `::`, `->`, `+=`, `&&`), and delimiters `{ } ( ) [ ]`.
  Whitespace and comments do not exist. Tokens inside macro invocations
  (`name ! ( .. )`) are opaque: kept verbatim at every level (a documented
  limitation); the macro name itself is a callee.
- **S3 Abstractions.**
  - `Renamed`: every name bound in the signature or the body — parameters,
    `let` bindings, closure parameters, pattern bindings — becomes `$vN`,
    numbered by first occurrence; every use of that name follows. Scoping
    is ignored (same name → same slot). `self` stays `self`. Field and
    method names are never renamed.
  - `Literals`: every literal becomes `$lN`, numbered by first occurrence
    of its **value** (the same literal twice is the same slot).
  - `Types`: every identifier in type position, every path segment that
    starts with an uppercase letter (types, variants, traits, constants
    spelled that way — over-approximation is fine), and every primitive
    type name becomes `$tN`, numbered by first occurrence of the name.
    `Self` stays `Self`. Field names after `.` are never abstracted.
  - `Callees`: the last segment of every call path, every method name in
    a method call, and every macro name becomes `$fN`, numbered by first
    occurrence of the spelling (full path for path calls).
- **S4 Slots.** For each level the shaper records, in first-occurrence
  order, `(placeholder, filler)` — what each `$vN`/`$lN`/`$tN`/`$fN`
  stood for in that function.
- **S5 Hashes and threshold.** `hash` = blake3 hex of the shape. `tokens`
  = significant tokens of signature + body; functions below
  `min_tokens` (default 16 — a one-expression getter with its signature
  is about 12) are not shaped and never cluster. Empty bodies never
  cluster whatever the threshold.

## C — clusters

- **C1** A cluster is a level plus `min_members` (default 2) or more
  functions with the same hash at that level; `members` sorted, `exemplar`
  = the first member.
- **C2 Tightest.** A cluster is `tightest` when no stricter level has a
  cluster containing all of its members. Clusters at every level are
  reported; consumers usually want the tightest ones.
- **C3 Order.** Clusters sorted by `(level, members.len() desc, hash)`.
- **C4 Parameters** (rule P1) are computed for every cluster at `Literals`
  and above; `Exact`/`Renamed` clusters have none.
- **C5 Determinism.** Same tree, same options → same report, byte for
  byte (`to_json_string`).
- **C6 Impl methods** cluster like any function; their ids are the Phase 1
  method ids.

## P — parameters and candidates

- **P1 Params.** For a cluster at level `L`, align members by placeholder
  (S4). A slot is a parameter when its fillers are not all equal across
  members; `kind` by placeholder family (`$l` literal, `$t` type, `$f`
  callee); `fillers` one per member in `members` order. Renamed slots
  (`$v`) are never parameters.
- **P2 Candidate per tightest cluster**, in this precedence:
  1. every member is an inherent method (Phase 1 `ImplMethod` with no
     trait) with the same name on at least two distinct self types →
     `Trait`, with `method` and `types` (the self type ids, sorted);
  2. level `Exact` or `Renamed` → `ExtractFn`;
  3. level `Literals` → `Parameterize`;
  4. level `Types` → `GenericFn`;
  5. level `Callees` → `MacroRules`.
  `note` is one plain sentence saying what to do.
- **P3 Required methods** (`GenericFn` only): for each type parameter
  `$tN`, the union over members of method names called on receivers
  whose Phase 5 `receiver_type` is that member's filler of `$tN`, sorted.
  Empty when Phase 5 could not type the receivers.
- **P4 Weight** = `members.len() × exemplar tokens`; candidates sorted by
  weight descending, then by first member.

## I — impl mining

- **I1 Shared method sets.** Over inherent impls of every scanned type,
  for every set of two or more types, the method names they all provide;
  report each **maximal** `(methods, types)` pair with at least two
  methods: no other reported set has both a superset of the types and a
  superset of the methods. Sorted by `(types.len() desc, methods.len()
  desc, types)`.
- **I2 Repeated trait impls.** For each trait id implemented by two or
  more types, group the impls whose method bodies match method-for-method
  at some level (the threshold S5 does **not** apply here); report the
  strictest level at which a group of two or more agrees, with `types`
  and `impls` sorted. One record per (trait, group).
- **I3 Ids.** Types and impls are named by their Phase 5 record ids.

## M — metrics

- **M1** `tokens` as S5; `lines` = last line − first line + 1 of the fn
  item (attributes included); `branches` = `if` + `while` + `loop` +
  `for` + (`match` arms − 1) + `?` + `&&` + `||`, macro bodies opaque;
  `calls` = call + method-call + macro-invocation expressions;
  `max_depth` = deepest `{ }` block nesting inside the body (the body is
  depth 0, a `for` body 1, …).
- **M2** `fan_out` = `ItemGraph::dependencies_of(id).len()`, `fan_in` =
  `dependents_of(id).len()`.
- **M3** Every function with a shape or not gets metrics (the threshold
  does not apply); keyed by item id.
- **M4** `lines` is `0` without `span-locations`.

## K10 — knowledge graph additions (in `kg::export`)

- `pattern` nodes for every **tightest** cluster: `id =
  pattern/<level>/<first 16 hex of hash>`, `level`, `hash`, `signature`
  (the shape, cut to 500 chars), `exemplar` (item id), `support`
  (member count), `description`. langbrain's `pattern` record uses the
  same words (`signature`, `exemplar`, support) on purpose.
- `realizes` edges `item/<member> → pattern/…` for every member of a
  tightest cluster (langbrain: `lang realizes pattern`; klepto: `item
  realizes pattern`).
- Challenge class `same_shape` (`item`, `level`): one per tightest
  cluster, item = the exemplar; expected = the other members, sorted, as
  a JSON array. Same digest/split rules as K7. Default `MiningOptions`.

## R — rules

- **R1 `KLEP016 duplicate-function`** — Info; one finding per tightest
  cluster at `Exact` or `Renamed`, at the exemplar's location; extra
  `{level, hash, members}`.
- **R2 `KLEP017 repeated-impl`** — Info; one per `RepeatedImpl`, at the
  first impl's location; extra `{trait, level, types, impls}`.

## Out of scope (say so in comments)

Rewriting anything; semantic equivalence beyond token shapes (reordered
statements, renamed fields); clone detection inside macro bodies; cross-
crate patterns (the receipt of each crate is the unit); suggesting names.

## Fixture

`tests/fixtures/patterns/` — one package `app` with: exact duplicates
(`sum_a`, `sum_b`), a renamed twin (`sum_c`), a literal twin pair
(`scale_a`, `scale_b`), a type twin pair with Phase 5-typed receivers
(`check_engine`, `check_motor`), a callee twin pair (`apply_a`,
`apply_b`), two tiny fns under the threshold (`double`, `triple`), three
types whose inherent impls share method sets (`Engine`, `Motor`, `Pump`),
same-named `describe` methods (trait candidate), a trait `Named`
implemented identically for two types, and `complex` for the metrics
(lines 51–64). Assertions in `tests/phase7.rs`; add cases there, do not
weaken them.
