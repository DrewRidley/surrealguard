//! `DEFINE FIELD` analysis.
//!
//! The definition's own contracts: it must target a known table (1001) and
//! not redefine a field without `OVERWRITE` (1022); its declared type must
//! be expressible (6003); a `DEFAULT` (or computed `VALUE`) must inhabit the
//! declared type (2001); an `ASSERT` is a condition with `$value` in scope
//! as the declared type (2005 when it can never be a bool, plus the usual
//! expression checking); and computed contexts should not block or reach out
//! (7012).

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::contract::{Contract, Position};
use crate::analyzer::facts::Bindings;
use crate::expression::{ExpressionFact, ExpressionValueClass, PartialReason};

pub(crate) fn analyze_define_field(ctx: &mut AnalysisContext<'_>, stmt: &ast::DefineField) -> Kind {
    let parsed_type = stmt
        .ty
        .as_ref()
        .map(|ty| crate::schema::kind_from_type_expr(&ty.node, ctx.source_text()));
    let declared = parsed_type.as_ref().and_then(|parsed| parsed.kind.clone());

    let no_partial = Vec::new();
    let partial = parsed_type
        .as_ref()
        .map_or(&no_partial, |parsed| &parsed.partial);
    check_field_definition(ctx, stmt, partial);
    check_record_targets(ctx, stmt, declared.as_ref());
    check_reference_back_target(ctx, stmt);

    for (position, clause) in [
        (Position::FieldDefault, &stmt.default),
        (Position::FieldValue, &stmt.value),
    ] {
        let Some(expr) = clause else {
            continue;
        };
        let fact = with_value_bound(ctx, declared.clone(), &stmt.table.node, |ctx| {
            let fact = crate::analyzer::expression::infer::infer_expression_fact(expr, ctx);
            crate::analyzer::expression::check::check_value_expression(ctx, expr);
            fact
        });
        check_computed_calls(ctx, expr);
        // A declared `VALUE`/`DEFAULT` inhabits the field's type under exactly
        // the contract a *written* value does, so it is the same contract: a
        // constant is compared as the literal it is, and a non-constant (call,
        // param, subquery) keeps its widened kind and stays silent.
        let Some(declared) = &declared else {
            continue;
        };
        let contract = Contract::new(position, declared.clone());
        let term = crate::analyzer::facts::eval(&expr.node, Bindings::NONE);
        if let Some(kind) = contract.violation(&term, &fact) {
            let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), expr.span);
            let field_name = idiom_text(&stmt.path.node);
            let def_span =
                surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), stmt.path.span);
            ctx.emit(
                surrealguard_diagnostics::catalog::finding(
                    span,
                    contract.code(),
                    format!(
                        "`{field_name}`'s value is `{}`, but the field is declared `{}`",
                        crate::render::render_offending(&kind, Some(declared)),
                        crate::render_kind(declared),
                    ),
                )
                .with_related(def_span, format!("`{field_name}` is defined here")),
            );
        }
    }

    if let Some(assert) = &stmt.assert {
        let kind = with_value_bound(ctx, declared.clone(), &stmt.table.node, |ctx| {
            let fact = crate::analyzer::expression::infer::infer_expression_fact(assert, ctx);
            crate::analyzer::expression::check::check_value_expression(ctx, assert);
            fact.kind
        });
        check_computed_calls(ctx, assert);
        if let Some(kind) = kind {
            if Contract::condition(Position::FieldAssert)
                .decide(&kind)
                .is_violation()
            {
                let span =
                    surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), assert.span);
                ctx.emit(surrealguard_diagnostics::catalog::finding(
                    span,
                    2005,
                    format!(
                        "this ASSERT is a `{}`, not a `bool`",
                        crate::render::render_offending(&kind, Some(&Kind::Bool))
                    ),
                ));
            }
        }
    }

    check_default_satisfies_assert(ctx, stmt);

    // Each `PERMISSIONS FOR <action> WHERE <expr>` predicate is evaluated
    // against a row of this table; `$value` is the field's declared kind.
    super::permissions::analyze_permission_predicates(
        ctx,
        &stmt.table.node,
        declared.clone().unwrap_or(Kind::Any),
        &stmt.permissions,
    );

    Kind::None
}

/// A record-reference back-traversal clause (`COMPUTED <~passkey`) must name a
/// table that exists *somewhere in the workspace* (1001) — the target of a
/// mutual reference is as often declared after the traversal as before it.
///
/// Without this a typo'd target silently produced `unknown` and no finding:
/// [`reference_back_traversal_kind`] returns `None` both for "the table does
/// not exist" and for "the table exists but nothing references back", so the
/// degraded type could not tell a typo from an unmodellable-but-legitimate
/// shape. Only the first is reported — a known table whose `REFERENCE` fields
/// point elsewhere is genuinely unresolvable, and `any` is the honest answer
/// there.
///
/// [`reference_back_traversal_kind`]: crate::analyzer::data::select::reference_back_traversal_kind
fn check_reference_back_target(ctx: &mut AnalysisContext<'_>, stmt: &ast::DefineField) {
    for clause in [&stmt.computed, &stmt.value, &stmt.default] {
        let Some(expr) = clause else {
            continue;
        };
        let ast::Expr::Idiom(idiom) = &expr.node else {
            continue;
        };
        let Some((target, _)) = crate::analyzer::data::select::reference_back_target(idiom) else {
            continue;
        };
        // Order-independent: a back-reference is mutual, so its target is
        // routinely declared after the field that traverses it.
        crate::analyzer::data::check_table_defined_anywhere(ctx, &target.node, target.span);
    }
}

/// E2 — every table named in the field's declared type (`record<...>`) must
/// exist in the workspace (1001). The declared `Kind` no longer carries the
/// per-target sub-spans, so the finding points at the whole type annotation.
///
/// "Exists" is asked of the *workspace* catalog, not the incrementally-built
/// one: a schema is applied as a unit, so `DEFINE FIELD b ON a TYPE record<bb>`
/// written above `DEFINE TABLE bb` is valid SurrealQL, and the help text
/// ("no `DEFINE TABLE bb` exists in the workspace") already claimed as much.
/// A target defined nowhere still errors.
fn check_record_targets(
    ctx: &mut AnalysisContext<'_>,
    stmt: &ast::DefineField,
    declared: Option<&Kind>,
) {
    let Some(declared) = declared else {
        return;
    };
    let mut tables = Vec::new();
    collect_record_tables(declared, &mut tables);
    if tables.is_empty() {
        return;
    }
    let span = stmt.ty.as_ref().map_or(stmt.path.span, |ty| ty.span);
    for table in tables {
        if !ctx.table_defined_anywhere(&table) {
            let mut finding = surrealguard_diagnostics::catalog::finding(
                surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), span),
                1001,
                format!("`record<{table}>` targets a table that's never defined"),
            )
            .with_help(format!("no `DEFINE TABLE {table}` exists in the workspace"));
            if let Some(suggestion) =
                crate::suggest::closest(&table, ctx.known_table_names())
            {
                finding = finding.with_help(format!("did you mean `{suggestion}`?"));
            }
            ctx.emit(finding);
        }
    }
}

/// Collects every table named by a `record<...>` leaf, recursing through the
/// wrappers a field type can nest a record inside: `option<T>`/unions lower to
/// `Either`, and `array<T>`/`set<T>` carry an element kind.
fn collect_record_tables(kind: &Kind, out: &mut Vec<String>) {
    match kind {
        Kind::Record(tables) => {
            for table in tables {
                let name = table.to_string();
                if !out.contains(&name) {
                    out.push(name);
                }
            }
        }
        Kind::Either(variants) => {
            for variant in variants {
                collect_record_tables(variant, out);
            }
        }
        Kind::Array(element, _) | Kind::Set(element, _) => collect_record_tables(element, out),
        _ => {}
    }
}

/// The definition's catalog contracts: target a known table (1001), don't
/// redefine an existing field without `OVERWRITE` (1022), and declare a type
/// the analyzer can express (6003).
fn check_field_definition(
    ctx: &mut AnalysisContext<'_>,
    stmt: &ast::DefineField,
    partial: &[PartialReason],
) {
    let path = crate::schema::idiom_field_path(&stmt.path.node);
    let field_key = path.join(".");

    if let Some(reason) = partial.iter().find_map(|reason| match reason {
        PartialReason::UnsupportedSyntax(text) => Some(text),
        PartialReason::Unresolved | PartialReason::DynamicExpression => None,
    }) {
        let span = stmt.ty.as_ref().map_or(stmt.path.span, |ty| ty.span);
        ctx.emit(
            surrealguard_diagnostics::catalog::finding(
                surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), span),
                6003,
                format!("surrealguard can't analyze the type of `{field_key}` yet"),
            )
            .with_help(format!("unsupported type syntax: {reason}")),
        );
    }

    match ctx.schema().table(&stmt.table.node) {
        None => {
            let finding = surrealguard_diagnostics::catalog::finding(
                surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), stmt.table.span),
                1001,
                format!(
                    "`{field_key}` is defined on `{}`, which is not a defined table",
                    stmt.table.node
                ),
            );
            let finding =
                crate::analyzer::data::with_table_suggestion(finding, ctx, &stmt.table.node);
            ctx.emit(finding);
        }
        Some(table) if !stmt.overwrite && field_is_duplicate(table, &stmt.path.node, &field_key) => {
            let mut finding = surrealguard_diagnostics::catalog::finding(
                surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), stmt.path.span),
                1022,
                format!("`{field_key}` is already defined on `{}`", stmt.table.node),
            )
            .with_help("use `DEFINE FIELD OVERWRITE` to redefine it intentionally");
            if let Some(existing) = table.fields.get(&field_key) {
                finding = finding
                    .with_related(existing.name_span.clone(), format!("`{field_key}` is defined here"));
            }
            ctx.emit(finding);
        }
        Some(_) => {}
    }

    check_subfield_parent(ctx, stmt, &field_key);
}

/// A subfield may only be declared under a parent whose declared kind has room
/// for it (1025).
///
/// The contract is the parent's declaration: `title TYPE string` says `title`
/// holds a string, so `title.sub` describes a member that can never exist —
/// and, left in the catalog, it would silently replace `title`'s declared kind
/// with `{ sub: string }`, turning the perfectly valid `SET title = 'hello'`
/// into a type error.
///
/// Only a parent whose declared kind PROVES there is no room is reported.
/// An undeclared parent, an untyped/`any` parent, a bare (or `FLEXIBLE`)
/// `object`, a literal object, and any collection reached through `[*]` all
/// have room, and stay silent.
fn check_subfield_parent(
    ctx: &mut AnalysisContext<'_>,
    stmt: &ast::DefineField,
    field_key: &str,
) {
    let steps = crate::schema::idiom_field_steps(&stmt.path.node);
    let Some(table) = ctx.schema().table(&stmt.table.node) else {
        return;
    };
    let crate::schema::FieldPlacement::Rejected {
        ancestor,
        ancestor_kind,
    } = crate::schema::field_placement(table, &steps)
    else {
        return;
    };
    let related = table.fields.get(&ancestor).map(|def| def.name_span.clone());
    let rendered = crate::render_kind(&ancestor_kind);
    let mut finding = surrealguard_diagnostics::catalog::finding(
        surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), stmt.path.span),
        1025,
        format!("`{field_key}` declares a subfield of `{ancestor}`, which is `{rendered}`"),
    )
    .with_help(format!(
        "only an object-shaped field has subfields — redeclare `{ancestor}` as an object, or drop this definition"
    ));
    if let Some(related) = related {
        finding = finding.with_related(related, format!("`{ancestor}` is declared here"));
    }
    ctx.emit(finding);
}

/// Whether `path` redefines an existing field. The duplicate-definition
/// contract keys on the FULL field path: a `field[*]` element-type definition
/// (or any `[index]`/wildcard sub-definition) is structurally distinct from
/// the base array field `field`, even though both collapse to the same
/// `idiom_field_path`. Only a plain (all-`Field`) path — whose dotted key is
/// lossless — can be a true duplicate of a stored field.
fn field_is_duplicate(table: &crate::schema::TableDef, path: &ast::Idiom, field_key: &str) -> bool {
    crate::analyzer::expression::infer::plain_field_segments(path).is_some()
        && table.fields.contains_key(field_key)
}

/// Infers the kind of a field's `VALUE`/`COMPUTED`/`DEFAULT` clause expression
/// with `$value`/`$input` bound exactly as they are inside a field clause body
/// (`$value` unknown here — the field has no declared `TYPE`). Used by schema
/// extraction to type an untyped field from the value it stores; the walk's own
/// [`analyze_define_field`] still owns the clause's real diagnostics, so this
/// only reads the inferred kind.
pub(crate) fn infer_field_clause_kind(
    ctx: &mut AnalysisContext<'_>,
    expr: &ast::Spanned<ast::Expr>,
    this_table: &str,
) -> Option<Kind> {
    // `$this` is the record being written/computed, so `$this.field` resolves
    // against the owning table — and so do its four siblings, which
    // `with_value_bound` now binds from the same table. (Bare field references
    // and graph traversals resolve through the row-table context the caller
    // sets.)
    with_value_bound(ctx, None, this_table, |ctx| {
        crate::analyzer::expression::infer::infer_expression_fact(expr, ctx).kind
    })
}

/// Runs `f` with the document context a field clause sees bound: `$value`
/// carries the declared type inside `ASSERT`/`VALUE`/`DEFAULT`/`COMPUTED`
/// bodies, and `$this`/`$self`/`$before`/`$after`/`$input` name the record
/// being written.
///
/// The document set comes from `context_params`' one table. It used to be a
/// two-name copy (`$value`, `$input`), which is why `$this` had to be bound
/// again by hand one function below and `$self`/`$before`/`$after` were bound
/// by nobody — while the hover map offered all five.
fn with_value_bound<T>(
    ctx: &mut AnalysisContext<'_>,
    declared: Option<Kind>,
    table: &str,
    f: impl FnOnce(&mut AnalysisContext<'_>) -> T,
) -> T {
    let document = crate::context_params::document_param_bindings(table);
    ctx.with_child_env(|ctx| {
        // A field's `ASSERT`/`VALUE`/`DEFAULT` is evaluated at write time, where
        // the session is not statically known. The top-level
        // `$auth: option<record>` seed must not reach here: a `DEFAULT $auth` on
        // a `record<T>` field would otherwise read as a type mismatch
        // (option<record> vs record<T>) — a false 2001. Reverting the session
        // params to unmodeled restores the pre-seed behavior for these clauses.
        ctx.unbind_session_params();
        let span = surrealguard_syntax::span::SourceSpan::new(
            ctx.source().clone(),
            surrealguard_syntax::span::ByteRange::new(0, 0).expect("empty range is ordered"),
        );
        let mut fact = ExpressionFact::new(span.clone(), ExpressionValueClass::Variable);
        fact.kind = declared;
        for (name, kind) in document {
            let mut bound = ExpressionFact::new(span.clone(), ExpressionValueClass::Variable);
            bound.kind = Some(kind);
            ctx.define_local(name.to_string(), bound);
        }
        ctx.define_local("value".to_string(), fact);
        // `$input` is the value being written into *this field*, not the record
        // containing it, and its kind before coercion is not the declared one —
        // so `any` is the honest answer rather than a guess.
        let mut input = ExpressionFact::new(span, ExpressionValueClass::Variable);
        input.kind = Some(Kind::Any);
        ctx.define_local("input".to_string(), input);
        f(ctx)
    })
}

/// Computed contexts run on every write; blocking or side-effecting calls
/// there are a footgun (7012).
fn check_computed_calls(ctx: &mut AnalysisContext<'_>, expr: &ast::Spanned<ast::Expr>) {
    match &expr.node {
        ast::Expr::Call(call) => {
            let path = call.path.node.as_str();
            if path.starts_with("http::") || path == "sleep::sleep" || path == "sleep" {
                let span = surrealguard_syntax::span::SourceSpan::new(
                    ctx.source().clone(),
                    call.path.span,
                );
                ctx.emit(
                    surrealguard_diagnostics::catalog::finding(
                        span,
                        7012,
                        format!("`{path}` runs on every write to this row"),
                    )
                    .with_help(
                        "this clause is computed on every write; avoid blocking or side-effecting calls here",
                    ),
                );
            }
            for arg in &call.args {
                check_computed_calls(ctx, arg);
            }
        }
        ast::Expr::Binary { lhs, rhs, .. } => {
            check_computed_calls(ctx, lhs);
            check_computed_calls(ctx, rhs);
        }
        ast::Expr::Prefix { expr: inner, .. } | ast::Expr::Cast { expr: inner, .. } => {
            check_computed_calls(ctx, inner);
        }
        ast::Expr::Array(elements) => {
            for element in elements {
                check_computed_calls(ctx, element);
            }
        }
        ast::Expr::Object(fields) => {
            for (_, value) in fields {
                check_computed_calls(ctx, value);
            }
        }
        _ => {}
    }
}

/// D1 — a `DEFAULT` that provably violates the field's own `ASSERT` (2037).
/// When a field carries BOTH clauses, SurrealDB substitutes the DEFAULT and
/// then enforces the ASSERT on that same value at write time, so a DEFAULT
/// outside the ASSERT's allowed set turns every field-omitting CREATE into a
/// hard runtime error. We fold the DEFAULT to a constant, bind it as
/// `$value`, and evaluate the ASSERT under that binding — emitting only when
/// the ASSERT folds to a definite `false`, and BAILing on anything we cannot
/// fold so we never guess.
///
/// The folder is the analyzer's one folder ([`crate::analyzer::facts::eval`]);
/// binding `$value` is the whole of what used to be a second copy of it.
fn check_default_satisfies_assert(ctx: &mut AnalysisContext<'_>, stmt: &ast::DefineField) {
    use crate::analyzer::facts::term::{fold, fold_bool, Bindings};

    let (Some(default), Some(assert)) = (&stmt.default, &stmt.assert) else {
        return;
    };
    let Some(value) = fold(&default.node, Bindings::NONE) else {
        return;
    };
    if fold_bool(&assert.node, Bindings::value(&value)) == Some(false) {
        let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), default.span);
        ctx.emit(surrealguard_diagnostics::catalog::finding(
            span,
            2037,
            format!(
                "`{}`'s DEFAULT can never satisfy its own ASSERT",
                idiom_text(&stmt.path.node)
            ),
        ));
    }
}

fn idiom_text(idiom: &ast::Idiom) -> String {
    crate::analyzer::expression::infer::plain_field_segments(idiom)
        .map(|segments| segments.join("."))
        .unwrap_or_default()
}
