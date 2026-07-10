//! rustc-style finding rendering: severity header, source excerpt with a
//! caret underline, `help:` suggestions, and related locations.

use std::collections::BTreeMap;

use surrealguard_diagnostics::{render_code, Finding, Severity};
use surrealguard_syntax::span::SourceSpan;

/// Renders one finding against the loaded source texts (keyed by the
/// span's source id string). Sources without text (virtual, unreadable)
/// degrade to the header-and-location form.
pub fn render_finding(
    finding: &Finding,
    severity: Severity,
    texts: &BTreeMap<String, String>,
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{}[{}]: {}\n",
        severity_label(severity),
        render_code(finding.code(), severity),
        finding.message()
    ));
    push_snippet(&mut out, finding.span(), texts);

    for help in finding.help() {
        out.push_str(&format!("  = help: {}\n", help.message));
    }
    for related in finding.related() {
        out.push_str(&format!("note: {}\n", related.message));
        push_snippet(&mut out, &related.span, texts);
    }
    out
}

fn push_snippet(out: &mut String, span: &SourceSpan, texts: &BTreeMap<String, String>) {
    let source = span.source().to_string();
    let display = display_source(&source);
    let Some(text) = texts.get(&source) else {
        out.push_str(&format!(
            "  --> {display}:{}..{}\n",
            span.range().start(),
            span.range().end()
        ));
        return;
    };

    let start = span.range().start() as usize;
    let end = (span.range().end() as usize).max(start + 1);
    let (line_no, col, line_text) = locate(text, start);
    out.push_str(&format!("  --> {display}:{}:{}\n", line_no + 1, col + 1));

    let gutter = (line_no + 1).to_string();
    let pad = " ".repeat(gutter.len());
    out.push_str(&format!("{pad} |\n"));
    out.push_str(&format!("{gutter} | {line_text}\n"));

    // Caret width: the span's portion of this line (multi-line spans
    // underline to the line's end).
    let line_remaining = line_text.chars().count().saturating_sub(col).max(1);
    let span_chars = text
        .get(start..end)
        .map(|s| s.chars().count().max(1))
        .unwrap_or(1);
    let carets = "^".repeat(span_chars.min(line_remaining));
    out.push_str(&format!("{pad} | {}{carets}\n", " ".repeat(col)));
}

/// Zero-based line number, character column, and the line's text for a
/// byte offset.
fn locate(text: &str, offset: usize) -> (usize, usize, String) {
    let clamped = offset.min(text.len());
    let line_start = text[..clamped].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_no = text[..line_start].matches('\n').count();
    let line_end = text[line_start..]
        .find('\n')
        .map(|i| line_start + i)
        .unwrap_or(text.len());
    let col = text[line_start..clamped].chars().count();
    (line_no, col, text[line_start..line_end].to_string())
}

/// File-URL source ids render as paths relative to the working
/// directory; everything else displays verbatim.
fn display_source(source: &str) -> String {
    let path = source.strip_prefix("file://").unwrap_or(source);
    match std::env::current_dir() {
        Ok(cwd) => std::path::Path::new(path)
            .strip_prefix(&cwd)
            .map(|relative| relative.display().to_string())
            .unwrap_or_else(|_| path.to_string()),
        Err(_) => path.to_string(),
    }
}

fn severity_label(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Hint => "hint",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealguard_diagnostics::catalog;
    use surrealguard_syntax::source::SourceId;
    use surrealguard_syntax::span::{ByteRange, SourceSpan};

    fn texts(name: &str, text: &str) -> BTreeMap<String, String> {
        BTreeMap::from([(name.to_string(), text.to_string())])
    }

    #[test]
    fn renders_header_snippet_caret_and_help() {
        let text = "DEFINE TABLE person;\nSELECT * FROM persn;";
        let span = SourceSpan::new(
            SourceId::new("queries.surql"),
            ByteRange::new(35, 40).expect("ordered"),
        );
        let finding = catalog::finding(span, 1001, "unknown table `persn`")
            .with_help("did you mean `person`?");

        let rendered = render_finding(&finding, Severity::Error, &texts("queries.surql", text));

        assert_eq!(
            rendered,
            "error[E1001]: unknown table `persn`\n  \
             --> queries.surql:2:15\n  \
             |\n\
             2 | SELECT * FROM persn;\n  \
             |               ^^^^^\n  \
             = help: did you mean `person`?\n"
        );
    }

    #[test]
    fn renders_related_locations_as_notes() {
        let text =
            "DEFINE TABLE likes TYPE RELATION IN person OUT post;\nRELATE post:1->likes->person:1;";
        let span = SourceSpan::new(
            SourceId::new("s.surql"),
            ByteRange::new(69, 74).expect("ordered"),
        );
        let declared = SourceSpan::new(
            SourceId::new("s.surql"),
            ByteRange::new(13, 18).expect("ordered"),
        );
        let finding = catalog::finding(span, 3002, "relation `likes` misused")
            .with_related(declared, "relation `likes` declared here");

        let rendered = render_finding(&finding, Severity::Error, &texts("s.surql", text));

        assert!(rendered.contains("note: relation `likes` declared here"));
        assert!(rendered.contains("--> s.surql:1:14"));
    }

    #[test]
    fn missing_source_text_degrades_to_offsets() {
        let span = SourceSpan::new(
            SourceId::new("gone.surql"),
            ByteRange::new(3, 9).expect("ordered"),
        );
        let finding = catalog::finding(span, 1001, "unknown table `x`");

        let rendered = render_finding(&finding, Severity::Error, &BTreeMap::new());

        assert!(rendered.contains("--> gone.surql:3..9"));
        assert!(!rendered.contains(" | "));
    }
}
