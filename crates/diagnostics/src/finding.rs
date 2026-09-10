//! The [`Finding`] record: span, code, intrinsic severity, message, and
//! optional help/related/tag attachments.

use serde::{Deserialize, Serialize};
use surrealguard_syntax::span::SourceSpan;

use crate::FindingCode;

/// A finding's intrinsic class — how confidently the contract is
/// violated, never how a surface chooses to report it (that is policy).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Severity {
    /// The contract is definitely violated — the query cannot be trusted.
    Error,
    /// The contract is probably violated, but analysis cannot be certain.
    Warning,
    /// A stylistic or advisory note; the contract holds.
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
}

impl Finding {
    /// Builds a bare finding at `span` for `code`, carrying its intrinsic
    /// `severity`. Attachments (help/related/tags) are added with the
    /// builder methods below. Prefer [`crate::catalog::finding`], which
    /// fixes the severity from the catalog rather than trusting the caller.
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
        }
    }

    /// Attach an actionable suggestion, rendered as `help:` by the CLI
    /// and appended to the message by the LSP.
    pub fn with_help(mut self, message: impl Into<String>) -> Self {
        self.help.push(Help {
            message: message.into(),
            replacement: None,
        });
        self
    }

    /// Attach a secondary location that explains the finding (e.g. where
    /// the violated declaration lives).
    pub fn with_related(mut self, span: SourceSpan, message: impl Into<String>) -> Self {
        self.related.push(RelatedInfo {
            span,
            message: message.into(),
        });
        self
    }

    /// Mark the finding with a rendering hint (unnecessary, deprecated)
    /// that surfaces can act on — e.g. fade or strike-through in an editor.
    pub fn with_tag(mut self, tag: FindingTag) -> Self {
        self.tags.push(tag);
        self
    }

    /// Rebuilds the finding with every span — the primary one and each
    /// `related` location — passed through `map`, keeping the code, severity,
    /// message, help, and tags untouched.
    ///
    /// This is how a finding raised on an *embedded* query (a template
    /// literal in a `.ts` file, a `query!` string in Rust) is re-addressed to
    /// its host file: every surface (CLI, LSP, WASM) used to hand-roll this
    /// and each dropped a different attachment. One implementation means all
    /// three explain the same facts at the same places.
    pub fn map_spans(&self, mut map: impl FnMut(&SourceSpan) -> SourceSpan) -> Self {
        Self {
            span: map(&self.span),
            code: self.code,
            severity: self.severity,
            message: self.message.clone(),
            help: self.help.clone(),
            related: self
                .related
                .iter()
                .map(|related| RelatedInfo {
                    span: map(&related.span),
                    message: related.message.clone(),
                })
                .collect(),
            tags: self.tags.clone(),
        }
    }

    /// The primary source location the finding points at.
    pub fn span(&self) -> &SourceSpan {
        &self.span
    }

    /// The catalog code identifying which contract was violated.
    pub fn code(&self) -> FindingCode {
        self.code
    }

    /// The intrinsic severity class from the catalog.
    pub fn severity(&self) -> Severity {
        self.severity
    }

    /// The human-readable description composed at the emission site.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Actionable suggestions attached to the finding.
    pub fn help(&self) -> &[Help] {
        &self.help
    }

    /// Secondary locations that explain the finding.
    pub fn related(&self) -> &[RelatedInfo] {
        &self.related
    }

    /// Rendering hints attached to the finding.
    pub fn tags(&self) -> &[FindingTag] {
        &self.tags
    }
}

/// An actionable suggestion, optionally carrying a replacement string for
/// an automated fix.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Help {
    /// The suggestion text shown to the user.
    pub message: String,
    /// Replacement text for the finding's span, when a fix can be applied
    /// mechanically.
    pub replacement: Option<String>,
}

/// A secondary location that gives context for a finding — for instance,
/// where the violated declaration lives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelatedInfo {
    /// The source location this note points at.
    pub span: SourceSpan,
    /// What the location contributes to the finding.
    pub message: String,
}

/// A rendering hint about the flagged code, mapping to editor tags (LSP
/// `DiagnosticTag`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FindingTag {
    /// The code is redundant and can be removed — rendered faded.
    Unnecessary,
    /// The code uses a deprecated construct — rendered struck through.
    Deprecated,
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
        .with_help("bind the table name with LET before the query")
        .with_tag(FindingTag::Unnecessary);

        assert_eq!(finding.span(), &span);
        assert_eq!(finding.help().len(), 1);
        assert_eq!(finding.tags(), &[FindingTag::Unnecessary]);
        assert_eq!(finding.code().to_string(), "E6001");
        assert_eq!(finding.severity(), Severity::Warning);
        assert_eq!(
            finding.message(),
            "dynamic table name cannot be fully analyzed"
        );
    }
}
