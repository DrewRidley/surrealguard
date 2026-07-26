//! The both-ways test: the fact layer is never *wider* than the recognizers.
//!
//! Stage 3 of the expression-fact migration keeps two narrowing paths alive at
//! once — the hand-written recognizers in `flow/narrow.rs`, and the
//! `Guard`/`Atom`/`Facts` layer that replaces them — behind a runtime gate. The
//! precision snapshot pins each path against its own golden file, which proves
//! neither path *moved*; it says nothing about how the two relate.
//!
//! This file says how they relate, and it is the whole safety argument for the
//! stage:
//!
//! > For every site in the corpus, the kind the fact layer infers is **equal to
//! > or a subtype of** the kind the recognizers infer.
//!
//! *Never wider, sometimes narrower.* That is the mechanical statement of "no
//! precision was lost", and it is checkable rather than reviewable because the
//! subtype relation is `kinds::kind_is_assignable_to` — the same order
//! `lattice::meet` is built on.
//!
//! Two directions are deliberately asymmetric:
//!
//! * a site the old path could not type at all (`unknown`) may become typed —
//!   that is the improvement the migration exists for;
//! * a site the old path typed may **not** become `unknown`, and a kind may not
//!   grow a variant the old one did not have.
//!
//! The corpus is the vendored one every other harness reads
//! (`tests/corpus/**`), analyzed twice in one process with the gate flipped
//! between runs — the gate is a thread-local override for exactly this reason.

mod support;

use surrealdb_types::Kind;
use surrealguard_workspace::kinds::kind_is_assignable_to;
use surrealguard_workspace::with_fact_layer;

use support::{analyze_corpus, sites, Site};

/// Every corpus site's kind, under one setting of the gate.
fn kinds_under(fact_layer: bool) -> Vec<(String, Option<Kind>)> {
    with_fact_layer(fact_layer, || {
        let corpus = analyze_corpus();
        sites(&corpus)
            .into_iter()
            .map(|site: Site| (site.id, site.kind))
            .collect()
    })
}

#[test]
fn the_fact_layer_is_never_wider_than_the_recognizers() {
    let old = kinds_under(false);
    let new = kinds_under(true);

    // The two runs must describe the same corpus, or the comparison below is
    // comparing different programs.
    let old_ids: Vec<&String> = old.iter().map(|(id, _)| id).collect();
    let new_ids: Vec<&String> = new.iter().map(|(id, _)| id).collect();
    assert_eq!(
        old_ids, new_ids,
        "the two paths enumerated different sites — a narrowing changed the SHAPE of the analysis, \
         not just a kind"
    );

    let mut regressions = Vec::new();
    let mut improvements = 0usize;
    for ((id, old_kind), (_, new_kind)) in old.iter().zip(&new) {
        match (old_kind, new_kind) {
            // Unchanged.
            (a, b) if a == b => {}
            // The old path knew nothing; anything is an improvement.
            (None, Some(_)) => improvements += 1,
            // The old path knew something and the new one does not.
            (Some(old_kind), None) => regressions.push(format!(
                "{id}\n      was: {old_kind}\n      now: unknown  (LOST — the fact layer typed nothing)"
            )),
            (Some(old_kind), Some(new_kind)) => {
                if kind_is_assignable_to(new_kind, old_kind) {
                    improvements += 1;
                } else {
                    regressions.push(format!(
                        "{id}\n      was: {old_kind}\n      now: {new_kind}  (WIDER — not a subtype of the recorded kind)"
                    ));
                }
            }
            (None, None) => {}
        }
    }

    assert!(
        regressions.is_empty(),
        "\nPRECISION LOST — {} site(s) where the fact layer is not a subtype of the recognizer path:\n\n    {}\n\n\
         A refinement may narrow or stay put. Widening one is the failure this stage exists to rule out.\n",
        regressions.len(),
        regressions.join("\n    ")
    );

    // A vacuous pass is the one failure mode an inequality test has: if the
    // gate stopped selecting anything, every site would be trivially equal and
    // this file would prove nothing.
    assert!(
        improvements > 0,
        "the two paths inferred identical kinds at every site — the gate is not selecting the fact layer"
    );
}

/// The corpus is all-valid by construction, so a *new* finding on the fact
/// layer's path is a false positive it introduced. (The precision snapshot's
/// `corpus_is_free_of_error_findings` asserts this for whichever path it runs
/// under; this asserts the comparison across both in one process.)
#[test]
fn the_fact_layer_raises_no_finding_the_recognizers_did_not() {
    let codes = |fact_layer: bool| {
        with_fact_layer(fact_layer, || {
            let corpus = analyze_corpus();
            let mut codes: Vec<String> = corpus
                .analysis
                .sources
                .values()
                .flat_map(|output| output.diagnostics.iter())
                .map(|finding| {
                    format!(
                        "{} {:?}",
                        corpus.position(finding.span()),
                        surrealguard_diagnostics::render_code(finding.code(), finding.severity()),
                    )
                })
                .collect();
            codes.sort();
            codes
        })
    };
    let old = codes(false);
    let new: Vec<String> = codes(true);
    let added: Vec<&String> = new.iter().filter(|code| !old.contains(code)).collect();
    assert!(
        added.is_empty(),
        "the fact layer raised {} finding(s) the recognizer path did not, on an all-valid corpus: {:?}",
        added.len(),
        added
    );
}

/// The cases from the design's table that the vendored corpus does not carry —
/// each written as `(query, old answer, new answer)` so the flip is a committed
/// fact rather than a claim in a commit message.
///
/// The corpus covers the rest: the three table-discriminant spellings, the
/// three membership spellings, the De Morgan pairs, `!(x = NONE)`,
/// `type::is_none`, a bare truthiness guard on a param, and a literal-union
/// field compared to one of its members.
#[test]
fn the_designs_remaining_cases_flip() {
    const SCHEMA: &str = "\
        DEFINE TABLE user SCHEMAFULL;\n\
        DEFINE FIELD name ON user TYPE string;\n\
        DEFINE FIELD email ON user TYPE option<string>;\n\
        DEFINE FIELD age ON user TYPE option<int>;\n\
        DEFINE FIELD status ON user TYPE 'active' | 'inactive' | 'banned';\n\
        DEFINE FIELD note ON user TYPE option<string | null>;\n";

    // (case, query, kind under the recognizers, kind under the fact layer)
    let cases = [
        (
            // F13 — a bare field as a `WHERE` is a truthiness guard.
            "F13",
            "SELECT email FROM user WHERE email;",
            "array<{ email: option<string> }>",
            "array<{ email: string }>",
        ),
        (
            // F15 — a kind predicate over a row field.
            "F15",
            "SELECT email FROM user WHERE type::is_string(email);",
            "array<{ email: option<string> }>",
            "array<{ email: string }>",
        ),
        (
            // F19 — `!=` against one member of a literal union subtracts it.
            "F19",
            "SELECT status FROM user WHERE status != 'banned';",
            "array<{ status: 'active' | 'inactive' | 'banned' }>",
            "array<{ status: 'active' | 'inactive' }>",
        ),
        (
            // F11 — a kind predicate on a param, read through a field access
            // in the guarded branch.
            "F11",
            "LET $x = (SELECT name FROM ONLY user LIMIT 1);\n\
             LET $r = IF type::is_object($x) THEN $x.name ELSE 'x' END;",
            "any | string",
            "string",
        ),
        (
            // Not in the design's table: the fall-through past a multi-branch
            // diverging guard is reached only when EVERY condition failed, so
            // the negations are a conjunction. The recognizer path applied them
            // as a list, each resolving the field path from its DECLARED kind,
            // so the second overwrote the first and the `none` came back.
            "FallThrough",
            "LET $u = (SELECT note FROM ONLY user LIMIT 1);\n\
             IF $u = NONE THEN THROW 'no user' END;\n\
             IF $u.note = NONE THEN THROW 'unset' ELSE IF $u.note = NULL THEN THROW 'cleared' END;\n\
             LET $r = $u.note;",
            "option<string>",
            "string",
        ),
        (
            // Not in the design's table: an ordering guard narrowed the row
            // side and not the param side, because only the row recognizer had
            // one. One atom, both sides.
            "Ord",
            "LET $x = (SELECT VALUE age FROM ONLY user LIMIT 1);\n\
             LET $r = IF $x > 18 THEN $x ELSE 0 END;",
            "option<int>",
            "int",
        ),
    ];

    for (case, query, old, new) in cases {
        assert_eq!(
            (case, response_kind(SCHEMA, query, false).as_str()),
            (case, old),
            "{case}: the recognizer path"
        );
        assert_eq!(
            (case, response_kind(SCHEMA, query, true).as_str()),
            (case, new),
            "{case}: the fact layer"
        );
    }
}

/// The rendered kind of the last statement (or last `LET` binding) of `query`,
/// analyzed against `schema`.
fn response_kind(schema: &str, query: &str, fact_layer: bool) -> String {
    use surrealguard_workspace::{analyze_workspace, render_kind, Workspace};

    with_fact_layer(fact_layer, || {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source("schema".into(), schema.into());
        let source = workspace.add_virtual_source("query".into(), query.into());
        let analysis = analyze_workspace(&workspace);
        let output = analysis.sources.get(&source).expect("the query source");
        let kind = output
            .let_bindings
            .last()
            .and_then(|binding| binding.kind.clone())
            .or_else(|| {
                output
                    .statements
                    .iter()
                    .rev()
                    .find_map(|statement| statement.response_kind.clone())
            });
        kind.map_or_else(|| "unknown".to_string(), |kind| render_kind(&kind))
    })
}
