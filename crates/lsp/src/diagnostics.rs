//! Convert surrealguard diagnostics to LSP diagnostics.
//!
//! Handles span-to-range conversion and related information formatting.

use tower_lsp::lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, NumberOrString, Url,
};

use surrealguard_analyzer::{self as sg};

use crate::text::byte_range_to_lsp;

/// Convert a surrealguard diagnostic to an LSP diagnostic.
pub fn to_lsp_diagnostic(source: &str, uri: &Url, diag: &sg::Diagnostic) -> Diagnostic {
    let range = byte_range_to_lsp(source, diag.span.start as usize, diag.span.end as usize);

    let severity = match diag.severity {
        sg::Severity::Error => DiagnosticSeverity::ERROR,
        sg::Severity::Warning => DiagnosticSeverity::WARNING,
        sg::Severity::Hint => DiagnosticSeverity::HINT,
    };

    // Build message: main message + optional suggestion
    let mut message = diag.message.clone();
    if let Some(suggestion) = &diag.suggestion {
        message.push_str(&format!("\nhelp: {suggestion}"));
    }

    // Convert related information (only valid same-file spans)
    let related_information = if diag.related.is_empty() {
        None
    } else {
        let valid: Vec<_> = diag
            .related
            .iter()
            .filter_map(|rel| {
                let rel_range = byte_range_to_lsp(
                    source,
                    rel.span.start as usize,
                    rel.span.end as usize,
                );
                Some(DiagnosticRelatedInformation {
                    location: Location::new(uri.clone(), rel_range),
                    message: rel.message.clone(),
                })
            })
            .collect();

        if valid.is_empty() { None } else { Some(valid) }
    };

    Diagnostic {
        range,
        severity: Some(severity),
        code: Some(NumberOrString::String(diag.code.id().to_string())),
        source: Some("surrealguard".to_string()),
        message,
        related_information,
        ..Diagnostic::default()
    }
}
