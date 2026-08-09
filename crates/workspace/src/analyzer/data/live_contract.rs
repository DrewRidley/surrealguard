//! What a SELECT may be, when the thing subscribing to it is a live query.
//!
//! `defineLive("SELECT * FROM user:1")` is a plain SELECT to everything that
//! reads it — it parses as one, it types as one, and analysis had nothing to
//! say about it. At runtime the client turns that string into `LIVE SELECT *
//! FROM user:1`, which SurrealDB accepts registering and which then **never
//! fires**. A subscription that silently produces nothing is the failure mode
//! this file exists to stop: there is no error to read, no row to be
//! surprised by, just a feature that does not work.
//!
//! LIVE SELECT is a much narrower statement than SELECT, and the boundary is
//! not documented anywhere we could take on trust, so it was established by
//! sending each form to a live 3.2.3 over `ws://` and reading what came back.
//! `db.query("LIVE SELECT …")` registers the subscription and answers with a
//! uuid; the rejections come back two different ways, which is itself part of
//! the picture:
//!
//! ```text
//! -- accepted, answering with a subscription uuid
//! LIVE SELECT * FROM ticket
//! LIVE SELECT title FROM ticket
//! LIVE SELECT title AS t FROM ticket
//! LIVE SELECT owner.name FROM ticket
//! LIVE SELECT ->owns->ticket AS x FROM person
//! LIVE SELECT VALUE title FROM ticket
//! LIVE SELECT DIFF FROM ticket
//! LIVE SELECT * FROM ticket WHERE title = 'x'
//! LIVE SELECT * FROM ticket FETCH owner
//! LIVE SELECT * FROM type::table('ticket')
//!
//! -- rejected while PARSING: "Unexpected token `X`, expected Eof"
//! LIVE SELECT * FROM ticket ORDER BY title
//! LIVE SELECT * FROM ticket GROUP BY title
//! LIVE SELECT * FROM ticket LIMIT 1
//! LIVE SELECT * FROM ticket START 1
//! LIVE SELECT * FROM ticket SPLIT title
//! LIVE SELECT * OMIT title FROM ticket
//! LIVE SELECT * FROM ticket TIMEOUT 1s
//! LIVE SELECT * FROM ticket PARALLEL
//! LIVE SELECT * FROM ticket EXPLAIN
//! LIVE SELECT * FROM ONLY ticket
//! LIVE SELECT * FROM ticket, person
//!
//! -- rejected while EXECUTING: "Cannot execute LIVE statement using value: X"
//! LIVE SELECT * FROM ticket:1
//! LIVE SELECT * FROM [ticket:1, ticket:2]
//! LIVE SELECT * FROM (SELECT * FROM ticket)
//! ```
//!
//! The shape underneath: a live query subscribes to a **table**, and every
//! clause it rejects is one that would need the whole result set to evaluate.
//! ORDER BY, GROUP BY, LIMIT, START and SPLIT are all set-shaping, and a
//! notification stream has no set to shape — each change arrives on its own.
//! WHERE and FETCH survive because both are per-row. A record id is rejected
//! for the same reason from the other side: a subscription is to a table's
//! changes, and a single row is not a table.
//!
//! Reported as 4009, whose contract — "LIVE SELECT with unsupported clause" —
//! is exactly this and which had no emission site before.
//!
//! Two rejected forms are deliberately NOT reported through the `defineLive`
//! path, because the SELECT AST does not carry them: `FROM [ticket:1,
//! ticket:2]` has no source node for an array literal (`stmt.from` comes back
//! empty), and `FROM (SELECT …)` reaches here as a subquery whose value is
//! only known at runtime. Both are engine rejections we can see no evidence
//! of, and a check written against a shape the AST never holds is a check
//! that silently never fires. Written as a real `LIVE SELECT` neither form
//! parses at all — the grammar takes only an ident, a record id or a param
//! after `FROM` — so both are already reported there, as syntax errors.
//!
//! # What the engine accepts and then does not honour
//!
//! Registering is not the contract. A notification is computed from the one
//! changed record, not by re-running the query, so a clause can parse, can
//! register, and still not reach the payload. That gap was measured the same
//! way as the rejections — by subscribing over `ws://` to 3.2.3, writing to
//! the table, and reading the raw notification frames. What the payload
//! showed, per clause:
//!
//! - `FETCH` **is** honoured, and resolves at notification time rather than
//!   at registration: `LIVE SELECT * FROM ticket FETCH owner` delivers
//!   `owner` as a full object on CREATE, UPDATE *and* DELETE, and picks up an
//!   edit made to the fetched record in between. A dangling link fetches as
//!   `null`. A `FETCH` naming a non-link or unknown field is silently a
//!   no-op.
//! - `WHERE` **is** honoured, including through a link path
//!   (`WHERE owner.name = 'x'`), on all three actions. Note for callers, not
//!   a defect: there is no "left the filter" notification — a row updated out
//!   of the filter simply goes quiet.
//! - Projections, aliases, computed expressions and `VALUE` **are** honoured,
//!   evaluated against the changed record.
//! - Graph traversals in a projection **are** honoured, with two edges worth
//!   knowing: they are re-evaluated only when the *subscribed* record
//!   changes, so writing an edge fires nothing at all, and on DELETE the
//!   traversal resolves against an already-deleted record and always comes
//!   back empty.
//! - `DIFF` **is** honoured, delivering a JSON-Patch-shaped array. Its
//!   `change` op carries a unified-diff *string* for edited text rather than
//!   the new value.
//!
//! and the two that are not honoured, reported as 4027:
//!
//! ```text
//! -- FETCH is silently ignored under DIFF. Same write, two subscriptions:
//! LIVE SELECT *    FROM ticket FETCH owner
//!    -> {"id":"ticket:a","owner":{"id":"person:bob","name":"bob"},...}
//! LIVE SELECT DIFF FROM ticket FETCH owner
//!    -> [{"op":"replace","path":"","value":{"owner":"person:bob",...}}]
//!
//! -- DIFF is the diff form only in leading position. Later in the list it
//! -- is an ordinary field path, and no table has a field called DIFF:
//! LIVE SELECT title, DIFF FROM ticket
//!    -> {"DIFF":null,"title":"t"}      (on every action, forever)
//! ```
//!
//! Both register, both deliver, and both quietly do something other than what
//! they say — which is why they are a warning rather than an error, and why
//! the message says what the payload will actually contain.

use surrealguard_syntax::ast;
use surrealguard_syntax::span::SourceSpan;

/// Checks a SELECT that will be run as a live query, appending 4009 for each
/// part of it a live query cannot have.
///
/// Every finding names the clause and says what it would do, because the fix
/// is never "delete this" — it is "do this on the client, over the rows the
/// subscription delivers". A `LIMIT` on a stream of changes was always going
/// to be client-side work; the point is to say so before the subscription
/// ships and silently returns nothing.
pub(crate) fn check_live_select(
    stmt: &ast::SelectStmt,
    source: &surrealguard_syntax::source::SourceId,
    out: &mut Vec<surrealguard_diagnostics::Finding>,
) {
    let mut emit = |span: surrealguard_syntax::span::ByteRange, message: String, help: &str| {
        out.push(
            surrealguard_diagnostics::catalog::finding(
                SourceSpan::new(source.clone(), span),
                4009,
                message,
            )
            .with_help(help.to_string()),
        );
    };

    // The set-shaping clauses. Each is a parse error on the engine, so the
    // query is not merely degraded — it never registers at all.
    if let Some(order) = &stmt.order {
        if let Some(key) = order.keys.first() {
            emit(
                key.expr.span,
                "a live query can't ORDER BY".to_string(),
                "a subscription delivers one change at a time, so there is no result set to sort — order the rows on the client",
            );
        }
    }
    if let Some(group) = &stmt.group {
        if let Some(key) = group.keys.first() {
            emit(
                key.span,
                "a live query can't GROUP BY".to_string(),
                "grouping needs the whole result set, which a change stream never has",
            );
        }
    }
    if let Some(limit) = &stmt.limit {
        emit(
            limit.span,
            "a live query can't take a LIMIT".to_string(),
            "a subscription runs until it is killed; there is no row count to cap",
        );
    }
    if let Some(start) = &stmt.start {
        emit(
            start.span,
            "a live query can't take a START".to_string(),
            "there is no result set to skip into — a subscription begins at the next change",
        );
    }
    for idiom in &stmt.split {
        emit(
            idiom.span,
            "a live query can't SPLIT".to_string(),
            "SPLIT fans one row out into several, which a per-change notification has no room for",
        );
    }
    for idiom in &stmt.omit {
        emit(
            idiom.span,
            "a live query can't OMIT fields".to_string(),
            "project the fields you want instead — a live query takes a projection but not an OMIT",
        );
    }

    if let Some(timeout) = &stmt.timeout {
        emit(
            timeout.span,
            "a live query can't take a TIMEOUT".to_string(),
            "TIMEOUT bounds one execution; a subscription runs until it is killed",
        );
    }
    if let Some(span) = stmt.parallel {
        emit(
            span,
            "a live query can't be PARALLEL".to_string(),
            "there is no result set to fan out across workers",
        );
    }
    if let Some(span) = stmt.explain {
        emit(
            span,
            "a live query can't be EXPLAINed".to_string(),
            "EXPLAIN describes one execution plan; a subscription has no single execution",
        );
    }
    // One subscription, one table. The engine stops at the comma.
    if stmt.from.len() > 1 {
        for from in &stmt.from[1..] {
            emit(
                from.span,
                "a live query subscribes to one table".to_string(),
                "register a separate live query per table",
            );
        }
    }

    // `ONLY` and a record-id source are the same mistake wearing two hats: a
    // subscription is to a table's changes, and one row is not a table. The
    // engine rejects `ONLY` while parsing and the record id while executing,
    // which is why this one is worth saying loudest — `defineLive("SELECT *
    // FROM user:1")` registers, returns a uuid, and never fires.
    if stmt.only {
        if let Some(from) = stmt.from.first() {
            emit(
                from.span,
                "a live query can't use ONLY".to_string(),
                "a live query subscribes to a table's changes, so it has no single row to reduce to",
            );
        }
    }
    // `FROM [ticket:1, ticket:2]` is rejected by the engine for the same
    // reason and is NOT reported: lowering has no source node for an array
    // literal, so `stmt.from` comes back empty and there is nothing here to
    // see. Recorded rather than guessed at — a check written against an AST
    // that never carries the shape is a check that silently never fires.
    for from in &stmt.from {
        if !matches!(from.node, ast::Expr::RecordId { .. }) {
            continue;
        }
        emit(
            from.span,
            "a live query can't subscribe to a record id".to_string(),
            "subscribe to the table and filter with WHERE — SurrealDB registers this and then never fires it",
        );
    }
}

/// Checks a real `LIVE SELECT` statement.
///
/// Everything the engine cannot parse — `ORDER BY`, `GROUP`, `LIMIT`,
/// `START`, `SPLIT`, `OMIT`, `TIMEOUT`, `PARALLEL`, `EXPLAIN`, `ONLY`, an
/// array or subquery source — the grammar cannot parse here either, so those
/// arrive as syntax errors and are not this function's business. What is left
/// is the two kinds of mistake that survive parsing: a source the engine
/// refuses once it runs the statement, and a clause it accepts and then does
/// not put in the notification.
pub(crate) fn check_live_select_statement(
    ctx: &mut crate::analyzer::context::AnalysisContext<'_>,
    stmt: &ast::LiveSelectStmt,
) {
    let source = ctx.source().clone();
    let mut findings = Vec::new();
    let mut emit =
        |span: surrealguard_syntax::span::ByteRange, code: u16, message: String, help: &str| {
            findings.push(
                surrealguard_diagnostics::catalog::finding(
                    SourceSpan::new(source.clone(), span),
                    code,
                    message,
                )
                .with_help(help.to_string()),
            );
        };

    // One subscription, one table — the engine stops at the comma while
    // parsing, so this never registers at all.
    if stmt.from.len() > 1 {
        for from in &stmt.from[1..] {
            emit(
                from.span,
                4009,
                "a live query subscribes to one table".to_string(),
                "register a separate live query per table",
            );
        }
    }

    // A record id registers, answers with a uuid, and then never fires: the
    // engine refuses it while executing, not while parsing, so there is no
    // error for the author to see anywhere.
    for from in &stmt.from {
        if matches!(from.node, ast::Expr::RecordId { .. }) {
            emit(
                from.span,
                4009,
                "a live query can't subscribe to a record id".to_string(),
                "subscribe to the table and filter with WHERE — SurrealDB registers this and then never fires it",
            );
        }
    }

    // `DIFF` and `FETCH` together: the engine takes both and the diff it
    // sends has the link unresolved, exactly as if the FETCH were not there.
    if stmt.diff.is_some() {
        for idiom in &stmt.fetch {
            emit(
                idiom.span,
                4027,
                "this FETCH does nothing — a DIFF notification is never fetched".to_string(),
                "notifications for this subscription carry a JSON-Patch array with the link left as a record id; drop DIFF to get fetched rows, or resolve the link on the client",
            );
        }
    }

    // `DIFF` is the diff form only in leading position. Later in the list the
    // parser has already committed to a projection list, so it reads as an
    // ordinary field path — and since no table has a field called `DIFF`,
    // every notification carries `"DIFF": null`.
    if stmt.diff.is_none() {
        for projection in &stmt.projections {
            let ast::Projection::Expr { expr, alias } = projection else {
                continue;
            };
            if alias.is_some() || !is_bare_diff_path(&expr.node) {
                continue;
            }
            emit(
                expr.span,
                4027,
                "this DIFF is read as a field name, not as the diff form".to_string(),
                "only `LIVE SELECT DIFF FROM ...` is the diff form; here it is an ordinary field path, so every notification will carry `DIFF: null`",
            );
        }
    }

    drop(emit);
    for finding in findings {
        ctx.emit(finding);
    }
}

/// Whether `expr` is the single unqualified path `DIFF`, in any casing —
/// the shape a `DIFF` that lost its leading position lowers to.
fn is_bare_diff_path(expr: &ast::Expr) -> bool {
    let ast::Expr::Idiom(idiom) = expr else {
        return false;
    };
    match idiom.parts.as_slice() {
        [only] => {
            matches!(&only.node, ast::IdiomPart::Field(name) if name.eq_ignore_ascii_case("diff"))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use crate::analysis::{analyze_workspace, Workspace};

    /// Analyzes `query` as a `defineLive` string would be, returning the 4009
    /// messages.
    fn live_findings(query: &str) -> Vec<String> {
        let mut workspace = Workspace::default();
        let schema = concat!(
            "DEFINE TABLE person SCHEMAFULL;\n",
            "DEFINE FIELD name ON person TYPE string;\n",
            "DEFINE TABLE ticket SCHEMAFULL;\n",
            "DEFINE FIELD owner ON ticket TYPE record<person>;\n",
            "DEFINE FIELD title ON ticket TYPE string;\n",
        );
        workspace.add_virtual_source("schema".into(), schema.into());
        let source = workspace.add_virtual_source("live".into(), query.into());
        workspace.mark_live_query(&source);
        analyze_workspace(&workspace)
            .sources
            .get(&source)
            .map(|output| {
                output
                    .diagnostics
                    .iter()
                    .filter(|finding| finding.code().number() == 4009)
                    .map(|finding| finding.message().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The forms 3.2.3 refuses, each verified over `ws://` — the parse-time
    /// rejections and the two that register and then never fire.
    #[test]
    fn a_live_query_reports_every_clause_the_engine_will_not_take() {
        for (query, expected) in [
            ("SELECT * FROM ticket ORDER BY title", "can't ORDER BY"),
            ("SELECT * FROM ticket GROUP BY title", "can't GROUP BY"),
            ("SELECT * FROM ticket LIMIT 1", "can't take a LIMIT"),
            ("SELECT * FROM ticket START 1", "can't take a START"),
            ("SELECT * FROM ticket SPLIT title", "can't SPLIT"),
            ("SELECT * OMIT title FROM ticket", "can't OMIT fields"),
            ("SELECT * FROM ticket TIMEOUT 1s", "can't take a TIMEOUT"),
            ("SELECT * FROM ticket PARALLEL", "can't be PARALLEL"),
            ("SELECT * FROM ONLY ticket", "can't use ONLY"),
            ("SELECT * FROM ticket:1", "can't subscribe to a record id"),
        ] {
            let found = live_findings(query);
            assert!(
                found.iter().any(|message| message.contains(expected)),
                "`{query}` should report {expected:?}, got {found:?}"
            );
        }
    }

    /// The other half, and the one that decides whether this check is usable:
    /// everything a live query really does take must stay silent. Every line
    /// here registered a subscription against 3.2.3 and answered with a uuid.
    #[test]
    fn a_live_query_accepts_everything_the_engine_accepts() {
        for query in [
            "SELECT * FROM ticket",
            "SELECT title FROM ticket",
            "SELECT title AS t FROM ticket",
            "SELECT owner.name FROM ticket",
            "SELECT VALUE title FROM ticket",
            "SELECT * FROM ticket WHERE title = 'x'",
            "SELECT * FROM ticket FETCH owner",
            "SELECT * FROM ticket WHERE title = 'x' FETCH owner",
        ] {
            let found = live_findings(query);
            assert!(found.is_empty(), "`{query}` should be clean, got {found:?}");
        }
    }

    /// The same schema, but for real `LIVE SELECT` statements — which reach
    /// analysis through the ordinary pipeline, not the `defineLive` post-pass,
    /// and so are not marked as live sources.
    fn statement_findings(query: &str) -> Vec<(u16, String)> {
        let mut workspace = Workspace::default();
        let schema = concat!(
            "DEFINE TABLE person SCHEMAFULL;\n",
            "DEFINE FIELD name ON person TYPE string;\n",
            "DEFINE TABLE ticket SCHEMAFULL;\n",
            "DEFINE FIELD owner ON ticket TYPE record<person>;\n",
            "DEFINE FIELD title ON ticket TYPE string;\n",
        );
        workspace.add_virtual_source("schema".into(), schema.into());
        let source = workspace.add_virtual_source("live".into(), query.into());
        analyze_workspace(&workspace)
            .sources
            .get(&source)
            .map(|output| {
                output
                    .diagnostics
                    .iter()
                    .map(|finding| (finding.code().number(), finding.message().to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The two forms that parse, register, and are then refused or ignored by
    /// the engine — the ones with no runtime symptom to debug from.
    #[test]
    fn a_live_statement_reports_the_sources_the_engine_will_not_subscribe_to() {
        for (query, code, expected) in [
            (
                "LIVE SELECT * FROM ticket:1;",
                4009,
                "can't subscribe to a record id",
            ),
            (
                "LIVE SELECT * FROM ticket, person;",
                4009,
                "subscribes to one table",
            ),
        ] {
            let found = statement_findings(query);
            assert!(
                found
                    .iter()
                    .any(|(number, message)| *number == code && message.contains(expected)),
                "`{query}` should report {code}/{expected:?}, got {found:?}"
            );
        }
    }

    /// 4027 — accepted by the engine, delivered, and provably not reflected in
    /// the notification. Both were read off raw ws:// frames from 3.2.3.
    #[test]
    fn a_live_statement_reports_the_clauses_the_notification_will_not_reflect() {
        let found = statement_findings("LIVE SELECT DIFF FROM ticket FETCH owner;");
        assert!(
            found
                .iter()
                .any(|(number, message)| *number == 4027 && message.contains("FETCH does nothing")),
            "a fetched DIFF should report 4027, got {found:?}"
        );

        let found = statement_findings("LIVE SELECT title, DIFF FROM ticket;");
        assert!(
            found.iter().any(
                |(number, message)| *number == 4027 && message.contains("read as a field name")
            ),
            "a trailing DIFF should report 4027, got {found:?}"
        );
    }

    /// The half that decides whether this is usable: every form the engine
    /// accepts *and* honours must stay silent. Each of these registered
    /// against 3.2.3 and delivered notifications carrying what it asked for.
    #[test]
    fn a_live_statement_accepts_everything_the_notification_really_honours() {
        for query in [
            "LIVE SELECT * FROM ticket;",
            "LIVE SELECT title FROM ticket;",
            "LIVE SELECT title AS t FROM ticket;",
            "LIVE SELECT owner.name AS who FROM ticket;",
            "LIVE SELECT string::uppercase(title) AS shout FROM ticket;",
            "LIVE SELECT VALUE title FROM ticket;",
            "LIVE SELECT DIFF FROM ticket;",
            "LIVE SELECT * FROM ticket WHERE title = 'x';",
            "LIVE SELECT * FROM ticket WHERE owner.name = 'x';",
            "LIVE SELECT * FROM ticket FETCH owner;",
            "LIVE SELECT * FROM ticket WHERE title = 'x' FETCH owner;",
        ] {
            let found = statement_findings(query);
            assert!(found.is_empty(), "`{query}` should be clean, got {found:?}");
        }
    }

    /// `DIFF` leading is the diff form and takes no alias; `DIFF` anywhere
    /// else is a field path. Only the second is 4027, and the flag that tells
    /// them apart comes from the grammar rather than from the spelling.
    #[test]
    fn only_a_non_leading_diff_is_read_as_a_field() {
        let leading = statement_findings("LIVE SELECT DIFF FROM ticket;");
        assert!(
            leading.is_empty(),
            "leading DIFF is the diff form: {leading:?}"
        );

        let trailing = statement_findings("LIVE SELECT title, diff FROM ticket;");
        assert!(
            trailing.iter().any(|(number, _)| *number == 4027),
            "a lowercase trailing `diff` is the same mistake: {trailing:?}"
        );
    }

    /// The contract belongs to the sink, not the SurrealQL: the same string is
    /// correct through `defineQuery` and wrong through `defineLive`. A source
    /// nobody marked must not be held to it.
    #[test]
    fn an_unmarked_source_is_not_held_to_the_live_contract() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE ticket SCHEMAFULL;\nDEFINE FIELD title ON ticket TYPE string;\n".into(),
        );
        let source = workspace.add_virtual_source(
            "plain".into(),
            "SELECT * FROM ticket ORDER BY title LIMIT 1;".into(),
        );
        let analysis = analyze_workspace(&workspace);
        let live: Vec<_> = analysis.sources[&source]
            .diagnostics
            .iter()
            .filter(|finding| finding.code().number() == 4009)
            .collect();
        assert!(live.is_empty(), "a plain query must not get 4009: {live:?}");
    }
}
