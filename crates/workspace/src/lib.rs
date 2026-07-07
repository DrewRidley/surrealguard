//! Workspace analysis: schema extraction, type inference over the lowered
//! AST (`analyzer::*`), and the findings pipeline consumed by the CLI and
//! LSP.

pub mod analysis;
pub mod analyzer;
pub mod config;
pub mod expression;
pub mod schema;
pub mod select_ir;
pub mod semantic;
pub mod source_registry;
pub mod statement_env;

pub use analysis::{
    analyze_query, analyze_source, analyze_workspace, AnalysisOutput, ParamInference,
    SelectModifierAnalysis, StatementAnalysis, Workspace, WorkspaceAnalysis,
};
pub use expression::{ExpressionFact, ExpressionValueClass, PartialReason};
pub use schema::{
    AnalyzerDef, FieldDef, FieldPath, FunctionDef, ParamDef, RelationDef, SchemaIndex, TableDef,
};
pub use statement_env::StatementEnv;
