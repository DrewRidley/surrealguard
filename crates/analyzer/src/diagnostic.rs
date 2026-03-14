use crate::span::Span;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Hint,
    Warning,
    Error,
}

/// Structured diagnostic codes.
///
/// Each code has a category prefix:
/// - SG0xx: Reference errors (table/field/param/function not found)
/// - SG1xx: Type errors (mismatch, cast, operator, args)
/// - SG2xx: Schema errors (readonly, missing field, schemafull violation)
/// - SG3xx: Scope errors (undefined var, shadow)
/// - SG4xx: Permission errors
/// - SG5xx: Graph errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    // Parse
    ParseError,
    // Reference errors
    TableNotFound,
    FieldNotFound,
    ParameterNotFound,
    FunctionNotFound,
    IndexNotFound,
    // Type errors
    TypeMismatch,
    InvalidCast,
    InvalidOperator,
    WrongArgCount,
    WrongArgType,
    AmbiguousType,
    ReturnTypeMismatch,
    // Schema errors
    ReadonlyAssignment,
    MissingRequiredField,
    UndefinedFieldOnSchemafull,
    DuplicateFieldDef,
    // Scope errors
    UndefinedVariable,
    VariableShadow,
    // Permission errors
    PermissionFieldInvalid,
    // Graph errors
    InvalidRelationTraversal,
    NotARelation,
    InvalidGraphEdge,
    // Assignment errors
    IncompatibleAssignment,
    ComputedFieldAssignment,
    InvalidAssignmentOperator,
    // Control flow errors
    BreakOutsideLoop,
    ContinueOutsideLoop,
    // Permission errors (additional)
    PermissionConditionNotBool,
    // Nested field access errors
    InvalidNestedAccess,
    // Schema validation
    DuplicateTableDef,
    FieldOnUndefinedTable,
    IndexOnUndefinedField,
    RecordRefUndefinedTable,
    AssertNotBool,
    DuplicateFieldAssignment,
    DefaultTypeMismatch,
    ValueTypeMismatch,
    // INSERT VALUES errors
    ValuesColumnCountMismatch,
    // Object literal errors
    DuplicateObjectKey,
    // Content clause errors
    ContentNotObject,
    // Return clause errors
    ReturnBeforeOnCreate,
    // Unreachable code
    UnreachableCode,
    // Transaction balance errors
    UnbalancedTransaction,
    // SELECT clause warnings
    GroupByWithoutAggregate,
    // Nested field without parent definition
    NestedFieldWithoutParent,
    // Path expression warnings
    NonIntegerArrayIndex,
    UnnecessaryOptionalChaining,
    UnknownCastType,
    // SELECT VALUE warnings
    SelectValueMultipleFields,
    // Permission conflict between table and field levels
    ConflictingPermissions,
    // PATCH clause validation
    InvalidPatchOperation,
    // REFERENCE clause on non-record field
    ReferenceOnNonRecord,
}

impl Code {
    pub fn id(&self) -> &'static str {
        match self {
            Code::ParseError => "SG000",
            Code::TableNotFound => "SG001",
            Code::FieldNotFound => "SG002",
            Code::ParameterNotFound => "SG003",
            Code::FunctionNotFound => "SG004",
            Code::IndexNotFound => "SG005",
            Code::TypeMismatch => "SG100",
            Code::InvalidCast => "SG101",
            Code::InvalidOperator => "SG102",
            Code::WrongArgCount => "SG103",
            Code::WrongArgType => "SG104",
            Code::AmbiguousType => "SG105",
            Code::ReturnTypeMismatch => "SG106",
            Code::ReadonlyAssignment => "SG200",
            Code::MissingRequiredField => "SG201",
            Code::UndefinedFieldOnSchemafull => "SG202",
            Code::DuplicateFieldDef => "SG203",
            Code::UndefinedVariable => "SG300",
            Code::VariableShadow => "SG301",
            Code::PermissionFieldInvalid => "SG400",
            Code::InvalidRelationTraversal => "SG500",
            Code::NotARelation => "SG501",
            Code::InvalidGraphEdge => "SG502",
            Code::IncompatibleAssignment => "SG204",
            Code::ComputedFieldAssignment => "SG205",
            Code::InvalidAssignmentOperator => "SG206",
            Code::BreakOutsideLoop => "SG302",
            Code::ContinueOutsideLoop => "SG303",
            Code::PermissionConditionNotBool => "SG401",
            Code::InvalidNestedAccess => "SG212",
            Code::DuplicateTableDef => "SG207",
            Code::FieldOnUndefinedTable => "SG208",
            Code::IndexOnUndefinedField => "SG209",
            Code::RecordRefUndefinedTable => "SG210",
            Code::AssertNotBool => "SG211",
            Code::DuplicateFieldAssignment => "SG213",
            Code::ValuesColumnCountMismatch => "SG214",
            Code::DefaultTypeMismatch => "SG215",
            Code::ValueTypeMismatch => "SG216",
            Code::DuplicateObjectKey => "SG217",
            Code::ContentNotObject => "SG218",
            Code::ReturnBeforeOnCreate => "SG220",
            Code::UnreachableCode => "SG219",
            Code::UnbalancedTransaction => "SG221",
            Code::GroupByWithoutAggregate => "SG222",
            Code::NestedFieldWithoutParent => "SG223",
            Code::NonIntegerArrayIndex => "SG224",
            Code::UnnecessaryOptionalChaining => "SG225",
            Code::UnknownCastType => "SG226",
            Code::SelectValueMultipleFields => "SG227",
            Code::ConflictingPermissions => "SG228",
            Code::InvalidPatchOperation => "SG229",
            Code::ReferenceOnNonRecord => "SG230",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub span: Span,
    pub severity: Severity,
    pub code: Code,
    pub message: String,
    pub suggestion: Option<String>,
    pub related: Vec<RelatedInfo>,
}

#[derive(Debug, Clone)]
pub struct RelatedInfo {
    pub span: Span,
    pub message: String,
}

impl Diagnostic {
    pub fn error(span: Span, code: Code, message: impl Into<String>) -> Self {
        Self {
            span,
            severity: Severity::Error,
            code,
            message: message.into(),
            suggestion: None,
            related: Vec::new(),
        }
    }

    pub fn warning(span: Span, code: Code, message: impl Into<String>) -> Self {
        Self {
            span,
            severity: Severity::Warning,
            code,
            message: message.into(),
            suggestion: None,
            related: Vec::new(),
        }
    }

    pub fn hint(span: Span, code: Code, message: impl Into<String>) -> Self {
        Self {
            span,
            severity: Severity::Hint,
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

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sev = match self.severity {
            Severity::Hint => "hint",
            Severity::Warning => "warning",
            Severity::Error => "error",
        };
        write!(
            f,
            "{}[{}]: {} (bytes {}..{})",
            sev,
            self.code.id(),
            self.message,
            self.span.start,
            self.span.end
        )?;
        if let Some(sug) = &self.suggestion {
            write!(f, "\n  help: {}", sug)?;
        }
        for rel in &self.related {
            write!(
                f,
                "\n  note: {} (bytes {}..{})",
                rel.message, rel.span.start, rel.span.end
            )?;
        }
        Ok(())
    }
}
