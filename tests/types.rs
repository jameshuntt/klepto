//! Type records and method-call resolution. Fixture: tests/fixtures/types/.

mod common;

use common::*;
use klepto::*;

fn types_ws() -> Klepto {
    Klepto::new("app")
        .scan_workspace_root(fixture("types"))
        .expect("cargo metadata on fixture types")
        .parse()
        .expect("parse fixture types")
}

fn local(s: &str) -> Resolution { Resolution::Local(s.to_string()) }
fn external(s: &str) -> Resolution { Resolution::External(s.to_string()) }
fn feat(s: &str) -> CfgPredicate { CfgPredicate::Feature(s.to_string()) }

fn ty<'a>(k: &'a Klepto, id: &str) -> &'a TypeDecl {
    k.type_decl(id).unwrap_or_else(|| panic!("no type record {id}; have {:?}", k.types.iter().map(|t| &t.id).collect::<Vec<_>>()))
}

fn field<'a>(t: &'a TypeDecl, name: &str) -> &'a FieldDecl {
    t.fields.iter().find(|f| f.name.as_deref() == Some(name)).unwrap_or_else(|| panic!("no field {name} on {}", t.id))
}

fn call<'a>(k: &'a Klepto, fn_suffix: &str, callee: &str) -> &'a CallOccurrence {
    k.calls
        .iter()
        .find(|c| c.is_method && c.callee == callee && c.enclosing_fn.as_deref().is_some_and(|f| f.ends_with(fn_suffix)))
        .unwrap_or_else(|| panic!("no method call `{callee}` in a fn ending with `{fn_suffix}`"))
}

fn is_generic_param(r: &Resolution, name: &str) -> bool {
    matches!(r, Resolution::Unresolved { raw, reason: UnresolvedReason::GenericParam, .. } if raw == name)
}

const CONFIG_IMPL: &str = "app::<impl Config<'a,T,N>>";
const CONFIG_RUNNER_IMPL: &str = "app::<impl app::Runner for Config<'a,T,N>>";
const CONFIG_TICK: &str = "app::Config<'a,T,N>::tick";
const CONFIG_CHAINED: &str = "app::Config<'a,T,N>::chained";

// ---------------------------------------------------------------- N rule

#[test]
fn type_spelling_is_normalized() {
    let n = normalize_type_spelling;
    assert_eq!(n("Vec < Option < store :: Engine > >"), "Vec<Option<store::Engine>>", "N1");
    assert_eq!(n("& 'a str"), "&'a str");
    assert_eq!(n("& mut [ u8 ]"), "&mut [u8]");
    assert_eq!(n("Box < dyn Fn ( u8 ) -> u8 >"), "Box<dyn Fn(u8) -> u8>");
    assert_eq!(n("( u8 , String )"), "(u8, String)");
    assert_eq!(n("[ u8 ; 4 ]"), "[u8; 4]");
    assert_eq!(n("* const u8"), "*const u8");
    assert_eq!(n("impl Runner + Send"), "impl Runner + Send");
    assert_eq!(n("HashMap < String , Vec < T > >"), "HashMap<String, Vec<T>>");
}

// ---------------------------------------------------------------- T rules

#[test]
fn type_records_cover_every_kind() {
    let k = types_ws();
    for (id, kind) in [
        ("app::Config", TypeKind::Struct),
        ("app::Pair", TypeKind::Struct),
        ("app::Mode", TypeKind::Enum),
        ("app::Raw", TypeKind::Union),
        ("app::Runner", TypeKind::Trait),
        ("app::EngineMap", TypeKind::TypeAlias),
        ("app::store::Engine", TypeKind::Struct),
        ("ext::Ext", TypeKind::Struct),
    ] {
        let t = ty(&k, id);
        assert_eq!(t.kind, kind, "{id}");
        assert!(k.item_graph().node(id).is_some(), "T8: ids match graph nodes: {id}");
    }
    let cfg = ty(&k, "app::Config");
    assert_eq!(cfg.package, "app");
    assert_eq!(cfg.module, "app");
    assert_eq!(cfg.name, "Config");
    assert!(cfg.is_public);
    assert_eq!(cfg.cfg, CfgPredicate::True);
    assert!(cfg.location.line.is_some());
}

#[test]
fn struct_fields_and_generics() {
    let k = types_ws();
    let cfg = ty(&k, "app::Config");

    assert_eq!(cfg.generics.len(), 3);
    assert_eq!(cfg.generics[0].name, "'a");
    assert_eq!(cfg.generics[0].kind, GenericKind::Lifetime);
    assert_eq!(cfg.generics[1].name, "T");
    assert_eq!(cfg.generics[1].kind, GenericKind::Type);
    assert_eq!(
        cfg.generics[1].bounds,
        vec![
            TypeRef { raw: "Runner".into(), mentions: vec![local("app::Runner")] },
            TypeRef { raw: "Default".into(), mentions: vec![external("std::Default")] },
        ]
    );
    assert_eq!(cfg.generics[2].name, "N");
    assert_eq!(cfg.generics[2].kind, GenericKind::Const);
    assert_eq!(cfg.generics[2].ty.as_ref().map(|t| t.raw.as_str()), Some("usize"));

    let names: Vec<Option<&str>> = cfg.fields.iter().map(|f| f.name.as_deref()).collect();
    assert_eq!(names, vec![Some("name"), Some("engine"), Some("map"), Some("extra")]);
    let name = field(cfg, "name");
    assert_eq!(name.ty.raw, "&'a str");
    assert!(name.ty.mentions.is_empty(), "primitives are not mentions");
    assert!(name.is_public);
    let engine = field(cfg, "engine");
    assert_eq!(engine.ty.mentions, vec![local("app::store::Engine")]);
    assert_eq!(engine.index, 1);
    let map = field(cfg, "map");
    assert!(!map.is_public);
    assert_eq!(map.ty.raw, "HashMap<String, Vec<T>>");
    assert_eq!(map.ty.mentions.len(), 4, "{:?}", map.ty.mentions);
    assert_eq!(map.ty.mentions[0], external("std::collections::HashMap"));
    assert_eq!(map.ty.mentions[1], external("std::String"));
    assert_eq!(map.ty.mentions[2], external("std::Vec"));
    assert!(is_generic_param(&map.ty.mentions[3], "T"), "{:?}", map.ty.mentions[3]);
    let extra = field(cfg, "extra");
    assert_eq!(extra.cfg, feat("x"), "field cfg");
    assert_eq!(extra.ty.mentions, vec![external("std::Option"), external("ext::Ext")]);
}

#[test]
fn tuple_fields_enum_variants_and_unions() {
    let k = types_ws();

    let pair = ty(&k, "app::Pair");
    assert_eq!(pair.fields.len(), 2);
    assert_eq!(pair.fields[0].name, None);
    assert_eq!(pair.fields[0].index, 0);
    assert_eq!(pair.fields[0].ty.raw, "u8");
    assert!(pair.fields[0].is_public);
    assert_eq!(pair.fields[1].ty.mentions, vec![local("app::store::Engine")]);
    assert!(!pair.fields[1].is_public);

    let mode = ty(&k, "app::Mode");
    let names: Vec<&str> = mode.variants.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, vec!["Off", "On", "Custom"], "T4");
    assert_eq!(mode.variants[0].discriminant.as_deref(), Some("0"));
    assert!(mode.variants[0].fields.is_empty());
    assert_eq!(mode.variants[1].fields[0].ty.raw, "u8");
    assert_eq!(mode.variants[1].fields[0].name, None);
    assert_eq!(mode.variants[2].fields[1].name.as_deref(), Some("engine"));
    assert_eq!(mode.variants[2].fields[1].ty.mentions, vec![local("app::store::Engine")]);

    let raw = ty(&k, "app::Raw");
    assert_eq!(raw.kind, TypeKind::Union);
    let names: Vec<Option<&str>> = raw.fields.iter().map(|f| f.name.as_deref()).collect();
    assert_eq!(names, vec![Some("bits"), Some("float")]);
}

#[test]
fn trait_records() {
    let k = types_ws();
    let r = ty(&k, "app::Runner");
    assert_eq!(
        r.supertraits,
        vec![
            TypeRef { raw: "std::fmt::Debug".into(), mentions: vec![external("std::fmt::Debug")] },
            TypeRef { raw: "Sized".into(), mentions: vec![external("std::Sized")] },
        ],
        "T5"
    );
    let sigs: Vec<(&str, &str, bool)> = r.trait_methods.iter().map(|m| (m.id.as_str(), m.name.as_str(), m.has_default)).collect();
    assert_eq!(sigs, vec![("app::Runner::run", "run", false), ("app::Runner::stop", "stop", true)]);
    assert_eq!(r.assoc_types, vec!["Output".to_string()]);
    assert_eq!(r.assoc_consts, vec!["LIMIT".to_string()]);
}

#[test]
fn aliases_and_derives() {
    let k = types_ws();
    let alias = ty(&k, "app::EngineMap");
    let target = alias.aliased.as_ref().expect("T7 alias target");
    assert_eq!(target.raw, "HashMap<String, store::Engine>");
    assert_eq!(target.mentions, vec![external("std::collections::HashMap"), external("std::String"), local("app::store::Engine")]);

    let cfg = ty(&k, "app::Config");
    assert_eq!(cfg.derives, vec![external("std::Debug"), external("std::Clone"), external("ext::Tag")], "T6");
    assert!(cfg.attrs.contains(&"derive".to_string()));
}

#[test]
fn impl_records_and_their_types() {
    let k = types_ws();

    let inherent = k.impls.iter().find(|i| i.id == CONFIG_IMPL).expect("inherent impl record");
    // the id keeps the resolver's no-space spelling; the TypeRef is the normalised spelling
    assert_eq!(inherent.self_ty.raw, "Config<'a, T, N>");
    assert_eq!(inherent.self_ty.mentions[0], local("app::Config"));
    assert!(inherent.trait_.is_none());
    assert_eq!(
        inherent.methods,
        vec![
            "app::Config<'a,T,N>::new".to_string(),
            "app::Config<'a,T,N>::engine".to_string(),
            CONFIG_TICK.to_string(),
            CONFIG_CHAINED.to_string(),
        ]
    );
    assert_eq!(inherent.generics.len(), 3);
    assert_eq!(inherent.generics[1].bounds[0].mentions, vec![local("app::Runner")]);

    let trait_impl = k.impls.iter().find(|i| i.id == CONFIG_RUNNER_IMPL).expect("trait impl record");
    assert_eq!(trait_impl.trait_.as_ref().unwrap().mentions, vec![local("app::Runner")]);
    assert_eq!(trait_impl.methods, vec!["app::<Config<'a,T,N> as app::Runner>::run".to_string()]);
    assert_eq!(trait_impl.assoc_types.len(), 1);
    assert_eq!(trait_impl.assoc_types[0].0, "Output");
    assert_eq!(trait_impl.assoc_types[0].1.raw, "u8");
    assert_eq!(trait_impl.assoc_consts, vec!["LIMIT".to_string()]);

    // Back-links, byte-order sorted (`<impl C` before `<impl a`)
    assert_eq!(ty(&k, "app::Config").impls, vec![CONFIG_IMPL.to_string(), CONFIG_RUNNER_IMPL.to_string()]);
    assert_eq!(ty(&k, "app::store::Engine").impls, vec!["app::store::<impl Engine>".to_string()]);
    assert_eq!(ty(&k, "app::Mode").impls, vec!["app::<impl Mode>".to_string()]);
    assert_eq!(k.impls_of("app::store::Engine").len(), 1);
    assert!(k.impls_of("app::Raw").is_empty());
    // every impl id is a graph node with the same cfg
    for i in &k.impls {
        let n = k.item_graph().node(&i.id).unwrap_or_else(|| panic!("impl {} is not a node", i.id));
        assert_eq!(n.cfg, i.cfg);
    }
}

// ---------------------------------------------------------------- M rules

#[test]
fn method_calls_resolve_via_self_and_fields() {
    let k = types_ws();
    let c = call(&k, CONFIG_TICK, "tick");
    assert_eq!(c.receiver_type, Some(local("app::store::Engine")), "M1 self → M4 field");
    assert_eq!(c.resolution, Some(local("app::store::Engine::tick")), "M2");
}

#[test]
fn method_calls_resolve_through_one_level_of_chaining() {
    let k = types_ws();
    let inner = call(&k, CONFIG_CHAINED, "engine");
    assert_eq!(inner.resolution, Some(local("app::Config<'a,T,N>::engine")));
    let outer = call(&k, CONFIG_CHAINED, "tick");
    assert_eq!(outer.receiver_type, Some(local("app::store::Engine")), "M3: engine() returns &store::Engine");
    assert_eq!(outer.resolution, Some(local("app::store::Engine::tick")));
}

#[test]
fn method_calls_resolve_via_params_lets_constructors_and_variants() {
    let k = types_ws();
    for f in ["typed_param", "let_annot", "let_ctor", "let_free_fn"] {
        let c = call(&k, f, "tick");
        assert_eq!(c.receiver_type, Some(local("app::store::Engine")), "M1 in {f}");
        assert_eq!(c.resolution, Some(local("app::store::Engine::tick")), "M2 in {f}");
    }
    let lit = call(&k, "let_literal", "tick");
    assert_eq!(lit.receiver_type, Some(local("app::store::Engine")), "M1 tuple-struct literal + M4 tuple field");
    let var = call(&k, "let_variant", "describe");
    assert_eq!(var.receiver_type, Some(local("app::Mode")), "M1 enum variant constructor");
    assert_eq!(var.resolution, Some(local("app::Mode::describe")));
}

#[test]
fn generic_receivers_dispatch_to_the_trait_declaration() {
    let k = types_ws();
    let run = call(&k, "generic_bound", "run");
    assert!(run.receiver_type.as_ref().is_some_and(|r| is_generic_param(r, "R")), "{:?}", run.receiver_type);
    assert_eq!(run.resolution, Some(local("app::Runner::run")), "M6");
    assert_eq!(call(&k, "generic_bound", "stop").resolution, Some(local("app::Runner::stop")));
}

#[test]
fn external_receivers_and_unknowns() {
    let k = types_ws();
    let ext = call(&k, "external_recv", "go");
    assert_eq!(ext.receiver_type, Some(local("ext::Ext")), "a scanned member is local");
    assert_eq!(ext.resolution, Some(local("ext::Ext::go")));

    let len = call(&k, "prelude_recv", "len");
    assert_eq!(len.receiver_type, Some(external("std::Vec")));
    assert_eq!(len.resolution, Some(external("std::Vec::len")), "M2 external: unverified");

    let unknown = call(&k, "really_unknown", "frob");
    assert_eq!(unknown.receiver_type, None, "M7");
    assert!(matches!(unknown.resolution, Some(Resolution::Unresolved { reason: UnresolvedReason::MethodCall, .. })));
}

// ---------------------------------------------------------------- G rule

#[test]
fn resolved_method_calls_become_graph_edges() {
    let k = types_ws();
    let g = k.item_graph();
    assert!(g.has_edge(CONFIG_TICK, "app::store::Engine::tick", EdgeKind::Reference), "G1: {:?}", g.edges_from(CONFIG_TICK));
    assert!(g.has_edge("app::typed_param", "app::store::Engine::tick", EdgeKind::Reference));
    assert!(g.has_edge("app::generic_bound", "app::Runner::run", EdgeKind::Reference));
    assert!(g.dependents_of("app::store::Engine::tick").contains("app::let_ctor"));
    // the unknown one is still listed, not guessed
    assert!(g.unresolved.iter().any(|u| u.from == "app::really_unknown" && u.raw == "frob"));
    assert!(!g.dependencies_of("app::really_unknown").iter().any(|d| d.ends_with("::frob")));
}
