# Phase 8 rules: macro expansion and receiver typing

Phase 8 makes macro-generated code visible and types more method receivers.
klepto still never runs code: only `macro_rules!` (token rewriting) is
expanded; procedural macros are recorded as opaque.

## X1 — what is expanded

Every `macro_rules!` defined anywhere in a scanned package is expanded at
every invocation in that package, in item, impl-item, trait-item, statement
and expression position, recursively (depth limit 32). Expansion happens
before the module tree is built and before anything is extracted, so a
`mod x;`, `impl`, `fn` or method call a macro produces is an ordinary one
from then on (rule M8 still applies to macros klepto cannot expand).

## X2 — where definitions come from

Lookup for a bare name: the current package's definition, then a declared
dependency's, then any known one. `crate_name::name!` picks that crate and
no other (hyper's local `trace!` wraps `tracing::trace!`; falling back to
the local definition would expand forever), so a qualified name whose crate
was not harvested is `ProcMacroOrUnknown`. A `macro_rules!` that an
expansion itself defines (serde's `tri!` comes out of `crate_root!`) is
found by expanding again from the parsed files until no new definition
appears (at most four rounds).
`$crate` becomes `crate` for a local macro and the crate's name for a
dependency's. Dependency macros are `#[macro_export] macro_rules!` read from
the sources `cargo metadata --frozen` reports (lockfile required, cargo never
writes into the scanned tree); only declared dependencies are read, only
files containing `macro_export` are parsed. When the lockfile cannot be
resolved, `Klepto::dep_macro_note` says so and dependency macros are simply
unknown. `KleptoBuilder::skip_dependency_macros(true)` turns the harvest off.

## X3 — matching

The first rule that consumes the whole input wins. Fragments parse with syn,
greedily, as rustc does (`ty`, `expr`, `pat`, `path`, `item`, `stmt`, `meta`,
`vis`, `block`, `ident`, `literal`, `lifetime`, `tt`). A repetition takes the
longest run that lets the rest of the rule match. No matching rule →
`Outcome::Unmatched`, the invocation stays as written.

## X4 — built-ins

`cfg_if!` is expanded exactly: branch *n* gets
`#[cfg(all(not(c1), …, not(cn-1), cn))]`, the `else` branch
`#[cfg(all(not(c1), …))]`. Standard-library macros (`println!`, `vec!`,
`format!`, `assert!`, …) are recorded as `Outcome::Builtin` and left alone:
they produce no items and no scanned code.

## X5 — invocation attributes carry over

`#[cfg]`, doc comments and other attributes written on an item-position
invocation are put on every item it produces (that is how cfg_if branches
and `cfg_feature! { … }` wrappers gate their items). The *definition's*
`#[cfg]`s (on the `macro_rules!` item and its enclosing inline modules) are
put on every product too: what syn's `full!` produces exists only under
`feature = "full"`, and its twin under `not(feature = "full")` is not the
definition klepto took (the first definition of a name in a crate wins).

## X6 — generated marker

Every produced node carries `#[doc = "klepto:generated:<macro>!"]`.
`ItemDecl::generated` and `CapturedFn::generated` hold `<macro>!`; items
nested inside a generated item inherit it; for nested expansions the
outermost invocation names the generator. The marker is not documentation
(`has_docs` ignores it).

## X7 — location

Tokens a rule transcribes are re-spanned to the invocation, so a generated
item is located at the invocation's line. Fragments bound from the
invocation keep their own spans.

## X8 — the ledger

`Klepto::expansions` lists every invocation met, with `Outcome::Expanded {
produced }`, `Unmatched`, `ParseError`, `ProcMacroOrUnknown`, `DepthExceeded`
or `Builtin`. An expanded invocation is still a `MacroInvocation` occurrence
(the first produced node carries it), so R12 resolution, `enclosing_fn` and
the item graph's `Reference` edge to the definition are unchanged.

## T1 — what a receiver can be

Besides scanned types, generics and external names (Phase 5), a receiver
can be a standard-library shape — `Vec`/slices/arrays, `String`/`str`,
`Option`, `Result`, `HashMap`/`BTreeMap`, sets, iterators, `RefCell`/
`Cell`/`Mutex`/`RwLock`/`OnceCell`/`LazyLock`, map entries, scalars — a
tuple, or a trait (`dyn Trait` / `impl Trait`: the first bound; iterator
bounds become the iterator shape with their `Item`). `Box`, `Rc`, `Arc`,
`Cow`, `Pin` and guards are transparent. A std receiver keeps the id the
resolver gives its spelling (`std::Vec`, `std::collections::HashMap`), so
one type has one id in every record.

## T2 — what the standard library returns

A fixed table says what each known method of each shape returns
(`unwrap` → the element, `get` → `Option` of it, `iter` → the iterator,
`keys`/`values`, `entry().or_insert`, `borrow`/`lock`, `len`, `chars`,
`parse::<T>`, `collect::<C<_>>`, …), plus the constructors `Some`/`Ok`/
`Err`/`None`, `Vec::new`, `String::from`, `HashMap::with_capacity`, the
literals, `format!`/`vec!`, `?` on `Option`/`Result`, and indexing of
sequences, maps and strings. Scanned return and field types are read in
full (`-> Vec<Item>` flows into `.iter().map(|i| …)`), the callee's own
generic parameters dropped. A method the table does not know returns
nothing: the chain stops there, untyped.

## T3 — patterns bind parts

`let`, `let … else`, `if let`, `while let`, `match` arms, `for`, closure
and fn parameters bind their names to parts of the scrutinee:
`Some(x)` / `Ok(x)` / `Err(e)`, tuple positions, struct fields of scanned
types (also tuple structs), slice elements, references, `@` bindings, the
first alternative of an `|` pattern. What cannot be projected binds Unknown
(shadowing whatever the name meant before).

## T4 — closure parameters

A closure passed to an adaptor of a std shape gets its parameters typed
from the receiver: the element for `map`/`filter`/`for_each`/`find`/…,
the error for `map_err`/`or_else`, `(k, v)` for maps, the accumulator
unknown for `fold`/`scan`. Sites are keyed by position; two closures at
one position (macro-expanded code) are left unknown rather than guessed.

## T5 — closure results

`map`, `and_then`, `filter_map`, `flat_map`, `find_map`, `map_err`,
`or_else`, `map_or`, `scan` take their result type from the closure's
body (its tail expression, with the parameters bound as in T4) or from a
written `-> T`.

## T6 — typing not done

Arithmetic on scanned types, `.into()`/`.try_into()`, type aliases,
`Deref` and `IntoIterator` of scanned types, blanket impls, supertraits,
trait-provided methods on scanned types (the `clone` family excepted),
containers of other crates (`parking_lot::Mutex`, `smallvec`, …),
`async` outputs, `match` as a value. Those calls keep
`Unresolved { MethodCall }` and no receiver type.

## Not done (on purpose or not yet)

- Procedural macros (derive, attribute, function-like) run code; klepto does
  not. Their invocations are `ProcMacroOrUnknown`; items they generate do not
  exist for klepto.
- Macros re-exported through an intermediate crate (`pub use dep::m;`) are
  not found under the re-exporting name.
- A `macro_rules!` that is itself produced by an expansion is not added to
  the table.
- Declarative `macro` 2.0 items are not expanded.
- Hygiene is ignored: names are textual.
- `#[macro_export(local_inner_macros)]` is treated as a plain export.
