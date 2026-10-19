# Changelog

## 0.3.1 (pending, not published)

Two things a consumer reading the environment needed, and two that kept klepto from finishing on a large workspace. Built against the kit's workspace (1,936 files, about a million lines): before, it did not finish (one run passed 100 GB of memory on five members); now the whole workspace parses in 22 s at 4.8 GB, dependency macros expanded.

### String literals and the call they are an argument of

- `LiteralOccurrence` and `Klepto::literals`: every string literal in code with its location, enclosing function and item, and `arg_of`, the innermost call or method call it is written in the arguments of: src/model.rs (`ArgOf`, `LiteralOccurrence`), recorded by the occurrence visitor at src/syntax/extract.rs (`visit_lit_str`, `push_literal`).
- The occurrence visitor walks a call's arguments under that call: the function or receiver first, then each argument with the call on a stack, so a literal or path deep inside an argument expression knows the call (src/syntax/extract.rs, `visit_expr_call` and `visit_expr_method_call`). Before, both handed the whole node to `syn::visit` and the call was not known below it: [extract.rs L1604-L1635 at 0.3.0](https://github.com/jameshuntt/klepto/blob/8eace60/src/syntax/extract.rs#L1604-L1635).
- `PathOccurrence::arg_of`: the same link for paths, so a constant passed to a call is found by the call.
- After resolution, each call's resolution is copied onto the literals and paths in its arguments, matched by the call's location, kind and, for a method (which starts where its receiver does), its name (src/klepto.rs, `link_arguments`).
- `MacroInvocation::token_paths`: the paths written inside a macro's tokens, by the helper attributes already used (`paths_in_tokens`); the literals in an unexpanded macro's tokens are recorded with `in_macro`.
- Attributes are not descended into, as before, so doc comments are not literals.

### Packages

- `PackageInfo` / `TargetInfo` and `Klepto::packages`: every workspace member's name (both spellings), version, manifest path, directory and targets, from the cargo metadata `scan_workspace_root` already reads (src/model.rs, src/klepto.rs). A consumer no longer runs cargo metadata a second time to find a package's binaries.

### Glob imports resolved once per module

- A module with two or more glob imports whose roots nothing in scope binds resolved each glob's path through the module's other globs, and those through the first, until `MAX_DEPTH` (32): 2^32 resolutions for two globs. A module with two such globs never finished. [resolve/mod.rs L858-L875 at 0.3.0](https://github.com/jameshuntt/klepto/blob/8eace60/src/resolve/mod.rs#L858-L875).
- Now what each glob of a module names is resolved once and kept (`Resolver::glob_targets`); a glob whose path can only come through the module's own globs is a cycle and stays unresolved (src/resolve/mod.rs, `via_globs`). Nothing that resolved before resolves differently; what resolved only by running out of depth is unresolved, as it was.

### Method typing keeps a local's descriptor small

- A local's descriptor is the receiver expression with earlier locals substituted, and it is copied wherever the local is used. `let b = a.f(&a)` holds `a`'s descriptor twice, so a run of such lets doubles at each step; a command-builder crate of 7k lines took more than 9 GB in 20 s: [typing/methods.rs L483-L491 at 0.3.0](https://github.com/jameshuntt/klepto/blob/8eace60/src/typing/methods.rs#L483-L491).
- Now a local whose descriptor has more than `MAX_DESCRIPTOR` (512) nodes is bound as unknown, in `let` statements and in block tails (src/typing/methods.rs, `D::capped`). Its method calls stay unresolved; nothing is claimed.

### Proof

- tests/literals.rs (7 tests, fixtures tests/fixtures/literals, globcycle, doubling): a literal's call and the call's resolution (`std::env::var("APP_KEY")`, `env_or(ENV_SOCKET, "/run/app.sock")`), a const initialiser as the literal's item, a path argument's call, literals and paths in macro tokens, doc comments not recorded, four cyclic globs finishing with the name unresolved, forty doubling lets finishing with the first calls typed.
- tests/packages.rs (2 tests): members' manifests, roots and targets; none for a folder scan.
- `cargo test`: 157 tests, all pass (148 before).
- examples/scale.rs: time and memory on a workspace or some of its members.
