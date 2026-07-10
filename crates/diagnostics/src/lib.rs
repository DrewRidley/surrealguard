//! Finding types and policy for SurrealGuard diagnostics.
//!
//! A [`Finding`] carries its intrinsic severity class only; consumers
//! (CLI, LSP, host adapters) resolve the effective severity through
//! [`PolicyConfig`] at their edge. Codes are allocated in the catalog
//! ([`catalog`]), one code per contract.

pub mod catalog;
pub mod code;
pub mod finding;
pub mod policy;
pub mod suppression;

pub use code::{FindingCategory, FindingCode};
pub use finding::{Finding, FindingData, FindingTag, Help, RelatedInfo, Severity};
pub use policy::{LintLevel, PolicyConfig};
pub use suppression::{
    parse_optional_suppression_directive, parse_suppression_directive, Suppression,
    SuppressionParseError, SuppressionTarget,
};

/// Renders a code for display: the severity's letter plus the number
/// (`E1004`, `W4003`, `I7002`) — syntax keeps its `S` prefix. Severity is
/// the *resolved* one, so policy promotion shows as `E`.
pub fn render_code(code: FindingCode, severity: Severity) -> String {
    if code.category() == FindingCategory::Syntax {
        return format!("S{:04}", code.number());
    }
    let letter = match severity {
        Severity::Error => 'E',
        Severity::Warning => 'W',
        Severity::Hint => 'I',
    };
    format!("{letter}{:04}", code.number())
}
