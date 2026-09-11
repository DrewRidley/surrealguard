//! Convert surrealguard diagnostics to LSP diagnostics.
//!
//! Handles span-to-range conversion and related information formatting.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use tower_lsp::lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, DiagnosticTag, Location,
    NumberOrString, Url,
};

use surrealguard_diagnostics::{Finding, FindingTag, PolicyConfig, Severity as WorkspaceSeverity};

use crate::text::LineIndex;

/// What a batch of findings for one document converts against: the target
/// document's line index, and the other documents a related span can point
/// at.
///
/// A document's findings are converted one after another, and each one maps
/// at least one span. Converting a span by scanning the text costs O(file),
/// so a file's own diagnostics used to cost O(file x findings) — the quadratic
/// this type exists to remove. The target's index is built once by the
/// document that owns it; a related file's is built the first time a span
/// actually points there and then reused for the rest of the batch.
pub struct DiagnosticContext<'a> {
    /// The line index of the document the findings belong to.
    source: &'a LineIndex,
    /// Every analyzed document keyed by its analysis source id, for resolving
    /// a related span that points at another file.
    texts: &'a BTreeMap<String, (Url, Arc<str>)>,
    /// Line indexes for those other documents, built on demand. A `Mutex`
    /// rather than a `RefCell` so the context stays `Sync` and can live
    /// inside an async handler.
    related: Mutex<HashMap<String, Arc<LineIndex>>>,
}

impl<'a> DiagnosticContext<'a> {
    /// A context for converting `source`'s findings, resolving related spans
    /// through `texts`.
    pub fn new(source: &'a LineIndex, texts: &'a BTreeMap<String, (Url, Arc<str>)>) -> Self {
        Self {
            source,
            texts,
            related: Mutex::new(HashMap::new()),
        }
    }

    /// The URI and line index of the document `key` names, or `None` when it
    /// is not one of the analyzed documents (a related span pointing at a file
    /// the editor does not track has no location to offer).
    fn related_source(&self, key: &str) -> Option<(Url, Arc<LineIndex>)> {
        let (uri, text) = self.texts.get(key)?;
        let mut related = self.related.lock().ok()?;
        let index = related
            .entry(key.to_string())
            .or_insert_with(|| Arc::new(LineIndex::new(Arc::clone(text))));
        Some((uri.clone(), Arc::clone(index)))
    }
}

/// Convert a workspace-analysis finding to an LSP diagnostic, applying the
/// workspace policy at this consumption edge. Returns `None` when policy
/// suppresses the finding (an allowed lint).
pub fn workspace_finding_to_lsp_diagnostic(
    context: &DiagnosticContext<'_>,
    finding: &Finding,
    policy: &PolicyConfig,
) -> Option<Diagnostic> {
    let resolved = policy.resolve_severity(finding.code(), finding.severity())?;
    let range = finding.span().range();
    let range = context
        .source
        .range(range.start() as usize, range.end() as usize);

    let code = surrealguard_diagnostics::render_code(finding.code(), resolved);
    let severity = match resolved {
        WorkspaceSeverity::Error => DiagnosticSeverity::ERROR,
        WorkspaceSeverity::Warning => DiagnosticSeverity::WARNING,
        WorkspaceSeverity::Hint => DiagnosticSeverity::HINT,
    };

    // LSP diagnostics have no help slot; suggestions ride the message.
    let mut message = finding.message().to_string();
    for help in finding.help() {
        message.push_str("\nhelp: ");
        message.push_str(&help.message);
    }

    let related_information: Vec<DiagnosticRelatedInformation> = finding
        .related()
        .iter()
        .filter_map(|related| {
            let (uri, index) = context.related_source(&related.span.source().to_string())?;
            let range = related.span.range();
            Some(DiagnosticRelatedInformation {
                location: Location {
                    uri,
                    range: index.range(range.start() as usize, range.end() as usize),
                },
                message: related.message.clone(),
            })
        })
        .collect();

    let mut tags: Vec<DiagnosticTag> = finding
        .tags()
        .iter()
        .map(|tag| match tag {
            FindingTag::Unnecessary => DiagnosticTag::UNNECESSARY,
            FindingTag::Deprecated => DiagnosticTag::DEPRECATED,
        })
        .collect();

    // Dead-code and redundancy findings should render greyed even when the
    // finding itself carries no tag: the code family is the signal. Editors
    // dedupe, but avoid emitting the tag twice.
    if marks_code_unnecessary(finding.code().number())
        && !tags.contains(&DiagnosticTag::UNNECESSARY)
    {
        tags.push(DiagnosticTag::UNNECESSARY);
    }

    Some(Diagnostic {
        range,
        severity: Some(severity),
        code: Some(NumberOrString::String(code)),
        source: Some("surrealguard".to_string()),
        message,
        related_information: (!related_information.is_empty()).then_some(related_information),
        tags: (!tags.is_empty()).then_some(tags),
        ..Diagnostic::default()
    })
}

/// Whether a finding's code marks the flagged span as unnecessary or
/// redundant — dead code the editor should grey out. The `Finding` model
/// has no intrinsic "redundant" category, so this maps the small set of
/// catalog codes whose contract is "this code can be removed":
/// unreachable statements (4006), duplicate SET targets (4010), duplicate
/// projections (4011), and unused LET bindings (7001).
fn marks_code_unnecessary(number: u16) -> bool {
    matches!(number, 4006 | 4010 | 4011 | 7001)
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealguard_diagnostics::{Finding, FindingCode, Severity};
    use surrealguard_syntax::source::SourceId;
    use surrealguard_syntax::span::{ByteRange, SourceSpan};

    #[test]
    fn help_related_and_tags_reach_the_lsp_diagnostic() {
        let schema_uri = Url::parse("file:///workspace/schema.surql").expect("valid url");
        let schema_text = "DEFINE TABLE likes TYPE RELATION IN person OUT post;";
        let texts = BTreeMap::from([(
            "file:///workspace/schema.surql".to_string(),
            (schema_uri.clone(), Arc::from(schema_text)),
        )]);

        let source = "SELECT * FROM persn;";
        let finding = Finding::new(
            SourceSpan::new(
                SourceId::new("file:///workspace/query.surql"),
                ByteRange::new(14, 19).expect("valid range"),
            ),
            FindingCode::schema(1001),
            Severity::Error,
            "unknown table `persn`",
        )
        .with_help("did you mean `person`?")
        .with_related(
            SourceSpan::new(
                SourceId::new("file:///workspace/schema.surql"),
                ByteRange::new(13, 18).expect("valid range"),
            ),
            "relation `likes` declared here",
        )
        .with_tag(FindingTag::Unnecessary);

        let index = LineIndex::for_str(source);
        let diagnostic = workspace_finding_to_lsp_diagnostic(
            &DiagnosticContext::new(&index, &texts),
            &finding,
            &PolicyConfig::default(),
        )
        .expect("passes default policy");

        assert_eq!(
            diagnostic.message,
            "unknown table `persn`\nhelp: did you mean `person`?"
        );
        let related = diagnostic.related_information.expect("related present");
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].location.uri, schema_uri);
        assert_eq!(related[0].location.range.start.character, 13);
        assert_eq!(related[0].message, "relation `likes` declared here");
        assert_eq!(diagnostic.tags, Some(vec![DiagnosticTag::UNNECESSARY]));
    }

    #[test]
    fn dead_code_findings_are_tagged_unnecessary_by_code_family() {
        // 4006 (unreachable) carries no tag at emission, but the LSP greys
        // it out from the code alone.
        let source = "RETURN 1;\nRETURN 2;";
        let finding = Finding::new(
            SourceSpan::new(
                SourceId::new("file:///workspace/query.surql"),
                ByteRange::new(10, 19).expect("valid range"),
            ),
            FindingCode::statement(4006),
            Severity::Warning,
            "unreachable statement",
        );
        assert!(finding.tags().is_empty(), "fixture has no intrinsic tag");

        let index = LineIndex::for_str(source);
        let diagnostic = workspace_finding_to_lsp_diagnostic(
            &DiagnosticContext::new(&index, &BTreeMap::new()),
            &finding,
            &PolicyConfig::default(),
        )
        .expect("passes default policy");

        assert_eq!(diagnostic.tags, Some(vec![DiagnosticTag::UNNECESSARY]));
    }

    #[test]
    fn ordinary_findings_carry_no_unnecessary_tag() {
        let source = "SELECT * FROM ghost;";
        let finding = Finding::new(
            SourceSpan::new(
                SourceId::new("file:///workspace/query.surql"),
                ByteRange::new(14, 19).expect("valid range"),
            ),
            FindingCode::schema(1001),
            Severity::Error,
            "unknown table `ghost`",
        );

        let index = LineIndex::for_str(source);
        let diagnostic = workspace_finding_to_lsp_diagnostic(
            &DiagnosticContext::new(&index, &BTreeMap::new()),
            &finding,
            &PolicyConfig::default(),
        )
        .expect("passes default policy");

        assert_eq!(diagnostic.tags, None);
    }

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

        let index = LineIndex::for_str(source);
        let diagnostic = workspace_finding_to_lsp_diagnostic(
            &DiagnosticContext::new(&index, &BTreeMap::new()),
            &finding,
            &PolicyConfig::default(),
        )
        .expect("non-lint findings pass default policy");

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
