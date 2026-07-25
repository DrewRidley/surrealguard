//! Workspace analysis: schema extraction, type inference over the lowered
//! AST (`analyzer::*`), and the findings pipeline consumed by the CLI and
//! LSP.

pub mod analysis;
pub mod analyzer;
pub mod config;
pub mod context_params;
pub mod expression;
pub mod kinds;
pub mod query;
pub mod schema;
pub mod source_registry;
pub mod statement_env;
mod suggest;
mod suppress;

pub use analysis::{
    analyze_one_source, analyze_query, analyze_source, analyze_workspace, build_global_catalog,
    AnalysisOutput, GlobalCatalog, LetBindingAnalysis, ParamInference, SelectModifierAnalysis,
    StatementAnalysis, Workspace, WorkspaceAnalysis,
};
pub use expression::{ExpressionFact, ExpressionValueClass, PartialReason};
pub use query::{
    definition_at, function_return_hints, hover_at, let_binding_hints, render_kind,
    DefinitionTarget, HoverInfo, TypeHint,
};
pub use schema::{
    AnalyzerDef, FieldDef, FieldPath, FunctionDef, ParamDef, RelationDef, SchemaIndex, TableDef,
};
pub use statement_env::StatementEnv;
