//! Span fidelity for queries embedded in Svelte markup attributes.
//!
//! The point of extraction is not that a query is *found* — it is that a
//! finding computed on the extracted query renders at the token the user
//! wrote. A diagnostic reported one column off does not merely fail to help:
//! it puts a caret under innocent code, so the reader "fixes" something that
//! was never wrong. These tests therefore assert absolute line and column,
//! not round-trips, so a mapping that drifts fails loudly.
//!
//! The fixture is built to defeat the calculations that *look* right:
//!
//! * The attributes are not on line 1, and markup precedes them, so an
//!   offset taken relative to the attribute alone lands nowhere near.
//! * Earlier lines hold multi-byte characters (`—`, `“`, `”`), so treating a
//!   whole-file byte offset as a character position skews every later line.
//! * A multi-byte character (`Ü`, `Ä`) sits on the *same* line as, and
//!   before, the query attribute, so a byte-counted column is off by one
//!   even when the line is right.

use surrealguard_embed::{extract, EmbeddedQuery};

const FIXTURE: &str = include_str!("fixtures/attributes.svelte");
const FIXTURE_NAME: &str = "attributes.svelte";

/// One-based line and column for a byte offset, matching how the CLI
/// renderer locates a finding: lines split on `\n`, columns counted in
/// characters rather than bytes.
fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let clamped = offset.min(text.len());
    let line_start = text[..clamped].rfind('\n').map_or(0, |i| i + 1);
    let line = text[..line_start].matches('\n').count() + 1;
    let column = text[line_start..clamped].chars().count() + 1;
    (line, column)
}

/// Where the given token of an extracted query lands in the host file.
fn token_at(query: &EmbeddedQuery, token: &str) -> (usize, usize) {
    let embedded = query
        .text
        .find(token)
        .unwrap_or_else(|| panic!("{token:?} present in {:?}", query.text));
    let host = query.host_span(embedded..embedded + token.len());
    assert_eq!(
        &FIXTURE[host.clone()],
        token,
        "the mapped host bytes must be the token itself"
    );
    line_col(FIXTURE, host.start)
}

#[test]
fn markup_and_script_queries_are_all_found_in_document_order() {
    let queries = extract(FIXTURE_NAME, FIXTURE);

    let texts: Vec<&str> = queries.iter().map(|query| query.text.as_str()).collect();
    assert_eq!(
        texts,
        vec![
            // The shipped `<script>` path, still working, and still first.
            "SELECT * OMIT scriptghost FROM user",
            "SELECT * OMIT ghostfield FROM user",
            "SELECT * FROM user WHERE phantomfield > $__host0",
        ]
    );

    // Only `<LiveQuery>` runs its query as a live one.
    assert_eq!(
        queries.iter().map(|query| query.live).collect::<Vec<_>>(),
        vec![false, false, true]
    );
}

#[test]
fn a_script_query_still_reports_at_its_own_token() {
    // The regression guard: markup extraction must not disturb the path that
    // already shipped.
    let queries = extract(FIXTURE_NAME, FIXTURE);

    assert_eq!(token_at(&queries[0], "scriptghost"), (3, 40));
}

#[test]
fn a_plain_attribute_query_reports_at_its_own_token() {
    let queries = extract(FIXTURE_NAME, FIXTURE);

    // Line 10, column 40 — under `ghostfield` in
    // `  <Query title="Über" q="SELECT * OMIT ghostfield FROM user" />`.
    // Column 41 would mean the `Ü` was counted as its two UTF-8 bytes.
    assert_eq!(token_at(&queries[1], "ghostfield"), (10, 40));
}

#[test]
fn an_interpolated_attribute_query_reports_at_its_own_token() {
    let queries = extract(FIXTURE_NAME, FIXTURE);

    // The bad field sits *before* the hole, so this also proves the rewrite
    // to `$__host0` did not shift the text ahead of it.
    assert_eq!(token_at(&queries[2], "phantomfield"), (14, 58));
}

#[test]
fn a_finding_on_the_hole_covers_the_expression_the_user_wrote() {
    let queries = extract(FIXTURE_NAME, FIXTURE);
    let query = &queries[2];

    // A finding raised on the generated parameter must come back as the
    // whole `{minAge}`: someone reading it never typed `$__host0`, and a
    // caret under a lone `{` tells them nothing about which value is wrong.
    let param = query.text.find("$__host0").expect("parameter present");
    let host = query.host_span(param..param + "$__host0".len());

    assert_eq!(&FIXTURE[host.clone()], "{minAge}");
    assert_eq!(line_col(FIXTURE, host.start), (14, 73));
    assert_eq!(line_col(FIXTURE, host.end), (14, 81));

    // And the substitution records that same range, so a caller that wants
    // to name the expression can quote the user's own text back at them.
    assert_eq!(query.substitutions.len(), 1);
    assert_eq!(
        &FIXTURE[query.substitutions[0].host_range.clone()],
        "{minAge}"
    );
}

#[test]
fn every_copied_byte_of_a_markup_query_round_trips() {
    // Blanket cover for the two assertions above: each byte the analyzer can
    // see maps to a host byte holding the same character. An off-by-N
    // anywhere in the segment map fails here even if it misses the tokens
    // the other tests happen to name.
    for query in extract(FIXTURE_NAME, FIXTURE) {
        for (offset, expected) in query.text.char_indices() {
            let Some(embedded) = query.embed_offset(query.host_offset(offset)) else {
                continue;
            };
            if embedded != offset {
                continue;
            }
            let host = query.host_offset(offset);
            assert_eq!(
                FIXTURE[host..].chars().next(),
                Some(expected),
                "byte {offset} of {:?} maps to the wrong host character",
                query.text
            );
        }
    }
}
