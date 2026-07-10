//! The [`Finding`] record: span, code, intrinsic severity, message, and
//! optional help/related/data attachments.

use serde::{Deserialize, Serialize};
use surrealguard_syntax::span::SourceSpan;

use crate::FindingCode;

/// A finding's intrinsic class — how confidently the contract is
/// violated, never how a surface chooses to report it (that is policy).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Severity {
    Error,
    Warning,
    Hint,
}

/// One diagnostic, carrying its *intrinsic* severity class from the
/// catalog. Findings are policy-free: consumers (CLI, LSP, host adapters)
/// map classes to their presentation through [`crate::PolicyConfig`] —
/// promotion (`warnings_as_errors`), lint levels, and suppression happen
/// at that edge, never here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    span: SourceSpan,
    code: FindingCode,
    severity: Severity,
    message: String,
    help: Vec<Help>,
    related: Vec<RelatedInfo>,
    tags: Vec<FindingTag>,
    data: FindingData,
}

impl Finding {
    pub fn new(
        span: SourceSpan,
        code: FindingCode,
        severity: Severity,
        message: impl Into<String>,
    ) -> Self {
        Self {
            span,
            code,
            severity,
            message: message.into(),
            help: Vec::new(),
            related: Vec::new(),
            tags: Vec::new(),
            data: FindingData::None,
        }
    }

    pub fn with_data(mut self, data: FindingData) -> Self {
        self.data = data;
        self
    }

    pub fn span(&self) -> &SourceSpan {
        &self.span
    }

    pub fn code(&self) -> FindingCode {
        self.code
    }

    /// The intrinsic severity class from the catalog.
    pub fn severity(&self) -> Severity {
        self.severity
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn data(&self) -> &FindingData {
        &self.data
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Help {
    pub message: String,
    pub replacement: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelatedInfo {
    pub span: SourceSpan,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FindingTag {
    Unnecessary,
    Deprecated,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FindingData {
    None,
    UnknownTable {
        table: String,
    },
    UnknownField {
        table: Option<String>,
        field: String,
    },
    TypeMismatch {
        expected: String,
        found: String,
    },
    DynamicTableName {
        expression: String,
    },
    Lint {
        name: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealguard_syntax::source::SourceId;
    use surrealguard_syntax::span::{ByteRange, SourceSpan};

    #[test]
    fn finding_carries_its_intrinsic_class() {
        let span = SourceSpan::new(
            SourceId::new("query:001"),
            ByteRange::new(7, 12).expect("valid range"),
        );

        let finding = Finding::new(
            span.clone(),
            FindingCode::param(6001),
            Severity::Warning,
            "dynamic table name cannot be fully analyzed",
        )
        .with_data(FindingData::DynamicTableName {
            expression: "${table}".into(),
        });

        assert_eq!(finding.span(), &span);
        assert_eq!(finding.code().to_string(), "E6001");
        assert_eq!(finding.severity(), Severity::Warning);
        assert_eq!(
            finding.message(),
            "dynamic table name cannot be fully analyzed"
        );
    }
}
