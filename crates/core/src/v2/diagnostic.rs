/// Diagnostics with precise source spans.
///
/// Every diagnostic carries a [`Span`] so that editors and the CLI
/// can point to the exact location in the source text.
use super::span::Span;
use std::fmt;

/// Severity level for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Informational hint (e.g., "consider using X instead").
    Hint,
    /// Warning — valid but likely unintended.
    Warning,
    /// Error — invalid query or schema violation.
    Error,
}

/// A diagnostic message with source location.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// The span in the source text.
    pub span: Span,
    /// Severity level.
    pub severity: Severity,
    /// The error code (e.g., "E001").
    pub code: DiagnosticCode,
    /// Human-readable message.
    pub message: String,
    /// Optional suggestion for how to fix the issue.
    pub suggestion: Option<String>,
    /// Related locations (e.g., "field defined here").
    pub related: Vec<RelatedInfo>,
}

/// Additional location context for a diagnostic.
#[derive(Debug, Clone)]
pub struct RelatedInfo {
    pub span: Span,
    pub message: String,
}

/// Structured error codes for programmatic consumption.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticCode {
    /// Parse error in SurrealQL.
    ParseError,
    /// Referenced table does not exist.
    TableNotFound,
    /// Referenced field does not exist on the table.
    FieldNotFound,
    /// Type mismatch in expression or assignment.
    TypeMismatch,
    /// Schema constraint violated.
    SchemaViolation,
    /// Referenced parameter not defined.
    ParameterNotFound,
    /// Referenced function not defined.
    FunctionNotFound,
    /// Invalid path or field access.
    InvalidPath,
    /// Permission check failed.
    PermissionDenied,
    /// Feature not yet implemented in the analyzer.
    Unimplemented,
}

impl Diagnostic {
    pub fn error(span: Span, code: DiagnosticCode, message: impl Into<String>) -> Self {
        Self {
            span,
            severity: Severity::Error,
            code,
            message: message.into(),
            suggestion: None,
            related: Vec::new(),
        }
    }

    pub fn warning(span: Span, code: DiagnosticCode, message: impl Into<String>) -> Self {
        Self {
            span,
            severity: Severity::Warning,
            code,
            message: message.into(),
            suggestion: None,
            related: Vec::new(),
        }
    }

    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    pub fn with_related(mut self, span: Span, message: impl Into<String>) -> Self {
        self.related.push(RelatedInfo {
            span,
            message: message.into(),
        });
        self
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let severity = match self.severity {
            Severity::Hint => "hint",
            Severity::Warning => "warning",
            Severity::Error => "error",
        };
        write!(
            f,
            "{severity}[{:?}]: {} (at bytes {}..{})",
            self.code, self.message, self.span.start, self.span.end
        )?;
        if let Some(suggestion) = &self.suggestion {
            write!(f, "\n  suggestion: {}", suggestion)?;
        }
        Ok(())
    }
}
