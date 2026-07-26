//! `PERMISSIONS` predicate analysis, shared by `DEFINE TABLE` and
//! `DEFINE FIELD`.
//!
//! SurrealDB evaluates a `PERMISSIONS FOR <action> WHERE <expr>` predicate
//! against the record being accessed (`core/src/doc/check.rs`
//! `process_permissions`): bare field references resolve against the row, and
//! the session params (`$auth`/`$token`/`$session`/`$access`) resolve from the
//! connection. A predicate that names an undefined field, calls an undefined
//! `fn::`, compares two provably-disjoint values, or can never be a boolean is
//! a silent total-deny/allow bug — but the predicate was discarded at lowering,
//! so none of it was ever checked.
//!
//! Lowering now retains each predicate; this walks it through the existing
//! expression pass in a child env scoped to a row of the table, so the
//! already-built checks fire for free: undefined field (1002), undefined
//! function (5001), always-false/true comparison (7005), and — added here —
//! a predicate whose kind is provably never boolean (2005).
//!
//! Schema-visibility note: the pipeline builds each source's own catalog
//! incrementally, applying a statement's effects only after analyzing it (see
//! `analyzer/pipeline.rs`). A `DEFINE TABLE`'s own fields are always declared
//! by *later* `DEFINE FIELD` statements, so a table-level predicate's bare
//! field references resolve against a table with no fields yet — which the
//! field-resolution pass treats as schemaless and leaves alone (no false
//! positives, but no F0/F3/F4 either). `fn::` dispatch (F2) needs no field
//! context and fires on table-level predicates regardless; and every
//! field-level predicate resolves fully against the fields declared before it.

use surrealdb_types::{Kind, Table};
use surrealguard_syntax::ast;
use surrealguard_syntax::span::{ByteRange, SourceSpan};

use crate::analyzer::context::AnalysisContext;
use crate::expression::{ExpressionFact, ExpressionValueClass};

/// Walks each `PERMISSIONS` predicate on `table_name` in a child env scoped to
/// a row of that table. `value_kind` is what `$value` binds to — the record
/// for a table predicate, the field's declared kind for a field predicate.
pub(crate) fn analyze_permission_predicates(
    ctx: &mut AnalysisContext<'_>,
    table_name: &str,
    value_kind: Kind,
    predicates: &[ast::Spanned<ast::Expr>],
) {
    if predicates.is_empty() {
        return;
    }
    let row_table = ctx.schema().tables.get(table_name);
    ctx.with_row_table(row_table, |ctx| {
        ctx.with_child_env(|ctx| {
            bind_row_params(ctx, table_name, value_kind);
            for predicate in predicates {
                let fact =
                    crate::analyzer::expression::infer::infer_expression_fact(predicate, ctx);
                crate::analyzer::expression::check::check_value_expression(ctx, predicate);
                // F0 — a bare field reference the schemafull table doesn't
                // declare (1002). Param-rooted idioms (`$auth.role`,
                // `$this.field`) are not plain field paths, so session params
                // and record members never trip this.
                if let Some(table) = row_table {
                    crate::analyzer::data::check_expression_field_paths(ctx, table, predicate, 1002);
                }
                // F4 — a predicate whose kind is provably never boolean can
                // never gate access as written. Guarded to provably-non-bool
                // scalars (`Any`/`None`/`Null` are skipped) to stay low-FP.
                if let Some(kind) = fact.kind {
                    if crate::analyzer::contract::Contract::condition(
                        crate::analyzer::contract::Position::PermissionPredicate,
                    )
                    .decide(&kind)
                    .is_violation()
                    {
                        let span = SourceSpan::new(ctx.source().clone(), predicate.span);
                        ctx.emit(surrealguard_diagnostics::catalog::finding(
                            span,
                            2005,
                            format!(
                                "this permission predicate is a `{}`, not a `bool`",
                                crate::render::render_offending(&kind, Some(&Kind::Bool))
                            ),
                        ));
                    }
                }
            }
        });
    });
}

/// Binds the params a permission predicate sees: the row params (`$value`,
/// `$this`/`$self`, `$before`/`$after`, `$input`) and the open session params
/// (`$auth`/`$token`/`$session`/`$access`/`$scope`). Session params stay open
/// shapes so member access on them (`$auth.role`) is never flagged.
fn bind_row_params(ctx: &mut AnalysisContext<'_>, table_name: &str, value_kind: Kind) {
    let record = Kind::Record(vec![Table::from(table_name)]);
    define(ctx, "value", value_kind);
    define(ctx, "this", record.clone());
    define(ctx, "self", record.clone());
    define(ctx, "before", record.clone());
    define(ctx, "after", record.clone());
    define(ctx, "input", record);
    // Session/access params: `$auth` is a record whose table depends on the
    // access method (unmodeled), so it stays a bare open record; the rest are
    // generic shapes.
    define(ctx, "auth", Kind::Record(Vec::new()));
    define(ctx, "token", Kind::Object);
    // `$session` gets its fixed field composition (the leniency gate proves an
    // unknown-field access on a closed literal object emits nothing, so no valid
    // access can start erroring); `$token` stays open (arbitrary JWT claims).
    define(ctx, "session", crate::context_params::session_kind());
    define(ctx, "access", Kind::String);
    define(ctx, "scope", Kind::String);
}

fn define(ctx: &mut AnalysisContext<'_>, name: &str, kind: Kind) {
    let span = SourceSpan::new(
        ctx.source().clone(),
        ByteRange::new(0, 0).expect("empty range is ordered"),
    );
    let mut fact = ExpressionFact::new(span, ExpressionValueClass::Variable);
    fact.kind = Some(kind);
    ctx.define_local(name.to_string(), fact);
}

#[cfg(test)]
mod tests {
    use crate::analysis::{analyze_query, Workspace};

    /// The rendered code strings (e.g. `"E1002"`) a query produces.
    fn codes(query: &str) -> Vec<String> {
        let mut workspace = Workspace::default();
        analyze_query(&mut workspace, query)
            .diagnostics
            .iter()
            .map(|finding| finding.code().to_string())
            .collect()
    }

    fn fires(query: &str, code: &str) -> bool {
        codes(query).iter().any(|c| c == code)
    }

    // ---- F0: undefined field in a PERMISSIONS predicate (1002) ----

    #[test]
    fn f0_field_permission_undefined_field_fires_1002() {
        let query = concat!(
            "DEFINE TABLE post SCHEMAFULL;\n",
            "DEFINE FIELD owner ON post TYPE string;\n",
            "DEFINE FIELD secret ON post TYPE string PERMISSIONS FOR select WHERE ownerr = $auth;\n",
        );
        assert!(fires(query, "E1002"), "codes: {:?}", codes(query));
    }

    #[test]
    fn f0_defined_field_stays_clean() {
        let query = concat!(
            "DEFINE TABLE post SCHEMAFULL;\n",
            "DEFINE FIELD owner ON post TYPE string;\n",
            "DEFINE FIELD secret ON post TYPE string PERMISSIONS FOR select WHERE owner = $auth;\n",
        );
        assert!(!fires(query, "E1002"), "codes: {:?}", codes(query));
    }

    // ---- F2: undefined `fn::` in a PERMISSIONS predicate (5001) ----

    #[test]
    fn f2_undefined_function_fires_5001() {
        let query =
            "DEFINE TABLE post SCHEMAFULL PERMISSIONS FOR select WHERE fn::org::permissible($auth);\n";
        assert!(fires(query, "E5001"), "codes: {:?}", codes(query));
    }

    #[test]
    fn f2_defined_function_over_auth_stays_clean() {
        let query = concat!(
            "DEFINE FUNCTION fn::can_read($a: record) { RETURN true; };\n",
            "DEFINE TABLE post SCHEMAFULL PERMISSIONS FOR select WHERE fn::can_read($auth.id);\n",
        );
        assert!(!fires(query, "E5001"), "codes: {:?}", codes(query));
        assert!(!fires(query, "E1002"), "codes: {:?}", codes(query));
    }

    // ---- Session params resolve leniently (no false 1002/5001) ----

    #[test]
    fn session_param_member_stays_clean() {
        let query =
            "DEFINE TABLE post SCHEMAFULL PERMISSIONS FOR select WHERE $auth.role = 'admin';\n";
        assert!(!fires(query, "E1002"), "codes: {:?}", codes(query));
        assert!(!fires(query, "E5001"), "codes: {:?}", codes(query));
    }

    // ---- F3: always-false comparison in a predicate (7005) ----

    #[test]
    fn f3_null_against_option_field_fires_7005() {
        let query = concat!(
            "DEFINE TABLE post SCHEMAFULL;\n",
            "DEFINE FIELD deleted ON post TYPE option<datetime>;\n",
            "DEFINE FIELD secret ON post TYPE string PERMISSIONS FOR select WHERE deleted = NULL;\n",
        );
        assert!(fires(query, "L7005"), "codes: {:?}", codes(query));
    }

    // ---- F4: a predicate that can never be boolean (2005) ----

    #[test]
    fn f4_non_bool_predicate_fires_2005() {
        let query = concat!(
            "DEFINE TABLE post SCHEMAFULL;\n",
            "DEFINE FIELD title ON post TYPE string;\n",
            "DEFINE FIELD secret ON post TYPE string PERMISSIONS FOR select WHERE title;\n",
        );
        assert!(fires(query, "E2005"), "codes: {:?}", codes(query));
    }

    #[test]
    fn f4_boolean_predicate_stays_clean() {
        let query = concat!(
            "DEFINE TABLE user SCHEMAFULL;\n",
            "DEFINE TABLE post SCHEMAFULL;\n",
            "DEFINE FIELD owner ON post TYPE record<user>;\n",
            "DEFINE FIELD secret ON post TYPE string PERMISSIONS FOR select WHERE owner = $auth;\n",
        );
        assert!(!fires(query, "E2005"), "codes: {:?}", codes(query));
    }

    // ---- Table-level `fn::` predicate (the workshop's dominant shape) ----

    #[test]
    fn f2_table_level_permission_over_function_stays_clean() {
        let query = concat!(
            "DEFINE FUNCTION fn::permissible($a: record) { RETURN true; };\n",
            "DEFINE TABLE post SCHEMAFULL PERMISSIONS FOR select WHERE fn::permissible($auth);\n",
        );
        assert!(!fires(query, "E5001"), "codes: {:?}", codes(query));
    }
}
