use serde::{Deserialize, Serialize};
use surrealguard_syntax::span::SourceSpan;

use crate::FindingCode;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Severity {
    Error,
    Warning,
    Hint,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    span: SourceSpan,
    code: FindingCode,
    default_severity: Severity,
    effective_severity: Severity,
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
        default_severity: Severity,
        message: impl Into<String>,
    ) -> Self {
        Self {
            span,
            code,
            default_severity,
            effective_severity: default_severity,
            message: message.into(),
            help: Vec::new(),
            related: Vec::new(),
            tags: Vec::new(),
            data: FindingData::None,
        }
    }

    pub fn with_effective_severity(mut self, severity: Severity) -> Self {
        self.effective_severity = severity;
        self
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

    pub fn default_severity(&self) -> Severity {
        self.default_severity
    }

    pub fn effective_severity(&self) -> Severity {
        self.effective_severity
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
    fn finding_keeps_default_and_effective_severity_separate() {
        let span = SourceSpan::new(
            SourceId::new("query:001"),
            ByteRange::new(7, 12).expect("valid range"),
        );

        let finding = Finding::new(
            span.clone(),
            FindingCode::dynamic(6001),
            Severity::Warning,
            "dynamic table name cannot be fully analyzed",
        )
        .with_effective_severity(Severity::Error)
        .with_data(FindingData::DynamicTableName {
            expression: "${table}".into(),
        });

        assert_eq!(finding.span(), &span);
        assert_eq!(finding.code().to_string(), "W6001");
        assert_eq!(finding.default_severity(), Severity::Warning);
        assert_eq!(finding.effective_severity(), Severity::Error);
        assert_eq!(
            finding.message(),
            "dynamic table name cannot be fully analyzed"
        );
    }
}
