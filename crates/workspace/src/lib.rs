pub mod analysis;
pub mod config;
pub mod response_shape;
pub mod schema;
pub mod select_ir;
pub mod semantic;
pub mod source_registry;

pub use analysis::{
    analyze_query, analyze_source, analyze_workspace, AnalysisOutput, ParamInference,
    SelectModifierAnalysis, StatementAnalysis, Workspace, WorkspaceAnalysis,
};
pub use response_shape::{FieldShape, PartialReason, ResponseShape};
pub use schema::{FieldDef, SchemaIndex, TableDef};
