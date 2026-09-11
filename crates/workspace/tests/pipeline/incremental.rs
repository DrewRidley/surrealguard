//! Incremental re-analysis equivalence: a single source re-analyzed against
//! a prebuilt `GlobalCatalog` must match the whole-workspace pass, and the
//! symbol-level affected-set computation must re-analyze exactly the sources
//! whose facts changed.

use surrealql_analyzer_syntax::parse::parse_source;
use surrealql_analyzer_workspace::{
    analyze_one_source, analyze_workspace, build_global_catalog, Workspace,
};

// ---- Incremental single-source analysis equivalence ----
//
// The whole risk of the incremental path is a stale/wrong result. These
// tests prove that, for a source that contributes nothing to the global
// catalog (a pure query source), re-analyzing it in isolation against a
// prebuilt `GlobalCatalog` is byte-identical to what the full
// `analyze_workspace` pass produces for that source.

/// Parses every registered source in a workspace, in registration order,
/// so the parsed set feeds `build_global_catalog` with the same source ids
/// the full pass uses.
fn parsed_sources_of(workspace: &Workspace) -> Vec<surrealql_analyzer_syntax::parse::ParsedSource> {
    workspace
        .registry()
        .source_ids()
        .filter_map(|id| {
            let text = workspace.registry().text(id)?;
            parse_source(id.clone(), text).ok()
        })
        .collect()
}

/// Asserts the incremental output for `target` equals the full-pass output
/// for `target` on every dimension the task pins.
fn assert_incremental_matches_full(schema_sources: &[&str], query_text: &str) {
    let mut workspace = Workspace::default();
    for (i, schema) in schema_sources.iter().enumerate() {
        workspace.add_virtual_source(format!("schema{i}"), (*schema).into());
    }
    let target = workspace.add_virtual_source("query".into(), query_text.into());

    let full = analyze_workspace(&workspace);
    let full_target = full
        .sources
        .get(&target)
        .expect("target analyzed in full pass")
        .clone();

    // Build the catalog from ONLY the schema sources — the incremental path
    // never re-parses the query into the catalog. This mirrors caching the
    // catalog across a query-file edit.
    let parsed = parsed_sources_of(&workspace);
    let catalog = build_global_catalog(&parsed);
    let parsed_target = parsed
        .iter()
        .find(|p| p.source_id() == &target)
        .expect("target parsed");

    let incremental = analyze_one_source(&catalog, parsed_target, false);

    assert_eq!(
        incremental.diagnostics, full_target.diagnostics,
        "diagnostics diverged for query `{query_text}`"
    );
    assert_eq!(
        incremental.response_kind, full_target.response_kind,
        "response_kind diverged for query `{query_text}`"
    );
    assert_eq!(
        incremental.let_bindings, full_target.let_bindings,
        "let_bindings diverged for query `{query_text}`"
    );
    assert_eq!(
        incremental.statements, full_target.statements,
        "statements diverged for query `{query_text}`"
    );
    assert_eq!(
        incremental.inferred_params, full_target.inferred_params,
        "inferred_params diverged for query `{query_text}`"
    );
}

#[test]
fn incremental_query_analysis_matches_full_across_a_battery() {
    let schema = &[
        "DEFINE TABLE person SCHEMAFULL;\n\
         DEFINE FIELD name ON person TYPE string;\n\
         DEFINE FIELD age ON person TYPE int;\n\
         DEFINE FIELD email ON person TYPE option<string>;",
        "DEFINE TABLE post SCHEMAFULL;\n\
         DEFINE FIELD title ON post TYPE string;\n\
         DEFINE FIELD author ON post TYPE record<person>;\n\
         DEFINE FUNCTION fn::adult($p: record<person>) { RETURN (SELECT VALUE age FROM ONLY $p) >= 18; };",
    ];

    // A representative battery of pure-query sources: clean, diagnostic-
    // producing, param-bearing, LET-binding, cross-source-UDF, and
    // unknown-table.
    for query in [
        "SELECT name, age FROM person;",
        "SELECT * FROM person WHERE age > $min;",
        "LET $p = person:one;\nSELECT name FROM $p;",
        "SELECT title, author.name FROM post;",
        "RETURN fn::adult(person:one);",
        "SELECT * FROM ghost;",
        "SELECT name FROM person WHERE email != NONE;",
        "UPDATE person SET name = 'Ada' WHERE age < $max RETURN name;",
        "SELECT math::sum(age) AS total FROM person GROUP BY name;",
        "LET $unused = 1;\nRETURN 5;",
    ] {
        assert_incremental_matches_full(schema, query);
    }
}

#[test]
fn incremental_query_analysis_matches_full_with_no_schema() {
    // No schema sources at all: the catalog is empty, and unknown-table and
    // opt-in lints must still match exactly.
    for query in [
        "SELECT * FROM person;",
        "RETURN 1 + 2;",
        "LET $x = 'a';\nRETURN $x;",
    ] {
        assert_incremental_matches_full(&[], query);
    }
}

/// Measurement hook (ignored by default): times a full whole-workspace
/// pass against a single-source incremental re-analysis on a ~20-file
/// corpus. Run with:
///   cargo test -p surrealql-analyzer-workspace `incremental_reanalysis_speedup` -- --ignored --nocapture
#[test]
#[ignore]
fn incremental_reanalysis_speedup() {
    use std::time::Instant;

    let mut workspace = Workspace::default();
    // Four schema files.
    for f in 0..4 {
        let mut schema = String::new();
        for t in 0..5 {
            schema.push_str(&format!(
                "DEFINE TABLE t{f}_{t} SCHEMAFULL;\n\
                 DEFINE FIELD name ON t{f}_{t} TYPE string;\n\
                 DEFINE FIELD n ON t{f}_{t} TYPE int;\n\
                 DEFINE FIELD tags ON t{f}_{t} TYPE array<string>;\n\
                 DEFINE FUNCTION fn::f{f}_{t}($x: record<t{f}_{t}>) {{ RETURN (SELECT VALUE n FROM ONLY $x); }};\n"
            ));
        }
        workspace.add_virtual_source(format!("schema{f}"), schema);
    }
    // Sixteen query files; keep a handle on one to re-analyze.
    let mut target = None;
    for q in 0..16 {
        let id = workspace.add_virtual_source(
            format!("query{q}"),
            "SELECT name, n, tags FROM t0_0 WHERE n > $min;\n\
                 RETURN fn::f1_1(t1_1:one);\n\
                 LET $rows = SELECT name FROM t2_2;\n\
                 UPDATE t3_3 SET name = 'x' WHERE n < $max RETURN name;"
                .to_string(),
        );
        if q == 8 {
            target = Some(id);
        }
    }
    let target = target.unwrap();

    let iters = 200;

    // Full whole-workspace pass per "keystroke".
    let full_start = Instant::now();
    for _ in 0..iters {
        let _ = analyze_workspace(&workspace);
    }
    let full = full_start.elapsed() / iters;

    // Incremental: catalog cached, only the dirty source re-analyzed.
    let parsed = parsed_sources_of(&workspace);
    let catalog = build_global_catalog(&parsed);
    let parsed_target = parsed
        .iter()
        .find(|p| p.source_id() == &target)
        .expect("target parsed");
    let incr_start = Instant::now();
    for _ in 0..iters {
        let _ = analyze_one_source(&catalog, parsed_target, false);
    }
    let incr = incr_start.elapsed() / iters;

    // Catalog build cost (paid once per schema change, amortized across
    // every subsequent query edit).
    let cat_start = Instant::now();
    for _ in 0..iters {
        let _ = build_global_catalog(&parsed);
    }
    let cat = cat_start.elapsed() / iters;

    eprintln!(
        "20-file corpus: full={full:?}  incremental={incr:?}  catalog_build={cat:?}  speedup={:.1}x",
        full.as_secs_f64() / incr.as_secs_f64().max(f64::MIN_POSITIVE)
    );
}

#[test]
fn incremental_catalog_is_reused_across_successive_query_edits() {
    // Prove the catalog built once from the schema is reusable across a
    // sequence of query-file edits: each edit's incremental output matches
    // a fresh full pass with that query. This is exactly the LSP fast path.
    let schema = "DEFINE TABLE person SCHEMAFULL;\n\
                  DEFINE FIELD name ON person TYPE string;\n\
                  DEFINE FIELD age ON person TYPE int;";

    // Build the catalog once (schema only).
    let mut schema_ws = Workspace::default();
    schema_ws.add_virtual_source("schema".into(), schema.into());
    let schema_parsed = parsed_sources_of(&schema_ws);
    let catalog = build_global_catalog(&schema_parsed);

    for query in [
        "SELECT name FROM person;",
        "SELECT age FROM person WHERE age > 18;",
        "SELECT * FROM person;",
        "SELECT missing FROM person;",
    ] {
        // Full reference: a workspace whose schema source is registered
        // first (same id ordering as the catalog build above), then query.
        let mut full_ws = Workspace::default();
        full_ws.add_virtual_source("schema".into(), schema.into());
        let target = full_ws.add_virtual_source("query".into(), query.into());
        let full = analyze_workspace(&full_ws);
        let full_target = full.sources.get(&target).expect("target").clone();

        // Incremental: parse only the query, reusing the cached catalog. The
        // query's source id must match `target` for spans to line up — which
        // they do because both workspaces register schema-then-query.
        let parsed_query = parse_source(target.clone(), query).expect("query parses");
        let incremental = analyze_one_source(&catalog, &parsed_query, false);

        assert_eq!(
            incremental.diagnostics, full_target.diagnostics,
            "query `{query}`"
        );
        assert_eq!(
            incremental.response_kind, full_target.response_kind,
            "query `{query}`"
        );
        assert_eq!(
            incremental.let_bindings, full_target.let_bindings,
            "query `{query}`"
        );
    }
}

mod symbol_incremental_tests {
    //! Equivalence + granularity coverage for symbol-level incremental
    //! re-analysis. The invariant under test: merging the previous per-source
    //! outputs with a fresh re-analysis of only the AFFECTED sources must equal
    //! a from-scratch `analyze_workspace` at the post-edit state, for EVERY
    //! source. A stale (unaffected-but-should-have-changed) source fails the
    //! merge check, which is exactly the correctness bug we must never ship.

    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;

    use surrealql_analyzer_syntax::parse::{parse_source, ParsedSource};
    use surrealql_analyzer_syntax::source::SourceId;
    use surrealql_analyzer_workspace::analysis::{
        build_workspace_schema, changed_symbols, reanalyze_sources, source_reference_set,
        source_requires_full_reanalysis, sources_with_changed_cycle_findings,
    };
    use surrealql_analyzer_workspace::{
        analyze_workspace, build_global_catalog, AnalysisOutput, Workspace, WorkspaceAnalysis,
    };

    fn sid(name: &str) -> SourceId {
        SourceId::new(format!("file:///{name}"))
    }

    fn parsed_for(sources: &[(&str, &str)]) -> Vec<ParsedSource> {
        sources
            .iter()
            .map(|(name, text)| parse_source(sid(name), *text).expect("test sources parse"))
            .collect()
    }

    /// Ground truth: a from-scratch whole-workspace pass over `sources`, using
    /// file ids so they align with the incremental path.
    fn full_workspace(sources: &[(&str, &str)]) -> WorkspaceAnalysis {
        let mut workspace = Workspace::default();
        for (name, text) in sources {
            workspace.add_file_source(PathBuf::from(format!("/{name}")), (*text).to_string());
        }
        analyze_workspace(&workspace)
    }

    /// Computes the affected set for an edit exactly as the LSP does, then
    /// returns `(affected_ids, merged_outputs)` where merged = the before-state
    /// outputs with the affected sources replaced by a fresh re-analysis.
    fn incremental(
        before: &[(&str, &str)],
        after: &[(&str, &str)],
    ) -> (BTreeSet<SourceId>, BTreeMap<SourceId, AnalysisOutput>) {
        assert_eq!(
            before.len(),
            after.len(),
            "harness expects a same-doc-set edit"
        );
        let before_parsed = parsed_for(before);
        let after_parsed = parsed_for(after);
        let before_catalog = build_global_catalog(&before_parsed);
        let after_catalog = build_global_catalog(&after_parsed);

        let dirty: BTreeSet<SourceId> = before
            .iter()
            .zip(after)
            .filter(|((_, bt), (_, at))| bt != at)
            .map(|((name, _), _)| sid(name))
            .collect();
        // The harness only covers edits the symbol path actually takes; an
        // unmodeled dirty effect would force the (trivially equivalent) full
        // pass in the LSP.
        for parsed in &after_parsed {
            if dirty.contains(parsed.source_id()) {
                assert!(
                    !source_requires_full_reanalysis(parsed),
                    "dirty source {} carries an unmodeled effect; test the full-fallback instead",
                    parsed.source_id()
                );
            }
        }

        let changed = changed_symbols(&before_catalog, &after_catalog);
        let mut affected = dirty.clone();
        for parsed in &after_parsed {
            if source_reference_set(parsed).is_affected_by(&changed) {
                affected.insert(parsed.source_id().clone());
            }
        }
        for id in sources_with_changed_cycle_findings(&before_catalog, &after_catalog) {
            affected.insert(id);
        }

        let reanalyzed = reanalyze_sources(&after_parsed, &after_catalog, &affected, false);
        let mut merged: BTreeMap<SourceId, AnalysisOutput> = full_workspace(before).sources;
        for (id, output) in &reanalyzed {
            merged.insert(id.clone(), output.clone());
        }
        (affected, merged)
    }

    /// Asserts the merged incremental result equals a full pass at `after` for
    /// every source (diagnostics, response kind, statements, params, let
    /// bindings) plus the rebuilt schema. Returns the affected-source count so
    /// callers can also assert granularity.
    fn assert_equivalent(before: &[(&str, &str)], after: &[(&str, &str)]) -> usize {
        let (affected, merged) = incremental(before, after);
        let after_full = full_workspace(after);

        for (id, expected) in &after_full.sources {
            let got = merged
                .get(id)
                .unwrap_or_else(|| panic!("merged result missing source {id}"));
            assert_eq!(
                got.diagnostics, expected.diagnostics,
                "diagnostics for {id}"
            );
            assert_eq!(
                got.response_kind, expected.response_kind,
                "response_kind for {id}"
            );
            assert_eq!(got.statements, expected.statements, "statements for {id}");
            assert_eq!(
                got.inferred_params, expected.inferred_params,
                "inferred_params for {id}"
            );
            assert_eq!(
                got.let_bindings, expected.let_bindings,
                "let_bindings for {id}"
            );
        }

        // The incrementally-rebuilt schema must match a full pass' schema so
        // editor features resolve against the current catalog.
        let after_parsed = parsed_for(after);
        assert_eq!(
            build_workspace_schema(&after_parsed),
            after_full.schema,
            "rebuilt schema must equal the full-pass schema"
        );

        affected.len()
    }

    const SCHEMA: &str = "DEFINE TABLE person SCHEMAFULL;\n\
         DEFINE FIELD name ON person TYPE string;\n\
         DEFINE FIELD age ON person TYPE int;";

    // (a) A query-file edit.
    #[test]
    fn query_edit_is_equivalent() {
        let before = &[
            ("schema.surql", SCHEMA),
            ("q.surql", "SELECT name FROM person;"),
        ];
        let after = &[
            ("schema.surql", SCHEMA),
            ("q.surql", "SELECT name, age FROM person WHERE age > 18;"),
        ];
        assert_equivalent(before, after);
    }

    // (b) A function-body edit that CHANGES its inferred return, read by a caller.
    #[test]
    fn function_body_edit_changing_return_reanalyzes_callers() {
        let before = &[
            ("fns.surql", "DEFINE FUNCTION fn::pick() { RETURN 1; };"),
            ("caller.surql", "RETURN fn::pick();"),
        ];
        let after = &[
            (
                "fns.surql",
                "DEFINE FUNCTION fn::pick() { RETURN 'text'; };",
            ),
            ("caller.surql", "RETURN fn::pick();"),
        ];
        let (affected, _) = incremental(before, after);
        assert!(
            affected.contains(&sid("caller.surql")),
            "the caller must re-analyze when the callee's return type changes"
        );
        assert_equivalent(before, after);
    }

    // (c) A function-body edit that does NOT change its signature.
    #[test]
    fn function_body_edit_preserving_signature_is_equivalent() {
        let before = &[
            (
                "fns.surql",
                "DEFINE FUNCTION fn::pick() -> int { RETURN 1; };",
            ),
            ("caller.surql", "RETURN fn::pick();"),
        ];
        let after = &[
            (
                "fns.surql",
                "DEFINE FUNCTION fn::pick() -> int { RETURN 1 + 1; };",
            ),
            ("caller.surql", "RETURN fn::pick();"),
        ];
        assert_equivalent(before, after);
    }

    // (d) A field TYPE change.
    #[test]
    fn field_type_change_is_equivalent() {
        let before = &[
            (
                "schema.surql",
                "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD age ON person TYPE int;",
            ),
            ("q.surql", "SELECT age FROM person;"),
        ];
        let after = &[
            (
                "schema.surql",
                "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD age ON person TYPE string;",
            ),
            ("q.surql", "SELECT age FROM person;"),
        ];
        let (affected, _) = incremental(before, after);
        assert!(
            affected.contains(&sid("q.surql")),
            "a reader of the changed field must re-analyze"
        );
        assert_equivalent(before, after);
    }

    // (e) Editing a function OTHER sources call: every caller re-analyzes.
    #[test]
    fn editing_a_called_function_reanalyzes_every_caller() {
        let before = &[
            ("fns.surql", "DEFINE FUNCTION fn::v() { RETURN 1; };"),
            ("a.surql", "RETURN fn::v();"),
            ("b.surql", "RETURN fn::v() + 1;"),
            ("c.surql", "RETURN 42;"),
        ];
        let after = &[
            ("fns.surql", "DEFINE FUNCTION fn::v() { RETURN 'x'; };"),
            ("a.surql", "RETURN fn::v();"),
            ("b.surql", "RETURN fn::v() + 1;"),
            ("c.surql", "RETURN 42;"),
        ];
        let (affected, _) = incremental(before, after);
        assert!(affected.contains(&sid("a.surql")));
        assert!(affected.contains(&sid("b.surql")));
        assert!(
            !affected.contains(&sid("c.surql")),
            "a source that never calls the function must NOT re-analyze"
        );
        assert_equivalent(before, after);
    }

    // (f) A definition removed from a file (no REMOVE statement — the DEFINE
    // text is deleted): the catalog loses the table, and its readers must see
    // the new unknown-table finding.
    #[test]
    fn removing_a_definition_reanalyzes_readers() {
        let before = &[
            ("schema.surql", "DEFINE TABLE ghost;\nDEFINE TABLE person;"),
            ("q.surql", "SELECT * FROM ghost;"),
        ];
        let after = &[
            ("schema.surql", "DEFINE TABLE person;"),
            ("q.surql", "SELECT * FROM ghost;"),
        ];
        let (affected, _) = incremental(before, after);
        assert!(affected.contains(&sid("q.surql")));
        assert_equivalent(before, after);
    }

    // (f') Adding a definition a reader depends on.
    #[test]
    fn adding_a_definition_reanalyzes_readers() {
        let before = &[
            ("schema.surql", "DEFINE TABLE person;"),
            ("q.surql", "SELECT * FROM ghost;"),
        ];
        let after = &[
            ("schema.surql", "DEFINE TABLE person;\nDEFINE TABLE ghost;"),
            ("q.surql", "SELECT * FROM ghost;"),
        ];
        assert_equivalent(before, after);
    }

    // A REMOVE statement in a dirty doc forces the full fallback (unmodeled).
    #[test]
    fn remove_statement_requires_full_reanalysis() {
        let parsed = parse_source(sid("s.surql"), "REMOVE TABLE person;").expect("parse");
        assert!(source_requires_full_reanalysis(&parsed));
        let alter = parse_source(sid("s.surql"), "ALTER TABLE person DROP;").expect("parse");
        assert!(source_requires_full_reanalysis(&alter));
        let define_param = parse_source(sid("s.surql"), "DEFINE PARAM $x VALUE 1;").expect("parse");
        assert!(source_requires_full_reanalysis(&define_param));
        let plain = parse_source(sid("s.surql"), "DEFINE TABLE person;").expect("parse");
        assert!(!source_requires_full_reanalysis(&plain));
    }

    // (g) An edit to a source many others reference: all readers re-analyze and
    // the result stays equivalent.
    #[test]
    fn editing_a_widely_referenced_table_reanalyzes_all_readers() {
        let before = &[
            (
                "schema.surql",
                "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;",
            ),
            ("q1.surql", "SELECT name FROM person;"),
            ("q2.surql", "SELECT name FROM person WHERE name != NONE;"),
            ("q3.surql", "CREATE person SET name = 'a';"),
            ("unrelated.surql", "RETURN 1 + 1;"),
        ];
        let after = &[
            (
                "schema.surql",
                "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE int;",
            ),
            ("q1.surql", "SELECT name FROM person;"),
            ("q2.surql", "SELECT name FROM person WHERE name != NONE;"),
            ("q3.surql", "CREATE person SET name = 'a';"),
            ("unrelated.surql", "RETURN 1 + 1;"),
        ];
        let (affected, _) = incremental(before, after);
        assert!(!affected.contains(&sid("unrelated.surql")));
        assert_equivalent(before, after);
    }

    // Mutual recursion: editing one function to close a cycle must flip the
    // 5009 findings of BOTH functions' sources, even though only one is dirty.
    #[test]
    fn closing_a_recursion_cycle_reanalyzes_the_other_function() {
        let before = &[
            ("a.surql", "DEFINE FUNCTION fn::a() { RETURN fn::b(); };"),
            ("b.surql", "DEFINE FUNCTION fn::b() { RETURN 1; };"),
        ];
        let after = &[
            ("a.surql", "DEFINE FUNCTION fn::a() { RETURN fn::b(); };"),
            ("b.surql", "DEFINE FUNCTION fn::b() { RETURN fn::a(); };"),
        ];
        let (affected, _) = incremental(before, after);
        assert!(
            affected.contains(&sid("a.surql")),
            "the other function in the newly-formed cycle must re-analyze for 5009"
        );
        assert_equivalent(before, after);
    }

    /// Measurement hook (ignored by default): on a ~200-file synthetic corpus,
    /// times a full whole-workspace pass against the symbol-incremental schema
    /// path for a function-body edit, and reports how many sources re-analyze.
    /// Run with:
    ///   cargo test -p surrealql-analyzer-workspace `symbol_incremental_schema_speedup` -- --ignored --nocapture
    #[test]
    #[ignore]
    fn symbol_incremental_schema_speedup() {
        use std::time::Instant;

        // 20 schema files (100 tables, 20 helper functions) + 180 query files.
        let mut before: Vec<(String, String)> = Vec::new();
        for f in 0..20 {
            let mut schema = String::new();
            for t in 0..5 {
                schema.push_str(&format!(
                    "DEFINE TABLE t{f}_{t} SCHEMAFULL;\n\
                     DEFINE FIELD name ON t{f}_{t} TYPE string;\n\
                     DEFINE FIELD n ON t{f}_{t} TYPE int;\n\
                     DEFINE FUNCTION fn::f{f}_{t}($x: record<t{f}_{t}>) {{ RETURN (SELECT VALUE n FROM ONLY $x); }};\n"
                ));
            }
            before.push((format!("schema{f}.surql"), schema));
        }
        for q in 0..180 {
            let f = q % 20;
            let t = q % 5;
            before.push((
                format!("query{q}.surql"),
                format!(
                    "SELECT name, n FROM t{f}_{t} WHERE n > $min;\n\
                     RETURN fn::f{f}_{t}(t{f}_{t}:one);"
                ),
            ));
        }
        // Edit ONE helper's body (add an arithmetic op). Nothing outside its own
        // file changes shape, but its callers read its return, so they re-run.
        let mut after = before.clone();
        after[0].1 = after[0].1.replacen(
            "DEFINE FUNCTION fn::f0_0($x: record<t0_0>) { RETURN (SELECT VALUE n FROM ONLY $x); };",
            "DEFINE FUNCTION fn::f0_0($x: record<t0_0>) { RETURN (SELECT VALUE n FROM ONLY $x) + 1; };",
            1,
        );

        let before_refs: Vec<(&str, &str)> = before
            .iter()
            .map(|(n, t)| (n.as_str(), t.as_str()))
            .collect();
        let after_refs: Vec<(&str, &str)> = after
            .iter()
            .map(|(n, t)| (n.as_str(), t.as_str()))
            .collect();

        let iters = 20;
        // Full whole-workspace pass per keystroke.
        let full_start = Instant::now();
        for _ in 0..iters {
            let _ = full_workspace(&after_refs);
        }
        let full = full_start.elapsed() / iters;

        // Symbol-incremental path per keystroke: rebuild the catalog (O(defines)),
        // diff it, and re-analyze only the affected sources.
        let before_parsed = parsed_for(&before_refs);
        let before_catalog = build_global_catalog(&before_parsed);
        let mut affected_count = 0usize;
        let incr_start = Instant::now();
        for _ in 0..iters {
            let after_parsed = parsed_for(&after_refs);
            let after_catalog = build_global_catalog(&after_parsed);
            let changed = changed_symbols(&before_catalog, &after_catalog);
            let mut affected: BTreeSet<SourceId> = [sid("schema0.surql")].into_iter().collect();
            for parsed in &after_parsed {
                if source_reference_set(parsed).is_affected_by(&changed) {
                    affected.insert(parsed.source_id().clone());
                }
            }
            for id in sources_with_changed_cycle_findings(&before_catalog, &after_catalog) {
                affected.insert(id);
            }
            affected_count = affected.len();
            let _ = reanalyze_sources(&after_parsed, &after_catalog, &affected, false);
        }
        let incr = incr_start.elapsed() / iters;

        eprintln!(
            "200-file corpus [body edit, return type unchanged]: sources={}  full={full:?}  symbol_incremental={incr:?}  reanalyzed={affected_count}  speedup={:.1}x",
            before.len(),
            full.as_secs_f64() / incr.as_secs_f64().max(f64::MIN_POSITIVE)
        );

        // Scenario B: the edit CHANGES the helper's return type (int -> string),
        // so every source that calls it re-analyzes — still far fewer than N.
        let mut after_b = before.clone();
        after_b[0].1 = after_b[0].1.replacen(
            "DEFINE FUNCTION fn::f0_0($x: record<t0_0>) { RETURN (SELECT VALUE n FROM ONLY $x); };",
            "DEFINE FUNCTION fn::f0_0($x: record<t0_0>) { RETURN 'label'; };",
            1,
        );
        let after_b_refs: Vec<(&str, &str)> = after_b
            .iter()
            .map(|(n, t)| (n.as_str(), t.as_str()))
            .collect();
        let mut affected_b = 0usize;
        let incr_b_start = Instant::now();
        for _ in 0..iters {
            let after_parsed = parsed_for(&after_b_refs);
            let after_catalog = build_global_catalog(&after_parsed);
            let changed = changed_symbols(&before_catalog, &after_catalog);
            let mut affected: BTreeSet<SourceId> = [sid("schema0.surql")].into_iter().collect();
            for parsed in &after_parsed {
                if source_reference_set(parsed).is_affected_by(&changed) {
                    affected.insert(parsed.source_id().clone());
                }
            }
            for id in sources_with_changed_cycle_findings(&before_catalog, &after_catalog) {
                affected.insert(id);
            }
            affected_b = affected.len();
            let _ = reanalyze_sources(&after_parsed, &after_catalog, &affected, false);
        }
        let incr_b = incr_b_start.elapsed() / iters;
        eprintln!(
            "200-file corpus [body edit, return type int->string]: full={full:?}  symbol_incremental={incr_b:?}  reanalyzed={affected_b}  speedup={:.1}x",
            full.as_secs_f64() / incr_b.as_secs_f64().max(f64::MIN_POSITIVE)
        );
    }

    // Granularity: a function-body edit whose function nothing references
    // re-analyzes ONLY the edited source, not the whole workspace.
    #[test]
    fn unreferenced_function_body_edit_reanalyzes_only_itself() {
        let mut before: Vec<(String, String)> = Vec::new();
        before.push((
            "helper.surql".to_string(),
            "DEFINE FUNCTION fn::helper() -> int { RETURN 1; };".to_string(),
        ));
        for i in 0..40 {
            before.push((format!("q{i}.surql"), format!("RETURN {i};")));
        }
        let before_refs: Vec<(&str, &str)> = before
            .iter()
            .map(|(n, t)| (n.as_str(), t.as_str()))
            .collect();

        let mut after = before.clone();
        after[0].1 = "DEFINE FUNCTION fn::helper() -> int { RETURN 2; };".to_string();
        let after_refs: Vec<(&str, &str)> = after
            .iter()
            .map(|(n, t)| (n.as_str(), t.as_str()))
            .collect();

        let affected = assert_equivalent(&before_refs, &after_refs);
        assert_eq!(
            affected,
            1,
            "an unreferenced function-body edit re-analyzes exactly one source, not {}",
            before.len()
        );
    }
}
