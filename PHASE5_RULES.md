# klepto Phase 5 — type records and method-call resolution

Goal: collector completeness. klepto knows functions in detail but structs,
enums, unions, traits and aliases only by name, and every `x.foo()` is
unresolved. Phase 5 records every type with its fields, variants, generics,
bounds, derives and associated items, records every impl block, and resolves
method calls whenever local syntax makes the receiver's type obvious — no type
checker, no inference beyond what is written.

Definition of done: `cargo test` stays green (Phases 0–4) **and**
`cargo test --test phase5 -- --ignored` is green. `UseSite`, `UseSiteKind`,
`UseSitesCount` and the `KleptoBuilder` calls used by `code_passenger` do not
change shape.

Guiding principle unchanged: **never guess.** A receiver whose type is not
written or constructed in plain sight stays `Unresolved { MethodCall }` with
`receiver_type = None`.

Where to implement: the occurrence visitor in `src/extract.rs` emits
`TypeDecl` / `ImplDecl` with unresolved `TypeRef`s; `src/types.rs`
`resolve_types` resolves every `TypeRef::mentions`, `derives` and fills
`TypeDecl::impls`; `src/methods.rs` `resolve_method_calls` implements M1–M7.
`normalize_type_spelling` is real. The graph builder already turns a resolved
method call into a `Reference` edge (G1).

---

## Data model (declared in `src/types.rs`, `src/model.rs`)

- `TypeRef { raw, mentions }`, `GenericParam { name, kind, bounds, ty, default }`,
  `FieldDecl { index, name, ty, is_public, cfg, attrs }`,
  `VariantDecl { name, fields, discriminant, cfg }`, `MethodSig`,
  `TypeDecl { … generics, fields, variants, supertraits, trait_methods,
  assoc_types, assoc_consts, aliased, derives, attrs, impls }`,
  `ImplDecl { id, …, generics, self_ty, trait_, methods, assoc_types, assoc_consts }`.
- `Klepto::types`, `Klepto::impls`, `Klepto::type_decl(id)`, `Klepto::impls_of(type_id)`.
- `CallOccurrence::receiver_type: Option<Resolution>`.

## N — spelling

- **N1 `normalize_type_spelling`** (real): tokens joined with no space after
  `<` `(` `[` `&` `*` `::`, none before `>` `)` `]` `,` `;` `::`, none
  between a name (or `>`) and a following `<` or `(`, and a single space
  everywhere else. Examples: `Vec<Option<store::Engine>>`, `&'a str`,
  `&mut [u8]`, `Box<dyn Fn(u8) -> u8>`, `(u8, String)`, `[u8; 4]`,
  `*const u8`, `impl Runner + Send`.

## T — type records

- **T1 `TypeRef`.** `raw` is N1 of the type's tokens. `mentions` lists
  every `syn::Path` inside the type in source order, resolved with the
  Phase 1 rules in the declaring module with the item's generic parameters
  in scope (`T` → `Unresolved{GenericParam}`), the head path first.
  Primitive names (`u8`, `str`, `bool`, …) are not listed; `Self` resolves
  to the enclosing type/impl target; lifetimes are not paths.
- **T2 Structs and unions.** One `FieldDecl` per field in order: `index`,
  `name` (`None` for tuple fields), `ty`, `is_public` (`pub` only), `cfg`
  (field's own `#[cfg]` and the type's), `attrs` (attribute paths).
- **T3 Generics.** One `GenericParam` per parameter in order: lifetimes
  (`'a`, kind `Lifetime`, no bounds), types (inline bounds plus every
  `where` predicate whose bounded type is exactly that parameter, in that
  order), consts (`ty` = the const's type). Defaults recorded when written.
  Bounds are `TypeRef`s (`Runner` → `Local(app::Runner)`, `Default` →
  `External(std::Default)` by the prelude rule). Lifetime bounds are dropped.
- **T4 Enums.** One `VariantDecl` per variant: fields as T2 (tuple or named),
  `discriminant` = the expression tokens after `=` when present.
- **T5 Traits.** `supertraits` = the trait's bounds as `TypeRef`s (`?Sized`
  included as written); `trait_methods` = one `MethodSig` per fn item with
  `id` = the trait method's fq_name, `has_default` = has a body;
  `assoc_types` / `assoc_consts` = names in order. Generic bounds as T3.
- **T6 Derives and attributes.** `derives` = the resolutions of every
  `#[derive(..)]` argument on the type (`Debug` → `External(std::Debug)`,
  `ext::Tag` → `External(ext::Tag)`), in order. `attrs` = attribute paths
  as on `CapturedFn`.
- **T7 Aliases.** `aliased` = the target `TypeRef`.
- **T8 Impls.** One `ImplDecl` per impl block with the graph node id
  (`<module>::<impl Ty>` / `<module>::<impl Trait for Ty>`, trait canonical
  after the F1 rename), `self_ty` and `trait_` as `TypeRef`s, `methods` =
  method node ids in order, `assoc_types` = `(name, TypeRef)`,
  `assoc_consts` = names. `TypeDecl::impls` = byte-order sorted ids of every
  impl whose `self_ty.mentions[0]` is `Local(<type id>)`. Ids and `cfg`
  match the Phase 3 nodes exactly; note the id keeps Phase 1's no-space
  spelling (`Config<'a,T,N>`) while `self_ty.raw` is N1 (`Config<'a, T, N>`).
- **T9 Cross-package.** Types of every scanned package are recorded;
  external types are not (they appear only as `External` mentions).

## M — method calls

For each `CallOccurrence` with `is_method`, inside a function body, the
receiver expression is typed by the rules below (first match wins); the
result goes to `receiver_type`. Then the method is looked up (M2). Anything
else keeps `resolution = Unresolved{MethodCall}` and `receiver_type = None`.

- **M1 Receiver typing.** A per-function environment of local bindings,
  built in source order (later `let`s shadow earlier ones):
  - `self` → the impl's self type (`Local(app::Config)`), or the trait's
    own id inside a trait default method;
  - a parameter with a written type → that type's head mention;
  - `let x: T = …` → `T`'s head mention;
  - `let x = Path::f(..)` where `Path::f` resolves to a method whose written
    return type is `Self` or the impl's type → that type; where it resolves
    to a free fn or method with a written return type → that type's head
    mention (resolved in the callee's module);
  - `let x = Type(..)` / `let x = Type { .. }` where `Type` resolves to a
    struct → that struct; `let x = Enum::Variant(..)` / `Enum::Variant` /
    `Enum::Variant { .. }` where the prefix is an enum → that enum;
  - references and smart pointers are transparent: `&T`, `&mut T`,
    `Box<T>`, `Rc<T>`, `Arc<T>` type as `T`;
  - a generic parameter of the fn or impl → `Unresolved{GenericParam}` with
    its bounds available for M6.
- **M2 Method lookup.** A receiver is `Local(T)` when `T` has a `TypeDecl`
  — also when the path resolved `External` because `T` lives in another
  scanned workspace member (the Phase 3 node rule). Then: an inherent impl
  of `T` (`ImplDecl` with `trait_ = None`, `self_ty.mentions[0] == Local(T)`)
  declaring a method named `m` → that method's id; else exactly one trait
  impl for `T` declaring `m` → that method's id; several →
  `Unresolved{Ambiguous, candidates}`; none → `Unresolved{MethodCall}`.
  A trait receiver (`self` in a default method, M6) looks up the trait's
  own declarations. Receiver `External(E)` with no `TypeDecl` →
  `External(E::m)` (unverified, over-approximated). A `Local` name without
  a `TypeDecl` (an associated type projection, a const) types nothing.
- **M3 Chaining.** A receiver that is itself a method call resolved by M2 to
  a method with a written return type is typed by that return type (M1's
  return-type rule); one level per call, applied left to right.
- **M4 Field access.** A receiver `recv.field` / `recv.0` where `recv`
  types to `Local(T)` and `T`'s `TypeDecl` has that field → the field's
  `ty` head mention. Nested field access applies the rule repeatedly.
- **M5 UFCS** `Type::method(x)` is a path call, already resolved by Phase 1;
  unchanged.
- **M6 Generic receivers.** Receiver typed as a generic parameter whose
  bounds include a `Local` trait declaring `m` → the trait's method
  declaration id (`app::Runner::run`); `receiver_type` =
  `Unresolved{GenericParam}` with `raw` = the parameter name.
- **M7 Unknown stays unknown.** Closure results, iterator chains, index
  expressions, method calls on untyped `let`s, deref of anything but M1's
  list, trait objects, and `impl Trait` return types are not typed.

## G — graph

- **G1** A method call whose `resolution` is `Local`/`External` becomes a
  `Reference` edge from the enclosing item (already in the builder); the
  rest stay in `graph.unresolved`. With M4, `self.engine.tick()` yields
  `<Config as Runner>::run → app::store::Engine::tick`-style edges the
  earlier phases could not see.

## Out of scope (say so in comments)

Type inference of any other kind (arithmetic, generics instantiation,
closures, iterator adaptors, `?`); deref coercion beyond M1's list; trait
method resolution through blanket impls or supertraits; auto-ref rules;
method calls on trait objects; macro-generated types and impls.

## Fixture

`tests/fixtures/types/` — `app` (+ dep `ext`): a generic struct with
lifetime/bounded/const params and a cfg'd field, a tuple struct, an enum
with a discriminant and named/tuple variants, a union, a trait with
supertraits and associated items, an alias, an inherent and a trait impl,
and one function per M rule (`typed_param`, `let_annot`, `let_ctor`,
`let_literal`, `let_variant`, `let_free_fn`, `generic_bound`,
`external_recv`, `prelude_recv`, `really_unknown`). Assertions in
`tests/phase5.rs`; add cases there, do not weaken them.
