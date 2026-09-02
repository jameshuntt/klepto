###
---

### code_passenger 0.2.0 — scans a workspace with klepto, then does the writing klepto never does:
### feature gates, cargo edits, headers, reports.

###
---

### the feature-gating loop (new in 0.2.0)

write a spec that says what you want the feature model to look like:

```toml
# klepto.toml — move `beta` behind a feature `b` in package `app`
[app]
optional = ["beta"]
default = []

[app.features]
b = ["dep:beta"]
```

then:

```bash
# what would change (plan + diff), nothing written
code-passenger --root . gate --spec klepto.toml

# CI: exit 1 while there is gating left to do
code-passenger --root . gate --spec klepto.toml --check

# prove it with real `cargo check` on a scratch copy, then write it
code-passenger --root . gate --spec klepto.toml --write
```

what happens:

1. klepto plans the gates: every use of `beta` that is not already behind
   `feature = "b"` gets a `#[cfg(feature = "b")]` on its carrier, and
   everything that depends on those carriers gets one too (the cascade).
2. the cargo oracle copies the tree to a scratch dir (minus `target`,
   `.git`, `.passenger`), applies the plan there, and runs `cargo check`
   for every feature combination that matters. errors the plan missed come
   back as forced gates; it goes again until everything compiles.
3. only then, and only with `--write`, the real files change: `#[cfg]`
   lines above the items (an existing `#[cfg(unix)]` becomes
   `#[cfg(all(unix, feature = "b"))]`, never two attributes stacked) and
   `Cargo.toml` edits through `toml_edit` so your formatting survives.

exit codes: `0` nothing to do or converged, `1` `--check` found work or
the oracle gave up, `2` something a person has to decide (the plan lists
it under `unknown` / `human`). nothing is written on 1 or 2.

###
---

### review before you gate

```bash
code-passenger --root . feature-map                          # crates, features, deps, use-site counts
code-passenger --root . feature-sites beta                   # every use site of a dep (or of a feature's deps), by file, with cfg
code-passenger --root . feature-probe beta --spec klepto.toml   # what is ungated, what would be gated, what gets lost; never writes
```

`--json` on any of them prints the structure instead of the text.

###
---

### the rest of klepto, on the command line

```bash
code-passenger --root . receipt --out receipt.json --kg kg/   # the versioned receipt + knowledge-graph ndjson
code-passenger --root . lint [--policy policy.toml]           # klepto's analyses under a policy; exit code = verdict
code-passenger --root . patterns --top 20                     # repeated code: extract / parameterize / generic / trait / macro candidates
```

the default lint policy denies ungated dependency uses (KLEP010) and
cfg-edge violations (KLEP011) and allows the noisy info rules. a policy
file looks like:

```toml
[levels]
KLEP013 = "deny"     # unreferenced private items fail the build
KLEP001 = "allow"

[max]
KLEP002 = 0          # no unwrap in public API
```

###
---

### the original job: headers

### check out report.json, it was made by doing this:
```bash
cargo run -- --root ../classified --src src --json scan
```

### if you wanna see the crate it was made from, go check out:

```txt
https://github.com/jameshuntt/classified
```

### it is designed to make file headers and documentations that explains itself
### it is intended to be a templating engine for max flexibility

# intended to produce things like this on command:

```txt
//! ----------------------------------------------
//! DOCUMENT DETAILS -----------------------------
//! 
//! filename:thread_pool_manager.rs
//! description:
//! usages:none in crate yet
//! 
//! ----------------------------------------------
//! FEATURE NOTES --------------------------------
//! 
//! feature_name:async
//! deps:[tokio][async_trait]
//! scope:[impl ThreadPoolManager]
//! corpus:true
//! 
//! feature_name:std
//! deps:[std]
//! scope:[impl ThreadPoolManager]
//! corpus:false
//! 
//! ----------------------------------------------
//! CORPUS FEATURES ------------------------------
//! 
#![cfg(feature = "async")]
#![cfg(feature = "std")]
```

`scan`, `annotate`, `scaffold` and `passenger` (the checkpoint store) are
unchanged in 0.2.0.

###
---

### notes

- klepto is a path dependency until klepto 0.2.0 is on crates.io.
- the oracle runs `cargo` offline; path dependencies only, nothing is
  downloaded and nothing outside the scratch copy is touched.
- the rules every command follows, with the fixture and tests that pin
  them, are in `GATE_RULES.md` and `tests/gate.rs`.
