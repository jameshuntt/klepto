#![doc = include_str!("../README.md")]

pub mod model;
pub mod syntax;
pub mod resolve;
pub mod graph;
pub mod typing;
pub mod gate;
pub mod receipt;
pub mod patterns;
pub mod rules;
mod klepto;

pub use crate::resolve::expand::{Expansion, MacroTable, Outcome};
pub use crate::patterns::{
    Candidate, CandidateKind, Cluster, FnMetrics, FnShape, MiningOptions, Param, ParamKind, PatternReport, RepeatedImpl, ShapeLevel,
    SharedMethodSet, Slot, SHAPE_LEVELS,
};
pub use crate::syntax::imports_ext::{ImportSummary, ImportVecExt};
pub use crate::receipt::{DepUseRecord, FileRecord, PackageRecord, Receipt, ReceiptDiff, SkippedFile, RECEIPT_VERSION};
pub use crate::receipt::kg::KgBundle;
pub use crate::receipt::pipeline::{run_pipeline, Level, Policy, PolicyError, Verdict, Violation};
pub use crate::typing::types::{
    normalize_type_spelling, FieldDecl, GenericKind, GenericParam, ImplDecl, MethodSig, TypeDecl, TypeKind, TypeRef, VariantDecl,
};

pub use crate::resolve::cfg::{CfgEnv, CfgPredicate, Implication};
pub use crate::resolve::features::{DepInfo, DepKind, EnableBy, FeatureEdge, FeatureEnable, FeatureInfo, FeatureKey, FeatureModel, UngatedUse};
pub use crate::graph::{Affected, CfgEdgeViolation, EdgeKind, FeatureChain, FeatureItemMap, FeatureItems, ItemEdge, ItemGraph, ItemNode, NodeKind, UnresolvedReference};
pub use crate::gate::oracle::{
    check_matrix, parse_cargo_json, CheckJob, ConvergeStatus, Convergence, Diagnostic, FnOracle, MissedGate, Oracle, OracleResult, Round,
};
pub use crate::gate::plan::{ForcedGate, GateEdit, GateKind, GatePlan, GateReason, UnknownGate};
pub use crate::gate::spec::{manifest_edits, FeatureSpec, ManifestEdit, PackageSpec, SpecError};
pub use crate::syntax::index::{EnclosingIndex, FnSpan};
pub use crate::klepto::{Klepto, KleptoBuilder, KleptoError, ParsedFile};
pub use crate::model::*;
pub use crate::syntax::query::*;
pub use crate::syntax::find::*;
pub use crate::syntax::snapshot::*;
pub use crate::syntax::report::*;
pub use crate::rules::*;
