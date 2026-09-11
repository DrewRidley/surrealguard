//! In-source suppression directives: `-- surrealql-analyzer: allow(E1001)`
//! comments parsed into [`Suppression`] targets matched by code or lint
//! name.

use serde::{Deserialize, Serialize};
use surrealql_analyzer_syntax::span::SourceSpan;

const DIRECTIVE_PREFIX: &str = "surrealql-analyzer:";
const ALLOW_PREFIX: &str = "allow(";

/// A parsed `allow(...)` directive: what it silences, an optional reason,
/// and the source span of the directive itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suppression {
    target: SuppressionTarget,
    reason: Option<String>,
    span: SourceSpan,
}

impl Suppression {
    /// Builds a suppression from its parsed parts.
    pub fn new(target: SuppressionTarget, reason: Option<String>, span: SourceSpan) -> Self {
        Self {
            target,
            reason,
            span,
        }
    }

    /// What the directive silences — a code or a lint name.
    pub fn target(&self) -> &SuppressionTarget {
        &self.target
    }

    /// The justification given in the directive, if any.
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    /// The location of the directive comment in source.
    pub fn span(&self) -> &SourceSpan {
        &self.span
    }

    /// Whether this directive targets the given code (`allow(E1001)`).
    pub fn matches_finding_code(&self, code: crate::FindingCode) -> bool {
        match &self.target {
            SuppressionTarget::Code(target) => target == &code.to_string(),
            SuppressionTarget::Name(_) => false,
        }
    }

    /// Whether this directive targets the given lint name
    /// (`allow(lint.select_star)`).
    pub fn matches_name(&self, name: &str) -> bool {
        match &self.target {
            SuppressionTarget::Code(_) => false,
            SuppressionTarget::Name(target) => target == name,
        }
    }
}

/// What an `allow(...)` directive names: a rendered code or a lint name.
/// Blanket (`*`) targets are rejected at parse time.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SuppressionTarget {
    /// A rendered finding code such as `E1001` or `L7001`.
    Code(String),
    /// A dotted lint name such as `lint.select_star`.
    Name(String),
}

/// Why a suppression directive failed to parse. Each variant is a distinct
/// contract violation the surface reports back at the directive's span.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SuppressionParseError {
    /// The target was `*` — blanket suppression is not permitted.
    BlanketSuppression,
    /// The `allow(...)` parentheses were empty.
    EmptyTarget,
    /// The directive did not match the `allow(target)` grammar.
    MalformedDirective,
    /// A `reason=` clause was present but not a valid quoted string.
    MalformedReason,
}

/// Parses a directive from a comment body, returning `Ok(None)` when the
/// comment is not a `surrealql-analyzer:` directive at all. Use this when
/// scanning ordinary comments; malformed directives still return `Err`.
pub fn parse_optional_suppression_directive(
    text: &str,
    span: SourceSpan,
) -> Result<Option<Suppression>, SuppressionParseError> {
    let Some(body) = text.trim().strip_prefix(DIRECTIVE_PREFIX) else {
        return Ok(None);
    };

    parse_allow_body(body.trim(), span).map(Some)
}

/// Parses a comment body that is required to be a `surrealql-analyzer:`
/// directive, erroring if the prefix is absent. Use this when the caller
/// has already committed to the comment being a directive.
pub fn parse_suppression_directive(
    text: &str,
    span: SourceSpan,
) -> Result<Suppression, SuppressionParseError> {
    let body = text
        .trim()
        .strip_prefix(DIRECTIVE_PREFIX)
        .ok_or(SuppressionParseError::MalformedDirective)?;

    parse_allow_body(body.trim(), span)
}

fn parse_allow_body(body: &str, span: SourceSpan) -> Result<Suppression, SuppressionParseError> {
    let after_allow = body
        .strip_prefix(ALLOW_PREFIX)
        .ok_or(SuppressionParseError::MalformedDirective)?;
    let close = after_allow
        .find(')')
        .ok_or(SuppressionParseError::MalformedDirective)?;

    let raw_target = after_allow[..close].trim();
    let rest = after_allow[close + 1..].trim();

    let target = parse_target(raw_target)?;
    let reason = parse_reason(rest)?;

    Ok(Suppression::new(target, reason, span))
}

fn parse_target(raw: &str) -> Result<SuppressionTarget, SuppressionParseError> {
    if raw.is_empty() {
        return Err(SuppressionParseError::EmptyTarget);
    }
    if raw == "*" {
        return Err(SuppressionParseError::BlanketSuppression);
    }
    if is_finding_code(raw) {
        return Ok(SuppressionTarget::Code(raw.to_owned()));
    }
    if is_suppression_name(raw) {
        return Ok(SuppressionTarget::Name(raw.to_owned()));
    }

    Err(SuppressionParseError::MalformedDirective)
}

fn parse_reason(rest: &str) -> Result<Option<String>, SuppressionParseError> {
    if rest.is_empty() {
        return Ok(None);
    }

    let raw = rest
        .strip_prefix("reason=")
        .ok_or(SuppressionParseError::MalformedDirective)?;

    let quoted = raw
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .ok_or(SuppressionParseError::MalformedReason)?;

    Ok(Some(quoted.to_owned()))
}

fn is_finding_code(raw: &str) -> bool {
    let mut chars = raw.chars();
    matches!(chars.next(), Some('S' | 'E' | 'W' | 'L'))
        && chars.clone().count() == 4
        && chars.all(|ch| ch.is_ascii_digit())
}

fn is_suppression_name(raw: &str) -> bool {
    raw.split('.').all(is_identifier_part) && raw.contains('.')
}

fn is_identifier_part(raw: &str) -> bool {
    let mut chars = raw.chars();
    matches!(chars.next(), Some(ch) if ch.is_ascii_alphabetic() || ch == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealql_analyzer_syntax::source::SourceId;
    use surrealql_analyzer_syntax::span::ByteRange;

    fn span(start: u32, end: u32) -> SourceSpan {
        SourceSpan::new(
            SourceId::new("query:001"),
            ByteRange::new(start, end).expect("valid range"),
        )
    }

    #[test]
    fn parses_named_suppression_with_reason() {
        let parsed = parse_suppression_directive(
            "surrealql-analyzer: allow(lint.select_star) reason=\"intentional projection\"",
            span(4, 68),
        )
        .expect("directive should parse");

        assert_eq!(
            parsed.target(),
            &SuppressionTarget::Name("lint.select_star".into())
        );
        assert_eq!(parsed.reason(), Some("intentional projection"));
        assert_eq!(parsed.span(), &span(4, 68));
    }

    #[test]
    fn parses_code_suppression_without_reason() {
        let parsed = parse_suppression_directive("surrealql-analyzer: allow(L7001)", span(0, 29))
            .expect("directive should parse");

        assert_eq!(parsed.target(), &SuppressionTarget::Code("L7001".into()));
        assert_eq!(parsed.reason(), None);
    }

    #[test]
    fn rejects_blanket_suppression() {
        let error = parse_suppression_directive("surrealql-analyzer: allow(*)", span(0, 21))
            .expect_err("blanket suppressions should be rejected");

        assert_eq!(error, SuppressionParseError::BlanketSuppression);
    }

    #[test]
    fn rejects_malformed_allow_directive() {
        let error =
            parse_suppression_directive("surrealql-analyzer: allow lint.select_star", span(0, 37))
                .expect_err("malformed allow directive should be rejected");

        assert_eq!(error, SuppressionParseError::MalformedDirective);
    }

    #[test]
    fn ignores_non_suppression_comments() {
        assert_eq!(
            parse_optional_suppression_directive("ordinary query comment", span(0, 22))
                .expect("ordinary comments should not error"),
            None
        );
    }

    #[test]
    fn code_target_matches_finding_code_text() {
        let parsed = parse_suppression_directive("surrealql-analyzer: allow(E6001)", span(0, 29))
            .expect("directive should parse");

        assert!(parsed.matches_finding_code(crate::FindingCode::param(6001)));
        assert!(!parsed.matches_finding_code(crate::FindingCode::lint(7001)));
    }

    #[test]
    fn named_target_matches_exact_lint_name() {
        let parsed =
            parse_suppression_directive("surrealql-analyzer: allow(lint.select_star)", span(0, 40))
                .expect("directive should parse");

        assert!(parsed.matches_name("lint.select_star"));
        assert!(!parsed.matches_name("lint.dynamic_query"));
    }
}
