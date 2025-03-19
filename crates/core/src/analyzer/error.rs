use surrealdb::err::Error as SurrealError;
use surrealdb::sql::Kind;
use thiserror::Error;

/// Result type for analyzer operations
pub type AnalyzerResult<T> = Result<T, AnalyzerError>;

/// Errors that can occur during static analysis of SurrealQL queries
#[derive(Debug, Error)]
pub enum AnalyzerError {
    /// Wraps underlying SurrealDB errors, primarily parse errors
    #[error("SurrealQL error: {0}")]
    Surreal(#[from] SurrealError),

    /// A referenced field does not exist in the table or record schema
    #[error("Field '{field}' not found in {context}")]
    FieldNotFound { field: String, context: String },

    /// A referenced table does not exist in the database schema
    #[error("Table '{0}' not found")]
    TableNotFound(String),

    /// A referenced parameter is not defined in the current context
    #[error("Parameter '${0}' not found")]
    ParameterNotFound(String),

    /// A referenced function is not defined or imported
    #[error("Function '{0}' not found")]
    FunctionNotFound(String),

    /// A type mismatch occurred during analysis
    #[error("Type mismatch: expected {expected}, found {found}")]
    TypeMismatch { expected: String, found: String },

    /// A schema constraint or rule was violated
    #[error("Schema violation: {message}")]
    SchemaViolation {
        message: String,
        table: Option<String>,
        field: Option<String>,
    },

    /// An invalid path or field access expression was encountered
    #[error("Invalid path: {path}")]
    InvalidPath {
        path: String,
        context: Option<String>,
    },

    /// Function call analysis failed
    #[error("Invalid function call: {message}")]
    InvalidFunctionCall { function: String, message: String },

    /// A permissions check failed during analysis
    #[error("Permission denied: {message}")]
    PermissionDenied { message: String, resource: String },

    #[error("Unexpected syntax was encountered")]
    UnexpectedSyntax,

    #[error("No scope provided for $auth inference.")]
    MissingAuth,

    #[error("Not implemented: {0}")]
    Unimplemented(String),
}

impl AnalyzerError {
    /// Creates a type mismatch error from actual and expected types
    pub fn type_mismatch(expected: &Kind, found: &Kind) -> Self {
        Self::TypeMismatch {
            expected: expected.to_string(),
            found: found.to_string(),
        }
    }

    pub fn report(&self) -> String {
        match self {
            Self::SchemaViolation {
                message,
                table,
                field,
            } => {
                format!(
                        "Schema violation in table '{}', field '{}': {}\n\
                         Suggestion: Ensure the field matches the schema definition or update the schema.",
                        table.as_deref().unwrap_or("unknown"),
                        field.as_deref().unwrap_or("unknown"),
                        message
                    )
            }
            Self::TypeMismatch { expected, found } => {
                format!(
                    "Type mismatch: expected {}, found {}\n\
                         Suggestion: Convert the value to the expected type or update the schema.",
                    expected, found
                )
            }
            Self::FieldNotFound { field, context } => {
                format!(
                    "Field '{}' not found in {}\n\
                         Suggestion: Check field name spelling or add the field to the schema.",
                    field, context
                )
            }
            Self::TableNotFound(table) => {
                format!(
                    "Table '{}' not found in database schema\n\
                         Suggestion: Define the table using DEFINE TABLE or check the table name.",
                    table
                )
            }
            Self::ParameterNotFound(param) => {
                format!(
                    "Parameter '{}' not found in current context\n\
                         Suggestion: Ensure all parameters are properly defined before use.",
                    param
                )
            }
            Self::FunctionNotFound(func) => {
                format!(
                    "Function '{}' not found\n\
                         Suggestion: Import the function or check function name spelling.",
                    func
                )
            }
            Self::InvalidPath { path, context } => {
                format!(
                    "Invalid path: {}{}\n\
                         Suggestion: Verify the path syntax and ensure all segments exist.",
                    path,
                    context
                        .as_ref()
                        .map(|ctx| format!(" in {}", ctx))
                        .unwrap_or_default()
                )
            }
            Self::InvalidFunctionCall { function, message } => {
                format!(
                    "Invalid function call to '{}': {}\n\
                         Suggestion: Check function documentation for correct usage.",
                    function, message
                )
            }
            Self::PermissionDenied { message, resource } => {
                format!(
                        "Permission denied: {} for resource '{}'\n\
                         Suggestion: Verify access permissions or authenticate with required privileges.",
                        message, resource
                    )
            }
            Self::UnexpectedSyntax => "Unexpected syntax encountered\n\
                     Suggestion: Verify query syntax against SurrealQL documentation."
                .to_string(),
            Self::Unimplemented(feature) => {
                format!(
                    "Feature not implemented: {}\n\
                         This is a limitation of the current version.",
                    feature
                )
            }
            Self::Surreal(err) => {
                format!(
                        "SurrealDB error: {}\n\
                         Suggestion: This is an underlying database error, check SurrealDB documentation.",
                        err
                    )
            }
            Self::MissingAuth => {
                format!(
                    "Missing provided auth scope but $auth is used\n\
                         Suggestion: A scope must be provided when selecting from $auth!"
                )
            }
        }
    }

    /// Creates a field not found error with context
    pub fn field_not_found(field: impl Into<String>, context: impl Into<String>) -> Self {
        Self::FieldNotFound {
            field: field.into(),
            context: context.into(),
        }
    }

    /// Creates a schema violation error
    pub fn schema_violation(
        message: impl Into<String>,
        table: Option<impl Into<String>>,
        field: Option<impl Into<String>>,
    ) -> Self {
        Self::SchemaViolation {
            message: message.into(),
            table: table.map(Into::into),
            field: field.map(Into::into),
        }
    }

    /// Returns true if this error represents a schema violation
    pub fn is_schema_violation(&self) -> bool {
        matches!(self, Self::SchemaViolation { .. })
    }

    /// Returns true if this error represents a type error
    pub fn is_type_error(&self) -> bool {
        matches!(self, Self::TypeMismatch { .. })
    }

    /// Returns true if this error represents a reference error (missing table, field, etc)
    pub fn is_reference_error(&self) -> bool {
        matches!(
            self,
            Self::FieldNotFound { .. }
                | Self::TableNotFound(_)
                | Self::ParameterNotFound(_)
                | Self::FunctionNotFound(_)
        )
    }
}
