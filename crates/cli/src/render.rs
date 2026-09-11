//! rustc-style finding rendering: severity header, source excerpt with a
//! caret underline, `help:` suggestions, and related locations.
//!
//! Colour is a parameter, not an ambient decision — see [`crate::style`]. The
//! same finding rendered plain and rendered coloured differs only by escape
//! sequences, which is what makes piping to a file safe.

use std::collections::BTreeMap;

use surrealql_analyzer_diagnostics::{render_code, Finding, Severity};
use surrealql_analyzer_syntax::span::SourceSpan;

use crate::style::Styles;

/// Renders one finding against the loaded source texts (keyed by the
/// span's source id string). Sources without text (virtual, unreadable)
/// degrade to the header-and-location form.
pub fn render_finding(
    finding: &Finding,
    severity: Severity,
    texts: &BTreeMap<String, String>,
    styles: Styles,
) -> String {
    let mut out = String::new();
    let header = format!(
        "{}[{}]",
        severity_label(severity),
        render_code(finding.code(), severity)
    );
    out.push_str(&format!(
        "{}: {}\n",
        styles.severity(severity, &header),
        styles.message(finding.message())
    ));
    let width = push_snippet(&mut out, finding.span(), texts, styles, Some(severity));

    let pad = " ".repeat(width);
    for help in finding.help() {
        // A blank scaffold line separates the excerpt from its suggestion, the
        // way rustc does — the caret line and the help text are two different
        // thoughts and should not run together.
        out.push_str(&format!("{}\n", styles.frame(&format!("{pad} |"))));
        out.push_str(&format!(
            "{pad} {} {} {}\n",
            styles.frame("="),
            styles.label("help:"),
            help.message
        ));
    }
    for related in finding.related() {
        out.push_str(&format!(
            "{} {}\n",
            styles.label("note:"),
            styles.message(&related.message)
        ));
        push_snippet(&mut out, &related.span, texts, styles, None);
    }
    out
}

/// Appends the `--> file:line:col` locator and, when the source text is
/// available, the excerpt and its caret underline. Returns the gutter width so
/// the caller can align whatever it writes underneath.
///
/// `severity` colours the carets; `None` marks a related (secondary) span,
/// which is underlined in the scaffold's own colour so it cannot be mistaken
/// for the finding itself.
fn push_snippet(
    out: &mut String,
    span: &SourceSpan,
    texts: &BTreeMap<String, String>,
    styles: Styles,
    severity: Option<Severity>,
) -> usize {
    let source = span.source().to_string();
    let display = display_source(&source);
    let Some(text) = texts.get(&source) else {
        out.push_str(&format!(
            "  {} {}\n",
            styles.frame("-->"),
            styles.path(&format!(
                "{display}:{}..{}",
                span.range().start(),
                span.range().end()
            ))
        ));
        return 1;
    };

    let start = span.range().start() as usize;
    let end = (span.range().end() as usize).max(start + 1);
    let (line_no, col, line_text) = locate(text, start);

    // Every scaffold column is derived from the line number's width, so a
    // finding on line 7 and one on line 1204 both line up under their own
    // gutter instead of drifting apart.
    let gutter = (line_no + 1).to_string();
    let width = gutter.len();
    let pad = " ".repeat(width);

    out.push_str(&format!(
        "{pad} {} {}\n",
        styles.frame("-->"),
        styles.path(&format!("{display}:{}:{}", line_no + 1, col + 1))
    ));
    out.push_str(&format!("{}\n", styles.frame(&format!("{pad} |"))));
    out.push_str(&format!(
        "{} {line_text}\n",
        styles.frame(&format!("{gutter} |"))
    ));

    // Caret width: the span's portion of this line (multi-line spans
    // underline to the line's end).
    let line_remaining = line_text.chars().count().saturating_sub(col).max(1);
    let span_chars = text.get(start..end).map_or(1, |s| s.chars().count().max(1));
    let carets = "^".repeat(span_chars.min(line_remaining));
    out.push_str(&format!(
        "{} {}{}\n",
        styles.frame(&format!("{pad} |")),
        " ".repeat(col),
        styles.carets(&carets, severity)
    ));
    width
}

/// Zero-based line number, character column, and the line's text for a
/// byte offset.
fn locate(text: &str, offset: usize) -> (usize, usize, String) {
    let clamped = offset.min(text.len());
    let line_start = text[..clamped].rfind('\n').map_or(0, |i| i + 1);
    let line_no = text[..line_start].matches('\n').count();
    let line_end = text[line_start..]
        .find('\n')
        .map_or(text.len(), |i| line_start + i);
    let col = text[line_start..clamped].chars().count();
    (line_no, col, text[line_start..line_end].to_string())
}

/// File-URL source ids render as paths relative to the working
/// directory; everything else displays verbatim.
fn display_source(source: &str) -> String {
    let path = source.strip_prefix("file://").unwrap_or(source);
    match std::env::current_dir() {
        Ok(cwd) => std::path::Path::new(path).strip_prefix(&cwd).map_or_else(
            |_| path.to_string(),
            |relative| relative.display().to_string(),
        ),
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
    use surrealql_analyzer_diagnostics::catalog;
    use surrealql_analyzer_syntax::source::SourceId;
    use surrealql_analyzer_syntax::span::{ByteRange, SourceSpan};

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

        let rendered = render_finding(
            &finding,
            Severity::Error,
            &texts("queries.surql", text),
            Styles::plain(),
        );

        assert_eq!(
            rendered,
            "error[E1001]: unknown table `persn`\n  \
             --> queries.surql:2:15\n  \
             |\n\
             2 | SELECT * FROM persn;\n  \
             |               ^^^^^\n  \
             |\n  \
             = help: did you mean `person`?\n"
        );
    }

    #[test]
    fn the_scaffold_stays_aligned_past_line_nine() {
        // Every column is derived from the line number's width. A hard-coded
        // indent lines up on line 2 and falls apart on line 12, which is where
        // real files live.
        let mut text = "DEFINE TABLE person;\n".repeat(11);
        let offset = text.len() + 14;
        text.push_str("SELECT * FROM persn;");
        let span = SourceSpan::new(
            SourceId::new("q.surql"),
            ByteRange::new(offset as u32, offset as u32 + 5).expect("ordered"),
        );
        let finding =
            catalog::finding(span, 1001, "unknown table `persn`").with_help("did you mean?");

        let rendered = render_finding(
            &finding,
            Severity::Error,
            &texts("q.surql", &text),
            Styles::plain(),
        );

        let bar = |line: &str| line.find('|').expect("every scaffold line has a bar");
        let lines: Vec<&str> = rendered.lines().collect();
        let arrow = lines
            .iter()
            .find(|line| line.contains("-->"))
            .expect("locator");
        assert_eq!(arrow.find("-->"), Some(3), "arrow sits under the gutter");
        let bars: Vec<usize> = lines
            .iter()
            .filter(|line| line.contains('|'))
            .map(|line| bar(line))
            .collect();
        assert!(
            bars.windows(2).all(|pair| pair[0] == pair[1]),
            "every bar shares one column: {bars:?} in\n{rendered}"
        );
        assert!(
            rendered.contains("   = help:"),
            "help aligns too:\n{rendered}"
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

        let rendered = render_finding(
            &finding,
            Severity::Error,
            &texts("s.surql", text),
            Styles::plain(),
        );

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

        let rendered = render_finding(&finding, Severity::Error, &BTreeMap::new(), Styles::plain());

        assert!(rendered.contains("--> gone.surql:3..9"));
        assert!(!rendered.contains(" | "));
    }

    #[test]
    fn color_adds_escapes_and_nothing_else() {
        // The piping contract, asserted at the only place that can break it:
        // strip the escapes from the coloured render and the plain render must
        // come back byte for byte.
        let text = "DEFINE TABLE person;\nSELECT * FROM persn;";
        let span = SourceSpan::new(
            SourceId::new("queries.surql"),
            ByteRange::new(35, 40).expect("ordered"),
        );
        let finding = catalog::finding(span, 1001, "unknown table `persn`")
            .with_help("did you mean `person`?");
        let texts = texts("queries.surql", text);

        let plain = render_finding(&finding, Severity::Error, &texts, Styles::plain());
        let colored = render_finding(&finding, Severity::Error, &texts, Styles::colored());

        assert!(colored.contains('\x1b'), "colour should paint something");
        assert_eq!(strip_ansi(&colored), plain);
    }

    #[test]
    fn a_warning_and_an_error_do_not_share_a_color() {
        let span = SourceSpan::new(
            SourceId::new("gone.surql"),
            ByteRange::new(0, 1).expect("ordered"),
        );
        let finding = catalog::finding(span, 1001, "m");
        let error = render_finding(
            &finding,
            Severity::Error,
            &BTreeMap::new(),
            Styles::colored(),
        );
        let warning = render_finding(
            &finding,
            Severity::Warning,
            &BTreeMap::new(),
            Styles::colored(),
        );
        assert_ne!(
            error, warning,
            "severity must be visible without reading the word"
        );
    }

    fn strip_ansi(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut chars = text.chars();
        while let Some(ch) = chars.next() {
            if ch != '\x1b' {
                out.push(ch);
                continue;
            }
            // CSI: `\x1b[` … final byte in `@`..`~`.
            for ch in chars.by_ref() {
                if ('@'..='~').contains(&ch) && ch != '[' {
                    break;
                }
            }
        }
        out
    }
}
