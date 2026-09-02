#![doc = include_str!("../README.md")]

pub mod model;
pub mod extract;
mod klepto;
pub mod query;
pub mod find;
pub mod snapshot;
pub mod report;
pub mod rules;
pub mod index;
pub mod imports_ext;
pub mod use_sites;
pub mod resolve;
pub mod cfg;
pub mod features;
pub mod graph;
pub mod spec;
pub mod plan;
pub mod oracle;
pub mod types;
pub mod methods;
pub mod receipt;
pub mod kg;
pub mod pipeline;
pub mod patterns;
pub mod expand;
pub use crate::expand::{Expansion, MacroTable, Outcome};
pub use crate::patterns::{
    Candidate, CandidateKind, Cluster, FnMetrics, FnShape, MiningOptions, Param, ParamKind, PatternReport, RepeatedImpl, ShapeLevel,
    SharedMethodSet, Slot, SHAPE_LEVELS,
};
pub use crate::imports_ext::{ImportSummary, ImportVecExt};
pub use crate::receipt::{DepUseRecord, FileRecord, PackageRecord, Receipt, ReceiptDiff, SkippedFile, RECEIPT_VERSION};
pub use crate::kg::KgBundle;
pub use crate::pipeline::{run_pipeline, Level, Policy, PolicyError, Verdict, Violation};
pub use crate::types::{
    normalize_type_spelling, FieldDecl, GenericKind, GenericParam, ImplDecl, MethodSig, TypeDecl, TypeKind, TypeRef, VariantDecl,
};

pub use crate::cfg::{CfgEnv, CfgPredicate, Implication};
pub use crate::features::{DepInfo, DepKind, FeatureEnable, FeatureInfo, FeatureModel, UngatedUse};
pub use crate::graph::{Affected, CfgEdgeViolation, EdgeKind, ItemEdge, ItemGraph, ItemNode, NodeKind, UnresolvedReference};
pub use crate::oracle::{
    check_matrix, parse_cargo_json, CheckJob, ConvergeStatus, Convergence, Diagnostic, FnOracle, MissedGate, Oracle, OracleResult, Round,
};
pub use crate::plan::{ForcedGate, GateEdit, GateKind, GatePlan, GateReason, UnknownGate};
pub use crate::spec::{manifest_edits, FeatureSpec, ManifestEdit, PackageSpec, SpecError};
pub use crate::index::{EnclosingIndex, FnSpan};
pub use crate::klepto::{Klepto, KleptoBuilder, KleptoError, ParsedFile};
pub use crate::model::*;
pub use crate::query::*;
pub use crate::find::*;
pub use crate::snapshot::*;
pub use crate::report::*;
pub use crate::rules::*;
