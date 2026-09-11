//! In-source suppression: a `surrealql-analyzer: allow(E1001) reason` comment
//! suppresses matching findings on the next line — or on its own line
//! when it trails code. Applied at analysis time, rustc-style: a
//! suppressed finding never leaves the pipeline. Directives that fail
//! their own contract (unknown code, name instead of code, missing
//! reason when the workspace requires one) are 7013.

use surrealql_analyzer_diagnostics::{parse_suppression_directive, Finding, SuppressionTarget};
use surrealql_analyzer_syntax::source::SourceId;
use surrealql_analyzer_syntax::span::{ByteRange, SourceSpan};

pub fn apply_suppressions(
    source: &SourceId,
    text: &str,
    require_reasons: bool,
    diagnostics: &mut Vec<Finding>,
) {
    let mut line_starts: Vec<u32> = vec![0];
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            line_starts.push(index as u32 + 1);
        }
    }
    let line_of = |offset: u32| match line_starts.binary_search(&offset) {
        Ok(line) => line,
        Err(line) => line - 1,
    };

    // (covered line, rendered code) pairs from well-formed directives.
    let mut covers: Vec<(usize, String)> = Vec::new();
    let mut directive_findings: Vec<Finding> = Vec::new();

    for (line_no, line) in text.lines().enumerate() {
        let Some(comment_start) = comment_start(line) else {
            continue;
        };
        let comment = line[comment_start..]
            .trim_start_matches(['-', '/', '#'])
            .trim();
        if !comment.starts_with("surrealql-analyzer:") {
            continue;
        }

        let line_start = line_starts[line_no];
        let span = SourceSpan::new(
            source.clone(),
            ByteRange::new(
                line_start + comment_start as u32,
                line_start + line.len() as u32,
            )
            .expect("comment offsets are ordered"),
        );
        let directive_finding = |message: String| {
            surrealql_analyzer_diagnostics::catalog::finding(span.clone(), 7013, message)
        };

        let suppression = match parse_suppression_directive(comment, span.clone()) {
            Ok(suppression) => suppression,
            Err(error) => {
                directive_findings.push(directive_finding(format!(
                    "this suppression directive does not parse ({error:?}); write `surrealql-analyzer: allow(E1001) reason=\"why\"`",
                )));
                continue;
            }
        };

        let code = match suppression.target() {
            SuppressionTarget::Code(code) => code.clone(),
            SuppressionTarget::Name(name) => {
                directive_findings.push(directive_finding(format!(
                    "suppress by catalog code, not name: `{name}` matches nothing",
                )));
                continue;
            }
        };
        let known = code
            .get(1..)
            .and_then(|digits| digits.parse::<u16>().ok())
            .is_some_and(|number| surrealql_analyzer_diagnostics::catalog::entry(number).is_some());
        if !known {
            directive_findings.push(directive_finding(
                format!("`{code}` is not a catalog code",),
            ));
            continue;
        }
        if require_reasons && suppression.reason().is_none() {
            directive_findings.push(directive_finding(format!(
                "this workspace requires a reason: `surrealql-analyzer: allow({code}) reason=\"why\"`",
            )));
            continue;
        }

        // A directive on its own line covers the next line; trailing a
        // statement, it covers its own line.
        let own_line = line[..comment_start].trim().is_empty();
        let covered = if own_line { line_no + 1 } else { line_no };
        covers.push((covered, code));
    }

    if !covers.is_empty() {
        diagnostics.retain(|finding| {
            if finding.span().source() != source {
                return true;
            }
            let line = line_of(finding.span().range().start());
            let code = finding.code().to_string();
            !covers
                .iter()
                .any(|(covered, target)| *covered == line && *target == code)
        });
    }
    diagnostics.extend(directive_findings);
}

/// The byte offset where a line comment begins (`--`, `//`, or `#`),
/// skipping quoted strings so `'a -- b'` is not a comment.
fn comment_start(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut quote: Option<u8> = None;
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        match quote {
            Some(open) => {
                if byte == open {
                    quote = None;
                }
            }
            None => match byte {
                b'\'' | b'"' | b'`' => quote = Some(byte),
                b'#' => return Some(i),
                b'-' if bytes.get(i + 1) == Some(&b'-') => return Some(i),
                b'/' if bytes.get(i + 1) == Some(&b'/') => return Some(i),
                _ => {}
            },
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::comment_start;

    #[test]
    fn comment_start_skips_quoted_strings() {
        assert_eq!(comment_start("SELECT '--' FROM a; -- tail"), Some(20));
        assert_eq!(comment_start("SELECT \"a // b\" FROM a"), None);
        assert_eq!(comment_start("# leading"), Some(0));
        assert_eq!(comment_start("// leading"), Some(0));
        assert_eq!(comment_start("plain line"), None);
    }
}
