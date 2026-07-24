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

    for (clause, checks_type) in [(&stmt.default, true), (&stmt.value, true)] {
        let Some(expr) = clause else {
            continue;
        };
        let kind = with_value_bound(ctx, declared.clone(), |ctx| {
            let fact = crate::analyzer::expression::infer::infer_expression_fact(expr, ctx);
            crate::analyzer::expression::check::check_value_expression(ctx, expr);
            fact.kind
        });
        check_computed_calls(ctx, expr);
        if !checks_type {
            continue;
        }
        if let (Some(declared), Some(kind)) = (&declared, kind) {
            if kind != Kind::Any && !crate::kinds::kind_is_assignable_to(&kind, declared) {
                let span =
                    surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), expr.span);
                ctx.emit(surrealguard_diagnostics::catalog::finding(
                    span,
                    2001,
                    format!(
                        "`{}`'s value is `{kind}`, but the field is declared `{declared}`",
                        idiom_text(&stmt.path.node)
                    ),
                ));
            }
        }
    }

    if let Some(assert) = &stmt.assert {
        let kind = with_value_bound(ctx, declared.clone(), |ctx| {
            let fact = crate::analyzer::expression::infer::infer_expression_fact(assert, ctx);
            crate::analyzer::expression::check::check_value_expression(ctx, assert);
            fact.kind
        });
        check_computed_calls(ctx, assert);
        if let Some(kind) = kind {
            if crate::analyzer::flow::if_else::definitely_not_bool(&kind) {
                let span =
                    surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), assert.span);
                ctx.emit(surrealguard_diagnostics::catalog::finding(
                    span,
                    2005,
                    format!("this ASSERT is a `{kind}`, not a `bool`"),
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

/// E2 — every table named in the field's declared type (`record<...>`) must
/// exist in the schema (1001). The declared `Kind` no longer carries the
/// per-target sub-spans, so the finding points at the whole type annotation.
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
        if ctx.schema().table(&table).is_none() {
            let mut finding = surrealguard_diagnostics::catalog::finding(
                surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), span),
                1001,
                format!("`record<{table}>` targets a table that's never defined"),
            )
            .with_help(format!("no `DEFINE TABLE {table}` exists in the workspace"));
            if let Some(suggestion) =
                crate::suggest::closest(&table, ctx.schema().tables.keys().map(String::as_str))
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
            ctx.emit(surrealguard_diagnostics::catalog::finding(
                surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), stmt.table.span),
                1001,
                format!(
                    "`{field_key}` is defined on `{}`, which is not a defined table",
                    stmt.table.node
                ),
            ));
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

/// Runs `f` with `$value` (and `$input`) bound: `$value` carries the
/// declared type inside `ASSERT`/`VALUE`/`DEFAULT` bodies.
fn with_value_bound<T>(
    ctx: &mut AnalysisContext<'_>,
    declared: Option<Kind>,
    f: impl FnOnce(&mut AnalysisContext<'_>) -> T,
) -> T {
    ctx.with_child_env(|ctx| {
        let span = surrealguard_syntax::span::SourceSpan::new(
            ctx.source().clone(),
            surrealguard_syntax::span::ByteRange::new(0, 0).expect("empty range is ordered"),
        );
        let mut fact = ExpressionFact::new(span.clone(), ExpressionValueClass::Variable);
        fact.kind = declared;
        ctx.define_local("value".to_string(), fact);
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

/// A value the constant-folder can reason about. Anything else (function
/// calls, param references other than `$value`, records, durations, …) is
/// not represented — folding bails rather than guessing.
#[derive(Clone, Debug, PartialEq)]
enum ConstVal {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    None,
    Null,
}

/// D1 — a `DEFAULT` that provably violates the field's own `ASSERT` (2037).
/// When a field carries BOTH clauses, SurrealDB substitutes the DEFAULT and
/// then enforces the ASSERT on that same value at write time, so a DEFAULT
/// outside the ASSERT's allowed set turns every field-omitting CREATE into a
/// hard runtime error. We fold the DEFAULT to a constant, bind it as
/// `$value`, and evaluate the ASSERT with a small folder — emitting only when
/// the ASSERT folds to a definite `false`, and BAILing on anything we cannot
/// fold so we never guess.
fn check_default_satisfies_assert(ctx: &mut AnalysisContext<'_>, stmt: &ast::DefineField) {
    let (Some(default), Some(assert)) = (&stmt.default, &stmt.assert) else {
        return;
    };
    let Some(value) = fold_const(&default.node) else {
        return;
    };
    if eval_assert(&assert.node, &value) == Some(false) {
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

/// Folds an expression to a [`ConstVal`], or `None` when it is not a literal
/// constant the folder understands.
fn fold_const(expr: &ast::Expr) -> Option<ConstVal> {
    match expr {
        ast::Expr::Literal(literal) => match literal {
            ast::Literal::Int(value) => Some(ConstVal::Int(*value)),
            ast::Literal::Float(value) => Some(ConstVal::Float(*value)),
            ast::Literal::String(value) => Some(ConstVal::Str(value.clone())),
            ast::Literal::Bool(value) => Some(ConstVal::Bool(*value)),
            ast::Literal::None => Some(ConstVal::None),
            ast::Literal::Null => Some(ConstVal::Null),
            _ => None,
        },
        ast::Expr::Prefix { op, expr } => match op.node {
            ast::PrefixOp::Neg => match fold_const(&expr.node)? {
                ConstVal::Int(value) => Some(ConstVal::Int(-value)),
                ConstVal::Float(value) => Some(ConstVal::Float(-value)),
                _ => None,
            },
            ast::PrefixOp::Pos => fold_const(&expr.node),
            _ => None,
        },
        _ => None,
    }
}

/// Evaluates an ASSERT expression with `$value` bound to `value`. Returns
/// `Some(bool)` only when the whole expression folds to a definite constant;
/// `None` means "cannot fold — bail, never guess".
fn eval_assert(expr: &ast::Expr, value: &ConstVal) -> Option<bool> {
    match expr {
        ast::Expr::Literal(ast::Literal::Bool(literal)) => Some(*literal),
        ast::Expr::Prefix { op, expr } if op.node == ast::PrefixOp::Not => {
            eval_assert(&expr.node, value).map(|folded| !folded)
        }
        ast::Expr::Binary { lhs, op, rhs } => eval_binary(&op.node, &lhs.node, &rhs.node, value),
        _ => None,
    }
}

/// Evaluates a binary ASSERT term. `AND`/`OR` short-circuit so one provable
/// side can decide the result even when the other cannot fold; comparisons
/// and membership require both sides to fold.
fn eval_binary(
    op: &ast::BinaryOp,
    lhs: &ast::Expr,
    rhs: &ast::Expr,
    value: &ConstVal,
) -> Option<bool> {
    use ast::BinaryOp;
    match op {
        BinaryOp::And => {
            let left = eval_assert(lhs, value);
            let right = eval_assert(rhs, value);
            if left == Some(false) || right == Some(false) {
                Some(false)
            } else if left == Some(true) && right == Some(true) {
                Some(true)
            } else {
                None
            }
        }
        BinaryOp::Or => {
            let left = eval_assert(lhs, value);
            let right = eval_assert(rhs, value);
            if left == Some(true) || right == Some(true) {
                Some(true)
            } else if left == Some(false) && right == Some(false) {
                Some(false)
            } else {
                None
            }
        }
        BinaryOp::Eq => operand_val(lhs, value)
            .zip(operand_val(rhs, value))
            .and_then(|(a, b)| const_eq(&a, &b)),
        BinaryOp::NotEq => operand_val(lhs, value)
            .zip(operand_val(rhs, value))
            .and_then(|(a, b)| const_eq(&a, &b).map(|equal| !equal)),
        BinaryOp::Lt => const_order(lhs, rhs, value).map(|o| o == std::cmp::Ordering::Less),
        BinaryOp::LtEq => {
            const_order(lhs, rhs, value).map(|o| o != std::cmp::Ordering::Greater)
        }
        BinaryOp::Gt => const_order(lhs, rhs, value).map(|o| o == std::cmp::Ordering::Greater),
        BinaryOp::GtEq => const_order(lhs, rhs, value).map(|o| o != std::cmp::Ordering::Less),
        BinaryOp::Other(name)
            if matches!(name.to_ascii_uppercase().as_str(), "IN" | "INSIDE") =>
        {
            eval_membership(lhs, rhs, value)
        }
        BinaryOp::Other(name) if name.eq_ignore_ascii_case("contains") => {
            eval_membership(rhs, lhs, value)
        }
        _ => None,
    }
}

/// `$value IN [a, b, ...]`: `element` is the tested value, `collection` an
/// array literal of constants. Returns `Some(true)` when the element equals a
/// folded member, `Some(false)` when every member folds and none match, and
/// `None` when a member cannot fold (so a non-match can't be proven).
fn eval_membership(element: &ast::Expr, collection: &ast::Expr, value: &ConstVal) -> Option<bool> {
    let element = operand_val(element, value)?;
    let ast::Expr::Array(members) = collection else {
        return None;
    };
    let mut all_folded = true;
    for member in members {
        match fold_const(&member.node) {
            Some(member) => {
                if const_eq(&element, &member) == Some(true) {
                    return Some(true);
                }
            }
            None => all_folded = false,
        }
    }
    if all_folded {
        Some(false)
    } else {
        None
    }
}

/// Folds an operand, resolving the bound `$value` reference to `value`.
fn operand_val(expr: &ast::Expr, value: &ConstVal) -> Option<ConstVal> {
    match expr {
        ast::Expr::Param(name) if name == "value" => Some(value.clone()),
        _ => fold_const(expr),
    }
}

/// Constant equality. `None` when the two values are not comparable under a
/// shape the folder proves (bail rather than assume unequal).
fn const_eq(a: &ConstVal, b: &ConstVal) -> Option<bool> {
    match (a, b) {
        (ConstVal::Int(x), ConstVal::Int(y)) => Some(x == y),
        (ConstVal::Float(x), ConstVal::Float(y)) => Some(x == y),
        (ConstVal::Int(x), ConstVal::Float(y)) | (ConstVal::Float(y), ConstVal::Int(x)) => {
            Some(*x as f64 == *y)
        }
        (ConstVal::Str(x), ConstVal::Str(y)) => Some(x == y),
        (ConstVal::Bool(x), ConstVal::Bool(y)) => Some(x == y),
        (ConstVal::None, ConstVal::None) => Some(true),
        (ConstVal::Null, ConstVal::Null) => Some(true),
        // Distinct sentinels / distinct scalar kinds never compare equal.
        (ConstVal::None | ConstVal::Null, _) | (_, ConstVal::None | ConstVal::Null) => Some(false),
        _ => None,
    }
}

/// Orders two operands numerically (or lexically for strings). `None` for any
/// pairing the folder cannot compare.
fn const_order(
    lhs: &ast::Expr,
    rhs: &ast::Expr,
    value: &ConstVal,
) -> Option<std::cmp::Ordering> {
    let a = operand_val(lhs, value)?;
    let b = operand_val(rhs, value)?;
    match (&a, &b) {
        (ConstVal::Int(x), ConstVal::Int(y)) => Some(x.cmp(y)),
        (ConstVal::Str(x), ConstVal::Str(y)) => Some(x.cmp(y)),
        _ => {
            let x = const_as_f64(&a)?;
            let y = const_as_f64(&b)?;
            x.partial_cmp(&y)
        }
    }
}

fn const_as_f64(value: &ConstVal) -> Option<f64> {
    match value {
        ConstVal::Int(v) => Some(*v as f64),
        ConstVal::Float(v) => Some(*v),
        _ => None,
    }
}

fn idiom_text(idiom: &ast::Idiom) -> String {
    crate::analyzer::expression::infer::plain_field_segments(idiom)
        .map(|segments| segments.join("."))
        .unwrap_or_default()
}
