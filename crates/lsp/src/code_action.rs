//! Suppression code actions: the text edits behind "suppress here" and
//! "suppress workspace-wide".
//!
//! Everything here is pure — a string in, a [`TextEdit`] out — so the two
//! things that can go wrong are testable without a server: producing a
//! directive that does not actually suppress, and mangling a hand-written
//! `surrealguard.toml`.
//!
//! # The code an editor shows is not the code a directive matches
//!
//! A published diagnostic carries [`surrealguard_diagnostics::render_code`]'s
//! spelling, whose letter is the *resolved severity* — `W7015`, or `I7015`
//! once a lint resolves to a hint. In-source suppression matches on
//! [`surrealguard_diagnostics::FindingCode`]'s `Display`, whose letter is the
//! *category* — `L7015` for every lint, `E1001` for every schema error.
//!
//! Copying what the editor showed is therefore wrong in two different ways:
//! `allow(W7015)` parses cleanly and silences nothing, and `allow(I7015)` is
//! not even code-shaped to the directive parser, so it reports itself as a
//! W7013. [`suppressible_code`] is the only spelling that works, and it is
//! what every edit in this module writes.

use surrealguard_diagnostics::{catalog, FindingCode};
use tower_lsp::lsp_types::{Range, TextEdit};

/// What a required `reason=` is seeded with. It is deliberately a `TODO:` so
/// it reads as unfinished in the buffer, greps like every other placeholder in
/// the tree, and — because `surrealguard check` only requires that *a* reason
/// is present — never silently ships as if it were an explanation.
const REASON_PLACEHOLDER: &str = "TODO: explain why this is allowed";

/// The canonical, actually-suppressible spelling of a code an editor rendered
/// (`W7015` → `L7015`, `E1001` → `E1001`).
///
/// `None` when the code is not one suppression can address: a syntax code
/// (`S0001`) has no catalog entry, and in-source suppression does not run at
/// all on a source that failed to parse. Refusing here is what keeps a
/// `[lints]` action from writing a key that makes the whole config fail to
/// parse — `surrealguard.toml` rejects an unknown code outright.
pub fn suppressible_code(rendered: &str) -> Option<String> {
    let digits = rendered
        .strip_prefix(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(rendered);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let number = digits.parse::<u16>().ok()?;
    catalog::entry(number)?;
    Some(FindingCode::from_number(number).to_string())
}

/// The directive comment text (no newline, no indentation).
pub fn directive(code: &str, with_reason: bool) -> String {
    if with_reason {
        format!("-- surrealguard: allow({code}) reason=\"{REASON_PLACEHOLDER}\"")
    } else {
        format!("-- surrealguard: allow({code})")
    }
}

/// An insertion of the directive on its own line immediately above the line
/// holding `offset`, indented to match it.
///
/// The own-line form (rather than a trailing comment) is what
/// `surrealguard-workspace`'s suppression scanner covers with "the next
/// line", and it is the only form that stays legible when the statement it
/// covers is long.
pub fn inline_suppression_edit(
    text: &str,
    offset: usize,
    code: &str,
    with_reason: bool,
) -> TextEdit {
    let offset = offset.min(text.len());
    let line_start = text[..offset].rfind('\n').map_or(0, |index| index + 1);
    let indent: String = text[line_start..]
        .chars()
        .take_while(|ch| *ch == ' ' || *ch == '\t')
        .collect();
    let position = crate::text::offset_to_position(text, line_start);

    TextEdit {
        range: Range::new(position, position),
        new_text: format!("{indent}{}\n", directive(code, with_reason)),
    }
}

/// The edit that makes `code` allowed in a `surrealguard.toml`, or `None`
/// when it already is.
///
/// Three shapes, in the order they are checked:
///   * an existing `[lints]` entry for this code — only its level is
///     rewritten, so a trailing comment on that line survives;
///   * a `[lints]` table with no entry for this code — one line appended at
///     the end of the table, leaving every existing line (and every comment
///     between them) byte-identical;
///   * no `[lints]` table — the table is appended at the end of the file.
///
/// A family wildcard (`"7xxx"`) is deliberately *not* treated as an existing
/// entry: a specific code overrides the family it falls under, which is
/// exactly what suppressing one code out of a family means.
pub fn lints_allow_edit(toml: &str, code: &str) -> Option<TextEdit> {
    let number = suppressible_code(code).and_then(|canonical| {
        canonical
            .get(1..)
            .and_then(|digits| digits.parse::<u16>().ok())
    })?;

    let lines = line_spans(toml);

    let Some(header) = lines.iter().position(|(_, _, line)| is_lints_header(line)) else {
        return Some(append_lints_table(toml, code));
    };

    // The table runs to the next header line, or to the end of the file.
    let end = lines
        .iter()
        .skip(header + 1)
        .position(|(_, _, line)| line.trim_start().starts_with('['))
        .map_or(lines.len(), |offset| header + 1 + offset);

    // An entry already here? Rewrite its level in place rather than adding a
    // second key for the same code (which TOML rejects outright).
    for (start, _, line) in &lines[header + 1..end] {
        let Some((key, value)) = lint_entry(line) else {
            continue;
        };
        if entry_code_number(key) != Some(number) {
            continue;
        }
        if value == "allow" {
            return None;
        }
        let value_start = start + value_offset(line)?;
        return Some(TextEdit {
            range: crate::text::byte_range_to_lsp(toml, value_start, value_start + value.len() + 2),
            new_text: "\"allow\"".to_string(),
        });
    }

    // Append to the end of the table, above any blank lines separating it
    // from whatever follows.
    let mut insert_at = end;
    while insert_at > header + 1 && lines[insert_at - 1].2.trim().is_empty() {
        insert_at -= 1;
    }
    let position = if insert_at < lines.len() {
        crate::text::offset_to_position(toml, lines[insert_at].0)
    } else {
        crate::text::offset_to_position(toml, toml.len())
    };
    // A file whose last line has no newline needs one before our entry.
    let prefix = if insert_at >= lines.len() && !toml.is_empty() && !toml.ends_with('\n') {
        "\n"
    } else {
        ""
    };

    Some(TextEdit {
        range: Range::new(position, position),
        new_text: format!("{prefix}{code} = \"allow\"\n"),
    })
}

/// A whole `[lints]` table appended at the end of a config that has none,
/// separated from the last table by exactly one blank line.
fn append_lints_table(toml: &str, code: &str) -> TextEdit {
    let mut prefix = String::new();
    if !toml.is_empty() {
        if !toml.ends_with('\n') {
            prefix.push('\n');
        }
        if !toml.ends_with("\n\n") {
            prefix.push('\n');
        }
    }
    let position = crate::text::offset_to_position(toml, toml.len());
    TextEdit {
        range: Range::new(position, position),
        new_text: format!("{prefix}[lints]\n{code} = \"allow\"\n"),
    }
}

/// Every line as `(start offset, end offset, text)`, newline excluded.
fn line_spans(text: &str) -> Vec<(usize, usize, &str)> {
    let mut spans = Vec::new();
    let mut start = 0;
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            spans.push((start, index, &text[start..index]));
            start = index + 1;
        }
    }
    if start < text.len() {
        spans.push((start, text.len(), &text[start..]));
    }
    spans
}

/// Whether a line opens the `[lints]` table — and not `[lints.something]`.
fn is_lints_header(line: &str) -> bool {
    let trimmed = line.trim_start();
    let Some(rest) = trimmed.strip_prefix("[lints]") else {
        return false;
    };
    let rest = rest.trim();
    rest.is_empty() || rest.starts_with('#')
}

/// A `key = "value"` line's key and unquoted value, or `None` for a comment,
/// a blank line, or a value that is not a plain quoted string.
fn lint_entry(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let (key, rest) = line.split_once('=')?;
    let value = rest.trim_start();
    let quoted = value.strip_prefix('"')?;
    let end = quoted.find('"')?;
    Some((key.trim(), &quoted[..end]))
}

/// Byte offset within `line` of the opening quote of its value.
fn value_offset(line: &str) -> Option<usize> {
    let equals = line.find('=')?;
    let quote = line[equals..].find('"')?;
    Some(equals + quote)
}

/// The catalog number a `[lints]` key names, or `None` for a family wildcard,
/// a named lint, or anything else that is not code-shaped. Mirrors
/// `surrealguard_workspace::config`'s own key parsing, which strips the
/// display-only leading letter.
fn entry_code_number(key: &str) -> Option<u16> {
    let key = key.trim().trim_matches('"').trim_matches('\'');
    let digits = key
        .strip_prefix(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(key);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u16>().ok()
}

/// Whether a `--` directive line can be inserted above `offset` in a host
/// file without breaking the string literal the query lives in.
///
/// Two conditions, both necessary:
///   * the literal is a **backtick** template. A single- or double-quoted JS
///     string cannot hold a newline at all, and a Svelte `q="…"` attribute
///     would be terminated by the `"` of a `reason="…"`. A backtick literal
///     is closed by nothing this module writes.
///   * the query already **began on an earlier line**, so column 0 of the
///     diagnostic's line is unambiguously inside the literal rather than in
///     the TypeScript around it.
///
/// Everything else — `db.query("…")` on one line, `<Query q="…" />` — gets
/// the workspace-wide action only. An action that silently corrupts a source
/// file is far worse than an action that is not offered.
pub fn host_inline_site(
    host_text: &str,
    queries: &[surrealguard_embed::EmbeddedQuery],
    offset: usize,
) -> bool {
    let Some(query) = queries
        .iter()
        .find(|query| query.host_range.contains(&offset))
    else {
        return false;
    };
    if query.host_range.start == 0 {
        return false;
    }
    if host_text.as_bytes()[query.host_range.start - 1] != b'`' {
        return false;
    }
    let line_start = host_text[..offset.min(host_text.len())]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    query.host_range.start < line_start
}

/// Applies a single-file edit set to text, for tests and round-trip checks:
/// edits are applied last-first so earlier offsets stay valid.
pub fn apply_edits(text: &str, edits: &[TextEdit]) -> String {
    let mut ordered: Vec<&TextEdit> = edits.iter().collect();
    ordered.sort_by_key(|edit| (edit.range.start.line, edit.range.start.character));
    let mut result = text.to_string();
    for edit in ordered.into_iter().rev() {
        let start = crate::text::position_to_offset(&result, edit.range.start);
        let end = crate::text::position_to_offset(&result, edit.range.end);
        result.replace_range(start..end, &edit.new_text);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::Position;

    #[test]
    fn rendered_codes_become_the_spelling_suppression_actually_matches() {
        // The letter an editor shows is the resolved severity; the letter a
        // directive must carry is the category.
        assert_eq!(suppressible_code("W7015").as_deref(), Some("L7015"));
        assert_eq!(suppressible_code("I7015").as_deref(), Some("L7015"));
        assert_eq!(suppressible_code("L7015").as_deref(), Some("L7015"));
        assert_eq!(suppressible_code("E1001").as_deref(), Some("E1001"));
        assert_eq!(suppressible_code("W1021").as_deref(), Some("E1021"));
        // Syntax codes have no catalog entry: not suppressible, and writing
        // one into [lints] would break the config outright.
        assert_eq!(suppressible_code("S0001"), None);
        assert_eq!(suppressible_code("E9999"), None);
        assert_eq!(suppressible_code("nonsense"), None);
    }

    #[test]
    fn the_directive_a_reasonless_workspace_gets_parses_and_names_the_code() {
        assert_eq!(directive("E1001", false), "-- surrealguard: allow(E1001)");
        assert_eq!(
            directive("E1001", true),
            "-- surrealguard: allow(E1001) reason=\"TODO: explain why this is allowed\""
        );
    }

    #[test]
    fn an_inline_edit_matches_the_indentation_of_the_line_it_covers() {
        let text = "BEGIN;\n    SELECT * FROM ghost;\nCOMMIT;\n";
        let offset = text.find("ghost").expect("fixture");
        let edit = inline_suppression_edit(text, offset, "E1001", false);
        assert_eq!(edit.range.start, Position::new(1, 0));
        assert_eq!(edit.range.end, Position::new(1, 0));
        assert_eq!(
            apply_edits(text, &[edit]),
            "BEGIN;\n    -- surrealguard: allow(E1001)\n    SELECT * FROM ghost;\nCOMMIT;\n"
        );
    }

    #[test]
    fn an_inline_edit_on_the_first_line_inserts_above_it() {
        let text = "SELECT * FROM ghost;\n";
        let offset = text.find("ghost").expect("fixture");
        let edit = inline_suppression_edit(text, offset, "E1001", true);
        assert_eq!(
            apply_edits(text, &[edit]),
            "-- surrealguard: allow(E1001) reason=\"TODO: explain why this is allowed\"\n\
             SELECT * FROM ghost;\n"
        );
    }

    #[test]
    fn a_config_without_a_lints_table_gains_one_at_the_end() {
        let toml = "# SurrealGuard workspace config.\n\n[analysis]\nstrict = false\n";
        let edit = lints_allow_edit(toml, "E1001").expect("not yet allowed");
        assert_eq!(
            apply_edits(toml, &[edit]),
            "# SurrealGuard workspace config.\n\n[analysis]\nstrict = false\n\n\
             [lints]\nE1001 = \"allow\"\n"
        );
    }

    #[test]
    fn a_config_with_no_trailing_newline_still_gets_a_well_formed_table() {
        let toml = "[analysis]\nstrict = false";
        let edit = lints_allow_edit(toml, "E1001").expect("not yet allowed");
        assert_eq!(
            apply_edits(toml, &[edit]),
            "[analysis]\nstrict = false\n\n[lints]\nE1001 = \"allow\"\n"
        );
    }

    #[test]
    fn an_existing_lints_table_keeps_every_comment_and_gains_one_line() {
        let toml = "\
# SurrealGuard workspace config for the SvelteKit example.

[sources]
# .svelte-kit holds generated route types; never scan it.
ignore = [\"node_modules/**\"]

[lints]
# `SELECT *` is fine in this app: the registry regenerates on every build.
select_star = \"allow\"
# Everything stylistic stays advisory.
\"7xxx\" = \"warn\"

[analysis]
strict = false
";
        let edit = lints_allow_edit(toml, "E1001").expect("not yet allowed");
        assert_eq!(
            apply_edits(toml, &[edit]),
            "\
# SurrealGuard workspace config for the SvelteKit example.

[sources]
# .svelte-kit holds generated route types; never scan it.
ignore = [\"node_modules/**\"]

[lints]
# `SELECT *` is fine in this app: the registry regenerates on every build.
select_star = \"allow\"
# Everything stylistic stays advisory.
\"7xxx\" = \"warn\"
E1001 = \"allow\"

[analysis]
strict = false
"
        );
    }

    #[test]
    fn a_lints_table_at_the_end_of_the_file_gains_the_entry_at_the_end() {
        let toml = "[analysis]\nstrict = false\n\n[lints]\nE1002 = \"warn\"\n";
        let edit = lints_allow_edit(toml, "E1001").expect("not yet allowed");
        assert_eq!(
            apply_edits(toml, &[edit]),
            "[analysis]\nstrict = false\n\n[lints]\nE1002 = \"warn\"\nE1001 = \"allow\"\n"
        );
    }

    #[test]
    fn a_code_already_allowed_offers_nothing_rather_than_a_duplicate_key() {
        let toml = "[lints]\nE1001 = \"allow\"\n";
        assert_eq!(lints_allow_edit(toml, "E1001"), None);
        // The display letter in the file is irrelevant: the number is the key.
        let toml = "[lints]\n1001 = \"allow\"\n";
        assert_eq!(lints_allow_edit(toml, "E1001"), None);
    }

    #[test]
    fn an_entry_at_another_level_is_relevelled_in_place_with_its_comment() {
        let toml = "[lints]\nW7002 = \"deny\"  # loud on purpose, for now\n";
        let edit = lints_allow_edit(toml, "L7002").expect("deny is not allow");
        assert_eq!(
            apply_edits(toml, &[edit]),
            "[lints]\nW7002 = \"allow\"  # loud on purpose, for now\n"
        );
    }

    #[test]
    fn a_family_wildcard_is_not_an_entry_for_the_code_it_covers() {
        // `"7xxx" = "warn"` covers 7002, but a specific code overrides its
        // family — which is exactly what suppressing one lint means.
        let toml = "[lints]\n\"7xxx\" = \"warn\"\n";
        let edit = lints_allow_edit(toml, "L7002").expect("wildcard is not an entry");
        assert_eq!(
            apply_edits(toml, &[edit]),
            "[lints]\n\"7xxx\" = \"warn\"\nL7002 = \"allow\"\n"
        );
    }

    #[test]
    fn a_nested_lints_table_is_not_mistaken_for_the_lints_table() {
        let toml = "[lints.nested]\nx = \"warn\"\n";
        let edit = lints_allow_edit(toml, "E1001").expect("no [lints] table here");
        assert_eq!(
            apply_edits(toml, &[edit]),
            "[lints.nested]\nx = \"warn\"\n\n[lints]\nE1001 = \"allow\"\n"
        );
    }

    #[test]
    fn only_a_multi_line_backtick_template_takes_an_inline_directive() {
        // Single-line double-quoted call: nowhere safe to put a comment line.
        let host = "const rows = await db.query(\"SELECT * FROM ghost\");\n";
        let queries = surrealguard_embed::extract("q.ts", host);
        let offset = host.find("ghost").expect("fixture");
        assert!(!host_inline_site(host, &queries, offset));

        // Svelte markup attribute, likewise.
        let host = "<Query q=\"SELECT * FROM ghost\" />\n";
        let queries = surrealguard_embed::extract("Page.svelte", host);
        let offset = host.find("ghost").expect("fixture");
        assert!(!host_inline_site(host, &queries, offset));

        // Multi-line template: the insertion point is inside the backticks.
        let host = "const rows = await db.query(`\n  SELECT * FROM ghost;\n`);\n";
        let queries = surrealguard_embed::extract("q.ts", host);
        let offset = host.find("ghost").expect("fixture");
        assert!(host_inline_site(host, &queries, offset));

        // A one-line backtick template is still unsafe: column 0 of that line
        // is TypeScript, not query text.
        let host = "const rows = await db.query(`SELECT * FROM ghost`);\n";
        let queries = surrealguard_embed::extract("q.ts", host);
        let offset = host.find("ghost").expect("fixture");
        assert!(!host_inline_site(host, &queries, offset));
    }
}
