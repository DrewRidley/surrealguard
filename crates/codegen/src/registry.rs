//! The generated TypeScript module: it re-exports the runtime entry points
//! (`createClient`, `defineQuery`, `defineLive`, the SDK value classes) and
//! augments `SurqlRegistry` with one entry per analyzed query, keyed by its
//! exact text. Importing from this one file also loads the augmentation, so the
//! user never writes a side-effect import.
//!
//! A string literal passed to `defineQuery` / `defineLive` / `db.query`
//! resolves its result and parameter types from these entries; TypeScript can
//! key on the literal because it infers literal types for string arguments (but
//! never for tagged-template string arrays — the TS#33304 limit, the same
//! reason gql.tada uses the call form `graphql("...")`).
//!
//! Each entry's `result` is the query's **per-statement response tuple**: the
//! SurrealDB SDK returns one result per statement, in source order, so the tuple
//! has one element per statement. A responding statement (SELECT/RETURN/…)
//! contributes its result type; a non-responding statement (LET/DEFINE/…)
//! contributes `null`. A single-statement query is therefore a one-element
//! tuple, which `db.run` unwraps and `db.query` does not.
//!
//! ```ts
//! import { createClient, defineQuery, RecordId } from "./surrealguard.generated";
//!
//! const db = createClient({ url: "ws://localhost:8000/rpc" });
//!
//! // ...augments @surrealguard/client with:
//! //   "SELECT name FROM person WHERE team = $team":
//! //     { result: [Array<{ name: string }>]; params: { team: RecordId<"team"> } }
//! const peopleOf = defineQuery("SELECT name FROM person WHERE team = $team");
//! const rows = await db.run(peopleOf, { team: new RecordId("team", "red") });
//! //    ^ Array<{ name: string }> — the single statement's result, unwrapped
//!
//! //   "LET $t = time::now(); SELECT name FROM person":
//! //     { result: [null, Array<{ name: string }>]; params: Record<string, never> }
//! const [, people] = await db.run(defineQuery("LET $t = time::now(); SELECT name FROM person"));
//! //     ^ two statements, so the tuple stays: the LET responds with null
//! ```

use surrealguard_workspace::analysis::{AnalysisOutput, ParamInference, StatementAnalysis};

/// One embedded query's generated entry.
pub struct QueryEntry {
    /// The template's static parts, in order (one part, no substitutions,
    /// for a plain template).
    pub parts: Vec<String>,
    /// The per-statement response tuple rendered as TypeScript: one element per
    /// statement in source order, each a responding statement's result type or
    /// `null` for a non-responder (e.g. `[Array<{ name: string }>]`).
    pub result_type: String,
    /// The query's inferred parameters (named + `__hostN` substitutions).
    pub params: Vec<ParamInference>,
}

impl QueryEntry {
    /// Builds one query's entry from its analysis output: the per-statement
    /// response tuple from `output.statements` and the inferred parameters.
    ///
    /// This is the one step between "analyzed" and "rendered", and it is the
    /// step the CLI's `generate` runs for every embedded query it finds. It
    /// lives here rather than in the CLI so `tests/golden.rs` — which compiles
    /// the rendered module with `tsc` — exercises the same code path a user's
    /// `surrealguard generate` does, not a re-implementation of it.
    pub fn from_analysis(parts: Vec<String>, output: &AnalysisOutput) -> Self {
        Self {
            parts,
            result_type: response_tuple(&output.statements),
            params: output.inferred_params.clone(),
        }
    }
}

/// The per-statement response tuple as TypeScript. The SurrealDB SDK returns
/// one result per statement, in source order, so the tuple has one element
/// per statement: a responding statement contributes its rendered result
/// kind, a non-responder (`LET`, `DEFINE`, …) contributes `null`.
///
/// Every element is a [`crate::TsContext::Value`]: a tuple slot has no key
/// to omit, so an `option<T>` result stays `undefined | T` rather than
/// becoming an optional slot — dropping it would shorten the tuple.
pub fn response_tuple(statements: &[StatementAnalysis]) -> String {
    let elements: Vec<String> = statements
        .iter()
        .map(|statement| {
            statement.response_kind.as_ref().map_or_else(
                || "null".into(),
                |kind| crate::ts_type(kind, crate::TsContext::Value).text,
            )
        })
        .collect();
    format!("[{}]", elements.join(", "))
}

/// The prefix `crates/embed` gives a host substitution, restated because the
/// dependency runs the other way. `surrealguard_embed::HOST_PARAM_PREFIX` is
/// the definition; a test below pins the two together.
const HOST_PARAM_PREFIX: &str = "__host";

/// Rebuild the analyzed text from a template's static parts, restoring the
/// `$__hostN` parameter each hole became. `parts` is the text split on those
/// parameters, so interleaving the names back in reverses the split exactly.
fn join_with_holes(parts: &[String]) -> String {
    let mut key = String::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            key.push('$');
            key.push_str(HOST_PARAM_PREFIX);
            key.push_str(&(index - 1).to_string());
        }
        key.push_str(part);
    }
    key
}

/// Renders the complete generated declaration file.
pub fn render_registry(entries: &[QueryEntry]) -> String {
    let mut rows = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for entry in entries {
        // Call-form queries have exactly one part, so the join is a no-op.
        // A query carrying substitutions keys by the text the analyzer saw,
        // holes and all: a Svelte markup attribute reaches the runtime through
        // the preprocessor, which binds these same names, so the key it looks
        // up is this string byte for byte. Joining on a hole-shaped placeholder
        // instead would spell a key nothing ever asks for.
        let key = join_with_holes(&entry.parts);
        if !seen.insert(key.clone()) {
            continue;
        }
        rows.push(format!(
            "    {}: {{ result: {}; params: {} }};",
            ts_string(&key),
            entry.result_type,
            crate::params_type(&entry.params),
        ));
    }

    format!(
        r#"// Generated by surrealguard — do not edit.
//
// Value conventions name what the SurrealDB SDK actually decodes, which is its
// own value classes: record links are `RecordId<"table">`, uuids are `Uuid`,
// durations are `Duration`, decimals are `Decimal`. Datetimes are a native
// `Date` because `createClient` sets `codecOptions.useNativeDates`. `NONE`
// fields are optional. Anything crossing a serialisation boundary (SSR, a
// `fetch` response, `JSON.stringify`) goes through `Json<T>`, which maps
// `RecordId<"team">` to `` `team:${{string}}` `` and `Date` to `string`.
//
// The classes matter beyond reading: a `RecordId` parameter encodes to a
// record link on the wire, while a plain string encodes to a SurrealQL
// string, so `WHERE team = $team` matches only with the class.
//
// This file re-exports the runtime entry points and augments `SurqlRegistry`
// with one entry per analyzed query, keyed by its exact text. Import from here
// — the augmentation loads with them, so no side-effect import is needed.

import type {{ Decimal, Duration, GeoJSON, RecordId, Uuid }} from "@surrealguard/client";

export {{
  createClient,
  defineLive,
  defineQuery,
  fromSurreal,
  SurrealGuardError,
}} from "@surrealguard/client";
// Value re-exports, so constructing a param needs only this one import:
// `new RecordId("team", "red")`.
export {{ Decimal, Duration, RecordId, Uuid }} from "@surrealguard/client";
export type {{
  GeoJSON,
  Json,
  Preloaded,
  SurqlLive,
  SurqlQuery,
  SurrealGuardClient,
}} from "@surrealguard/client";

declare module "@surrealguard/client" {{
  interface SurqlRegistry {{
{rows}
  }}
}}

// The value types are referenced only when a query result uses them; this keeps
// the imports live regardless.
export type __SurqlGeneratedRefs = [Decimal, Duration, GeoJSON, RecordId, Uuid];
"#,
        rows = rows.join("\n")
    )
}

fn ts_string(text: &str) -> String {
    format!(
        "\"{}\"",
        text.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealdb_types::Kind;

    #[test]
    fn the_restated_host_prefix_still_matches_extraction() {
        // Generation restates this rather than depending on extraction. If the
        // definition moves, every generated key silently stops matching the
        // text the runtime asks for, and results degrade to `unknown` with no
        // error anywhere — so fail here instead.
        assert_eq!(HOST_PARAM_PREFIX, surrealguard_embed::HOST_PARAM_PREFIX);
    }

    #[test]
    fn a_hole_keys_by_the_parameter_the_analyzer_bound() {
        assert_eq!(join_with_holes(&["SELECT 1".into()]), "SELECT 1");
        assert_eq!(
            join_with_holes(&["WHERE a > ".into(), "".into()]),
            "WHERE a > $__host0"
        );
        assert_eq!(
            join_with_holes(&["a = ".into(), " AND b = ".into(), "".into()]),
            "a = $__host0 AND b = $__host1"
        );
    }

    #[test]
    fn registry_renders_keys_results_subs_and_params() {
        let entries = vec![QueryEntry {
            parts: vec![
                "SELECT name FROM person WHERE age > ".into(),
                " AND team = $team".into(),
            ],
            result_type: "[Array<{ name: string }>]".into(),
            params: vec![
                ParamInference {
                    name: "team".into(),
                    kind: Some(Kind::String),
                    domain: None,
                    required: true,
                    spans: Vec::new(),
                },
                ParamInference {
                    name: "__host0".into(),
                    kind: Some(Kind::Int),
                    domain: None,
                    required: true,
                    spans: Vec::new(),
                },
            ],
        }];

        let rendered = render_registry(&entries);

        // The hole keeps the parameter name the analyzer bound, because that is
        // the string the runtime looks up.
        assert!(
            rendered.contains("\"SELECT name FROM person WHERE age > $__host0 AND team = $team\"")
        );
        assert!(rendered.contains("result: [Array<{ name: string }>];"));
        assert!(rendered.contains("params: { team: string }"));
        assert!(rendered.contains("declare module \"@surrealguard/client\""));
        assert!(rendered.contains("interface SurqlRegistry"));
        assert!(
            rendered.contains("  createClient,") && rendered.contains("  defineQuery,"),
            "generated file re-exports the entry points for a single-import DX"
        );
        assert!(
            rendered.contains(
                "export { Decimal, Duration, RecordId, Uuid } from \"@surrealguard/client\""
            ),
            "value classes are re-exported as VALUES so a param can be constructed \
             (`new RecordId(\"team\", \"red\")`) from the one generated import"
        );
        assert!(
            rendered.contains("import type { Decimal, Duration, GeoJSON, RecordId, Uuid }"),
            "the augmentation body resolves these names from the file's own imports"
        );
    }

    /// The two spellings of one `option<string>`, side by side in the file a
    /// consumer actually imports. The same two queries sit in the golden
    /// fixture (`tests/fixtures/typecheck`), so these exact rows also appear
    /// in the generated `packages/client/test-d/gen/surrealguard.generated.ts`,
    /// where `optionality.test-d.ts` proves `tsc` treats them as different
    /// types.
    #[test]
    fn an_optional_field_and_an_optional_result_spell_differently() {
        use surrealdb_types::KindLiteral;

        let optional = Kind::Either(vec![Kind::None, Kind::String]);
        let mut row = std::collections::BTreeMap::new();
        row.insert("name".to_string(), Kind::String);
        row.insert("nick".to_string(), optional.clone());
        let rows = Kind::Array(Box::new(Kind::Literal(KindLiteral::Object(row))), None);

        let entries = vec![
            QueryEntry {
                parts: vec!["SELECT name, nick FROM person".into()],
                result_type: format!("[{}]", crate::ts_type(&rows, crate::TsContext::Value).text),
                params: Vec::new(),
            },
            QueryEntry {
                parts: vec!["SELECT VALUE nick FROM ONLY person:jane".into()],
                result_type: format!(
                    "[{}]",
                    crate::ts_type(&optional, crate::TsContext::Value).text
                ),
                params: Vec::new(),
            },
        ];

        let rendered = render_registry(&entries);

        // Inside a row the `none` is a key that may be missing…
        assert!(
            rendered.contains(
                "\"SELECT name, nick FROM person\": \
                 { result: [Array<{ name: string; nick?: string }>]; \
                 params: Record<string, never> };"
            ),
            "{rendered}"
        );
        // …and as a statement's whole result there is no key, so it is a
        // union member. Same kind, same file, two spellings.
        assert!(
            rendered.contains(
                "\"SELECT VALUE nick FROM ONLY person:jane\": \
                 { result: [undefined | string]; \
                 params: Record<string, never> };"
            ),
            "{rendered}"
        );
    }

    #[test]
    fn duplicate_query_texts_emit_one_registry_row() {
        let entry = || QueryEntry {
            parts: vec!["SELECT 1 FROM person".into()],
            result_type: "unknown".into(),
            params: Vec::new(),
        };
        let rendered = render_registry(&[entry(), entry()]);

        assert_eq!(rendered.matches("SELECT 1 FROM person").count(), 1);
    }
}
