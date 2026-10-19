# Changelog

## 0.3.1 (pending, not published)

Four things a consumer reading a workspace needed (literals with their calls, packages, matches, a fingerprint), and two that kept klepto from finishing on a large workspace. Built against the kit's workspace (1,936 files, about a million lines): before, it did not finish (one run passed 100 GB of memory on five members); now the whole workspace parses in 22 s at 4.8 GB, dependency macros expanded.

### String literals and the call they are an argument of

- `LiteralOccurrence` and `Klepto::literals`: every string literal in code with its location, enclosing function and item, and `arg_of`, the innermost call or method call it is written in the arguments of: src/model.rs (`ArgOf`, `LiteralOccurrence`), recorded by the occurrence visitor at src/syntax/extract.rs (`visit_lit_str`, `push_literal`).
- The occurrence visitor walks a call's arguments under that call: the function or receiver first, then each argument with the call on a stack, so a literal or path deep inside an argument expression knows the call (src/syntax/extract.rs, `visit_expr_call` and `visit_expr_method_call`). Before, both handed the whole node to `syn::visit` and the call was not known below it: [extract.rs L1604-L1635 at 0.3.0](https://github.com/jameshuntt/klepto/blob/8eace60/src/syntax/extract.rs#L1604-L1635).
- `PathOccurrence::arg_of`: the same link for paths, so a constant passed to a call is found by the call.
- After resolution, each call's resolution is copied onto the literals and paths in its arguments, matched by the call's location, kind and, for a method (which starts where its receiver does), its name (src/klepto.rs, `link_arguments`).
- `MacroInvocation::token_paths`: the paths written inside a macro's tokens, by the helper attributes already used (`paths_in_tokens`); the literals in an unexpanded macro's tokens are recorded with `in_macro`.
- Attributes are not descended into, as before, so doc comments are not literals.

### Where a path is written

- `PathOccurrence::role`: `StructLiteral` for a struct literal's own path, `Pattern` inside any pattern (`let`, arms, `if let`, parameters), `Other` elsewhere (src/model.rs `PathRole`, set in src/syntax/extract.rs `visit_pat` and `visit_expr_struct`). "Who builds this type" no longer counts the patterns that take it apart.

### Attribute arguments

- `AttrOccurrence::args`: the argument tokens as written (`secret`, `"platform"`, `note = "use b"`), beside the paths already recorded (src/model.rs, src/syntax/extract.rs `push_attr`). A consumer reading an attribute's values (an owner's name, a threshold) no longer has nothing but paths.

### Variants and macro tokens

- `VariantDecl::location` and `VariantDecl::attrs` (`(path, argument tokens)`, docs left out): a code table declared as an enum (`#[liaise(code = 1, msg = "..")]` on each variant) is readable from klepto's data (src/typing/types.rs, src/syntax/extract.rs). The receipt relocates the variant locations like every other.
- `MacroInvocation::tokens`: the macro's tokens as written, for a table a macro declares (`liaise_codes! { .. }`).

### Packages

- `PackageInfo` / `TargetInfo` and `Klepto::packages`: every workspace member's name (both spellings), version, manifest path, directory and targets, from the cargo metadata `scan_workspace_root` already reads (src/model.rs, src/klepto.rs). A consumer no longer runs cargo metadata a second time to find a package's binaries.

### Match expressions

- `MatchOccurrence` and `Klepto::matches`: every `match` with its scrutinee as written and each arm's pattern paths, resolved from the path occurrences written there, and whether the arm is a catch-all (`_`, or a lowercase binding): src/model.rs, recorded at src/syntax/extract.rs (`visit_expr_match`, `visit_arm`), linked in src/klepto.rs (`link_arguments`). "Every match on this enum" is now a query over klepto's data.

### Fingerprint

- `KleptoBuilder::fingerprint`: the files a scan would read (its `.rs` files, every member's manifest, the root manifest and lockfile) with the blake3 of each and one digest over them, without parsing (src/klepto.rs). The file selection is `parse`'s own, moved into `candidates` so the two cannot disagree. Equal fingerprints mean the same scan; a consumer can reuse what it read before.

### Glob imports resolved once per module

- A module with two or more glob imports whose roots nothing in scope binds resolved each glob's path through the module's other globs, and those through the first, until `MAX_DEPTH` (32): 2^32 resolutions for two globs. A module with two such globs never finished. [resolve/mod.rs L858-L875 at 0.3.0](https://github.com/jameshuntt/klepto/blob/8eace60/src/resolve/mod.rs#L858-L875).
- Now what each glob of a module names is resolved once and kept (`Resolver::glob_targets`); a glob whose path can only come through the module's own globs is a cycle and stays unresolved (src/resolve/mod.rs, `via_globs`). Nothing that resolved before resolves differently; what resolved only by running out of depth is unresolved, as it was.

### Method typing keeps a local's descriptor small

- A local's descriptor is the receiver expression with earlier locals substituted, and it is copied wherever the local is used. `let b = a.f(&a)` holds `a`'s descriptor twice, so a run of such lets doubles at each step; a command-builder crate of 7k lines took more than 9 GB in 20 s: [typing/methods.rs L483-L491 at 0.3.0](https://github.com/jameshuntt/klepto/blob/8eace60/src/typing/methods.rs#L483-L491).
- Now a local whose descriptor has more than `MAX_DESCRIPTOR` (512) nodes is bound as unknown, in `let` statements and in block tails (src/typing/methods.rs, `D::capped`). Its method calls stay unresolved; nothing is claimed.

### Proof

- tests/literals.rs (7 tests, fixtures tests/fixtures/literals, globcycle, doubling): a literal's call and the call's resolution (`std::env::var("APP_KEY")`, `env_or(ENV_SOCKET, "/run/app.sock")`), a const initialiser as the literal's item, a path argument's call, literals and paths in macro tokens, doc comments not recorded, four cyclic globs finishing with the name unresolved, forty doubling lets finishing with the first calls typed.
- tests/literals.rs also: the arms of two matches on an enum, resolved, with the catch-all marked; a fingerprint equal on the same files and different after an edit; a workspace fingerprint holding its manifests, an attribute's argument tokens, a path's role in a literal and a pattern, a variant's place and attributes and a macro's tokens (13 tests in the file).
- tests/packages.rs (2 tests): members' manifests, roots and targets; none for a folder scan.
- `cargo test`: 163 tests, all pass (148 before).
- examples/scale.rs: time and memory on a workspace or some of its members.
