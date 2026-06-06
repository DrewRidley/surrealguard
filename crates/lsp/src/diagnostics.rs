//! Convert surrealguard diagnostics to LSP diagnostics.
//!
//! Handles span-to-range conversion and related information formatting.

use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString};

use surrealguard_diagnostics::{Finding, Severity as WorkspaceSeverity};

use crate::text::byte_range_to_lsp;

/// Convert a workspace-analysis finding to an LSP diagnostic.
pub fn workspace_finding_to_lsp_diagnostic(source: &str, finding: &Finding) -> Diagnostic {
    let range = finding.span().range();
    let range = byte_range_to_lsp(source, range.start() as usize, range.end() as usize);

    let severity = match finding.effective_severity() {
        WorkspaceSeverity::Error => DiagnosticSeverity::ERROR,
        WorkspaceSeverity::Warning => DiagnosticSeverity::WARNING,
        WorkspaceSeverity::Hint => DiagnosticSeverity::HINT,
    };

    Diagnostic {
        range,
        severity: Some(severity),
        code: Some(NumberOrString::String(finding.code().to_string())),
        source: Some("surrealguard".to_string()),
        message: finding.message().to_string(),
        related_information: None,
        ..Diagnostic::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealguard_diagnostics::{Finding, FindingCode, Severity};
    use surrealguard_syntax::source::SourceId;
    use surrealguard_syntax::span::{ByteRange, SourceSpan};

    #[test]
    fn workspace_finding_converts_to_lsp_diagnostic_contract() {
        let source = "SELECT * FROM ;";
        let finding = Finding::new(
            SourceSpan::new(
                SourceId::new("file:///workspace/query.surql"),
                ByteRange::new(14, 14).expect("valid range"),
            ),
            FindingCode::syntax(1),
            Severity::Error,
            "unexpected syntax",
        );

        let diagnostic = workspace_finding_to_lsp_diagnostic(source, &finding);

        assert_eq!(
            diagnostic.code,
            Some(NumberOrString::String("S0001".into()))
        );
        assert_eq!(diagnostic.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(diagnostic.source, Some("surrealguard".into()));
        assert_eq!(diagnostic.message, "unexpected syntax");
        assert_eq!(diagnostic.range.start.line, 0);
        assert_eq!(diagnostic.range.start.character, 14);
        assert_eq!(diagnostic.range.end, diagnostic.range.start);
    }
}
