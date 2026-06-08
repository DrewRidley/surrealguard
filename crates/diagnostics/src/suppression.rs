use serde::{Deserialize, Serialize};
use surrealguard_syntax::span::SourceSpan;

const DIRECTIVE_PREFIX: &str = "surrealguard:";
const ALLOW_PREFIX: &str = "allow(";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suppression {
    target: SuppressionTarget,
    reason: Option<String>,
    span: SourceSpan,
}

impl Suppression {
    pub fn new(target: SuppressionTarget, reason: Option<String>, span: SourceSpan) -> Self {
        Self {
            target,
            reason,
            span,
        }
    }

    pub fn target(&self) -> &SuppressionTarget {
        &self.target
    }

    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    pub fn span(&self) -> &SourceSpan {
        &self.span
    }

    pub fn matches_finding_code(&self, code: crate::FindingCode) -> bool {
        match &self.target {
            SuppressionTarget::Code(target) => target == &code.to_string(),
            SuppressionTarget::Name(_) => false,
        }
    }

    pub fn matches_name(&self, name: &str) -> bool {
        match &self.target {
            SuppressionTarget::Code(_) => false,
            SuppressionTarget::Name(target) => target == name,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SuppressionTarget {
    Code(String),
    Name(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SuppressionParseError {
    BlanketSuppression,
    EmptyTarget,
    MalformedDirective,
    MalformedReason,
}

pub fn parse_optional_suppression_directive(
    text: &str,
    span: SourceSpan,
) -> Result<Option<Suppression>, SuppressionParseError> {
    let Some(body) = text.trim().strip_prefix(DIRECTIVE_PREFIX) else {
        return Ok(None);
    };

    parse_allow_body(body.trim(), span).map(Some)
}

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
    use surrealguard_syntax::source::SourceId;
    use surrealguard_syntax::span::ByteRange;

    fn span(start: u32, end: u32) -> SourceSpan {
        SourceSpan::new(
            SourceId::new("query:001"),
            ByteRange::new(start, end).expect("valid range"),
        )
    }

    #[test]
    fn parses_named_suppression_with_reason() {
        let parsed = parse_suppression_directive(
            "surrealguard: allow(lint.select_star) reason=\"intentional projection\"",
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
        let parsed = parse_suppression_directive("surrealguard: allow(L7001)", span(0, 29))
            .expect("directive should parse");

        assert_eq!(parsed.target(), &SuppressionTarget::Code("L7001".into()));
        assert_eq!(parsed.reason(), None);
    }

    #[test]
    fn rejects_blanket_suppression() {
        let error = parse_suppression_directive("surrealguard: allow(*)", span(0, 21))
            .expect_err("blanket suppressions should be rejected");

        assert_eq!(error, SuppressionParseError::BlanketSuppression);
    }

    #[test]
    fn rejects_malformed_allow_directive() {
        let error =
            parse_suppression_directive("surrealguard: allow lint.select_star", span(0, 37))
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
        let parsed = parse_suppression_directive("surrealguard: allow(W6001)", span(0, 29))
            .expect("directive should parse");

        assert!(parsed.matches_finding_code(crate::FindingCode::dynamic(6001)));
        assert!(!parsed.matches_finding_code(crate::FindingCode::lint(7001)));
    }

    #[test]
    fn named_target_matches_exact_lint_name() {
        let parsed =
            parse_suppression_directive("surrealguard: allow(lint.select_star)", span(0, 40))
                .expect("directive should parse");

        assert!(parsed.matches_name("lint.select_star"));
        assert!(!parsed.matches_name("lint.dynamic_query"));
    }
}
