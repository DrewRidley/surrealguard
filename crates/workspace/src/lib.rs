pub mod analysis;
pub mod config;
pub mod schema;
pub mod semantic;
pub mod source_registry;

pub use analysis::{
    analyze_query, analyze_source, analyze_workspace, AnalysisOutput, ParamInference,
    StatementAnalysis, Workspace, WorkspaceAnalysis,
};
pub use schema::{FieldDef, SchemaIndex, TableDef};
