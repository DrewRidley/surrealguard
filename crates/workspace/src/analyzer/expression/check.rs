//! Expression invariant checking.
//!
//! The checking-side twin of [`super::infer`]: inference computes kinds
//! and never rejects; this walk reads the same expressions and emits
//! findings where an invariant is violated. Both run at the sites that
//! own the expressions.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;
use surrealguard_syntax::span::SourceSpan;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::expression::infer::{binary_result_kind, infer_expression_fact, is_numeric};

/// Recursively checks operator invariants in a value expression:
/// incompatible operands (2004) and negation of non-numerics (2014).
/// Unknown kinds are never violations.
pub fn check_value_expression(ctx: &mut AnalysisContext<'_>, expr: &ast::Spanned<ast::Expr>) {
    match &expr.node {
        ast::Expr::Binary { lhs, op, rhs } => {
            check_value_expression(ctx, lhs);
            // Occurrence typing over a short-circuit operator: SurrealDB
            // evaluates the right operand of `A AND B` only where `A` held, and
            // of `A OR B` only where it did not, so the right operand — and
            // `check_binary`, which re-reads the rhs kind via its function
            // analysis — is checked with what the left proves applied to a
            // scoped child env. `(x != NONE) AND f(x)`,
            // `(subject IN $a) AND fn::test(subject)`, `$x IS NONE OR f($x)`
            // and chains all see the narrowed subject in the right operand's
            // function-call arguments. The child env is discarded after, so the
            // narrowing never leaks past the operator. A child env is forked
            // only when the left proves something, so an ordinary operator
            // keeps checking in the parent env (no re-recorded spans).
            let facts = crate::analyzer::expression::infer::short_circuit_facts(
                &op.node,
                &lhs.node,
                ctx.env(),
            );
            if let Some(facts) = facts {
                ctx.with_child_env(|ctx| {
                    crate::analyzer::flow::narrow::apply_facts(ctx, &facts);
                    check_value_expression(ctx, rhs);
                    check_binary(ctx, expr, lhs, &op.node, rhs);
                });
                return;
            }
            check_value_expression(ctx, rhs);
            check_binary(ctx, expr, lhs, &op.node, rhs);
        }
        ast::Expr::Prefix { op, expr: inner } => {
            check_value_expression(ctx, inner);
            if matches!(op.node, ast::PrefixOp::Neg | ast::PrefixOp::Pos) {
                if let Some(kind) = known_kind(ctx, inner) {
                    if !is_numeric(&kind) {
                        emit(
                            ctx,
                            expr.span,
                            2004,
                            format!(
                                "`-` can't be applied to a `{}`",
                                crate::render::render_offending(&kind, Some(&Kind::Number))
                            ),
                        );
                    }
                }
            }
        }
        ast::Expr::Array(elements) => {
            for element in elements {
                check_value_expression(ctx, element);
            }
            check_mixed_array(ctx, expr, elements);
        }
        ast::Expr::Object(fields) => {
            for (_, value) in fields {
                check_value_expression(ctx, value);
            }
            check_geometry_shape(ctx, expr, fields);
        }
        ast::Expr::Call(call) => {
            // Same argument position the inference side marks: the checking
            // walk reaches the argument's SELECT too, and both must agree or
            // the finding comes back from whichever path did not know.
            let cardinality =
                crate::analyzer::data::select::reads_only_cardinality(call.path.node.as_str());
            ctx.with_cardinality_position(cardinality, |ctx| {
                for arg in &call.args {
                    check_value_expression(ctx, arg);
                }
            });
        }
        ast::Expr::Cast { ty, expr: inner } => {
            check_value_expression(ctx, inner);
            check_cast(ctx, expr, ty, inner);
        }
        ast::Expr::Idiom(idiom) => check_idiom_positions(ctx, idiom),
        ast::Expr::Literal(literal) => check_literal_content(ctx, expr, literal),
        ast::Expr::Subquery(statement) => check_subquery(ctx, statement),
        // A block nested inside an expression, or standing as a whole clause
        // value (`DEFINE FIELD … VALUE { … }`). `expr_fact` routes a block it
        // is handed directly, but every other caller pairs pure inference —
        // which reports a block as partial and walks nothing — with this
        // function, so those blocks were analyzed by nobody at all.
        //
        // The child scope is the boundary: the block's `LET`s are resolved
        // inside it and cannot escape into the enclosing expression.
        ast::Expr::Block(block) => {
            ctx.with_child_env(|ctx| {
                crate::analyzer::flow::block::analyze_block(ctx, block);
            });
        }
        ast::Expr::Closure(closure) => check_closure(ctx, closure),
        // A param, a table name, a record id and an unlowerable node carry no
        // position of their own. Exhaustive rather than a catch-all: a new
        // expression form must decide whether it has inner obligations, and
        // the `_ => {}` that used to stand here is what made `(THROW …)`,
        // `({ … })` and every closure body silently unchecked.
        ast::Expr::Param(_)
        | ast::Expr::Table(_)
        | ast::Expr::RecordId { .. }
        | ast::Expr::Partial(_) => {}
    }
}

/// A closure's body, checked in the closure's own scope with its parameters
/// bound — the internal half of the rule, for the one construct that had
/// *neither* half. No closure body has ever been checked: inference walks one
/// (to type the return), and the checking walk's catch-all swallowed it.
///
/// The parameters are bound at their **declared** kinds, `any` where a closure
/// declares none. That is the honest answer for a closure read on its own: the
/// element kind a `.filter()` or `.map()` applies it to is a fact about the
/// call site, and reading it here would be inventing one. An `any` parameter
/// proves nothing, so a body that depends on it stays silent — prove or stay
/// silent, applied to a position that used to be silent unconditionally.
fn check_closure(ctx: &mut AnalysisContext<'_>, closure: &ast::Closure) {
    ctx.with_child_env(|ctx| {
        let declared = crate::analyzer::expression::infer::closure_param_kinds(closure, ctx);
        crate::analyzer::expression::infer::bind_closure_params(closure, &declared, ctx);
        // `expr_fact` is the entry that pairs inference with checking, and it
        // routes a block body through the block analyzer itself.
        crate::analyzer::expression::expr_fact(ctx, &closure.body);
    });
}

/// The inner obligations of a statement used as a value — `(THROW …)`,
/// `(RETURN …)`, a bare expression statement.
///
/// A construct is checked against its own contracts in its own environment,
/// and its result against the enclosing contract; the second half is the
/// caller's, and this is the first. The statement kinds whose own analyzer
/// runs on the inference route (`statement_value_kind`) already do it —
/// re-entering them here would analyze the same statement twice — so what is
/// left is exactly the kinds that route reads with pure inference, which never
/// checks anything.
fn check_subquery(ctx: &mut AnalysisContext<'_>, statement: &ast::Spanned<ast::Statement>) {
    match &statement.node {
        ast::Statement::Return(stmt) => {
            if let Some(value) = &stmt.value {
                check_value_expression(ctx, value);
            }
        }
        ast::Statement::Throw(stmt) => {
            if let Some(value) = &stmt.value {
                check_value_expression(ctx, value);
            }
        }
        ast::Statement::Expr(inner) => check_value_expression(ctx, inner),
        _ => {}
    }
}

/// Literal content must be valid for its kind (2032); regex literals must
/// compile (2031).
fn check_literal_content(
    ctx: &mut AnalysisContext<'_>,
    whole: &ast::Spanned<ast::Expr>,
    literal: &ast::Literal,
) {
    use std::str::FromStr;
    let problem = match literal {
        ast::Literal::Datetime(text) => surrealdb_types::Datetime::from_str(text)
            .is_err()
            .then(|| (2032, format!("`{text}` is not a valid datetime"))),
        ast::Literal::Duration(text) => surrealdb_types::Duration::from_str(text)
            .is_err()
            .then(|| (2032, format!("`{text}` is not a valid duration"))),
        ast::Literal::Uuid(text) => surrealdb_types::Uuid::from_str(text)
            .is_err()
            .then(|| (2032, format!("`{text}` is not a valid uuid"))),
        ast::Literal::Regex(pattern) => {
            use std::str::FromStr;
            surrealdb_types::Regex::from_str(pattern)
                .is_err()
                .then(|| (2031, format!("`{pattern}` is not a valid regex")))
        }
        _ => None,
    };
    if let Some((code, message)) = problem {
        emit(ctx, whole.span, code, message);
    }
}

/// The cast contract: the conversion must be able to succeed (2008) — by
/// kind (mirroring SurrealDB's `Cast` impls: every non-string target
/// accepts itself and strings; numerics interconvert; bytes also accept
/// arrays) or, when the operand is a known constant, by value.
fn check_cast(
    ctx: &mut AnalysisContext<'_>,
    whole: &ast::Spanned<ast::Expr>,
    ty: &ast::Spanned<ast::TypeExpr>,
    inner: &ast::Spanned<ast::Expr>,
) {
    // The named-type contract (2007): the cast target must be a type
    // SurrealQL knows. Generous by design — every kind name the engine
    // accepts is listed, so only genuine misspellings fire.
    const TYPE_NAMES: &[&str] = &[
        "any",
        "array",
        "bool",
        "bytes",
        "datetime",
        "decimal",
        "duration",
        "either",
        "file",
        "float",
        "function",
        "future",
        "geometry",
        "int",
        "literal",
        "none",
        "null",
        "number",
        "object",
        "option",
        "point",
        "range",
        "record",
        "references",
        "regex",
        "set",
        "string",
        "uuid",
    ];
    let named = match &ty.node {
        ast::TypeExpr::Name(name) => Some(name),
        ast::TypeExpr::Parameterized { name, .. } => Some(name),
        _ => None,
    };
    if let Some(name) = named {
        if !TYPE_NAMES.contains(&name.node.to_ascii_lowercase().as_str()) {
            let mut finding = surrealguard_diagnostics::catalog::finding(
                SourceSpan::new(ctx.source().clone(), name.span),
                2007,
                format!("`{}` is not a known type", name.node),
            );
            if let Some(nearest) = crate::suggest::closest(&name.node, TYPE_NAMES.iter().copied()) {
                finding = finding.with_help(format!("did you mean `{nearest}`?"));
            }
            ctx.emit(finding);
            return;
        }
    }

    let Some(target) = crate::analyzer::expression::infer::cast_target_kind(&ty.node) else {
        return;
    };
    let fact = infer_expression_fact(inner, ctx);

    // Value-proven: the constant can be converted right now.
    if let Some(surrealdb_types::Value::String(text)) = &fact.value {
        use std::str::FromStr;
        let fails = match target {
            Kind::Int => text.trim().parse::<i64>().is_err(),
            Kind::Float => text.trim().parse::<f64>().is_err(),
            Kind::Number => {
                text.trim().parse::<i64>().is_err() && text.trim().parse::<f64>().is_err()
            }
            Kind::Datetime => surrealdb_types::Datetime::from_str(text).is_err(),
            Kind::Duration => surrealdb_types::Duration::from_str(text).is_err(),
            Kind::Uuid => surrealdb_types::Uuid::from_str(text).is_err(),
            Kind::Bool => !matches!(text.as_str(), "true" | "false"),
            _ => false,
        };
        if fails {
            emit(
                ctx,
                whole.span,
                2008,
                format!(
                    "`{text}` can't be cast to `{}`",
                    crate::render_kind(&target)
                ),
            );
        }
        return;
    }

    // Kind-proven: no value of the operand's kind converts.
    let Some(kind) = fact.kind else {
        return;
    };
    if kind == Kind::Any {
        return;
    }
    let base = crate::kinds::literal_base_kind(&kind).unwrap_or_else(|| kind.clone());
    let possible = match &target {
        Kind::String | Kind::Any => true,
        Kind::Bool => matches!(base, Kind::Bool | Kind::String),
        Kind::Int | Kind::Float | Kind::Decimal | Kind::Number => {
            is_numeric(&base) || matches!(base, Kind::String)
        }
        Kind::Datetime => matches!(base, Kind::Datetime | Kind::String),
        Kind::Duration => matches!(base, Kind::Duration | Kind::String),
        Kind::Uuid => matches!(base, Kind::Uuid | Kind::String),
        Kind::Bytes => matches!(base, Kind::Bytes | Kind::String | Kind::Array(_, _)),
        Kind::Record(_) => matches!(base, Kind::Record(_) | Kind::String),
        _ => true,
    };
    if !possible {
        emit(
            ctx,
            whole.span,
            2008,
            format!(
                "a `{}` can't be cast to `{}`",
                crate::render::render_offending(&kind, Some(&target)),
                crate::render_kind(&target)
            ),
        );
    }
}

/// Walks an idiom's parts with the kind in hand, enforcing position
/// contracts: index/filter/splat apply to collections (2030), and method
/// calls resolve on their receiver's kind (5001).
fn check_idiom_positions(ctx: &mut AnalysisContext<'_>, idiom: &ast::Idiom) {
    use crate::analyzer::expression::infer::idiom_prefix_kinds;

    // Idioms containing graph steps get the traversal contract checks,
    // from wherever they stand (the row table). The row-table origin only
    // holds when the traversal starts *from the row*: an idiom rooted at a
    // leading value (`$auth->employee_of`, `(SELECT ...)->edge`) traverses
    // from that value's record type, not the row, so attributing the step to
    // the row table would misfire — e.g. flagging a valid `$auth->employee_of`
    // in `SELECT ... FROM member_of WHERE in = $auth->employee_of` as
    // stepping from `member_of`. That leading value is generally an open
    // record (a session param), so its true origin is unknown; leave it
    // lenient rather than misattribute.
    if idiom
        .parts
        .iter()
        .any(|part| matches!(part.node, ast::IdiomPart::Graph { .. }))
    {
        let rooted_at_value = matches!(
            idiom.parts.first().map(|p| &p.node),
            Some(ast::IdiomPart::Start(_))
        );
        if !rooted_at_value {
            if let Some(table) = ctx.row_table() {
                let name = table.name.clone();
                crate::analyzer::data::graph::check_graph_idiom(ctx, &name, idiom);
            }
        }
        return;
    }

    // A graph tail's destructure is checked where the traversal is resolved
    // (`validate_graph_destructure`), which is why this stands after the
    // early return above rather than before it.
    check_field_after_destructure(ctx, &idiom.parts);

    for (part, receiver) in idiom_prefix_kinds(idiom, ctx) {
        let Some(receiver) = receiver else {
            continue;
        };
        if receiver == Kind::Any {
            continue;
        }
        match &part.node {
            // Index/filter needs a collection — and a union of
            // collections is one: `[[1, 2], [3]]` is
            // `array<int, 2> | array<int, 1>`, indexable on every arm.
            ast::IdiomPart::Index(_) | ast::IdiomPart::Where(_) | ast::IdiomPart::Last
                if !crate::analyzer::expression::infer::is_indexable_kind(&receiver) =>
            {
                emit(
                    ctx,
                    part.span,
                    2030,
                    format!(
                        "a `{}` can't be indexed or filtered — it is not a collection",
                        crate::render::render_offending(&receiver, None)
                    ),
                );
                return;
            }
            // The splat is the same contract with one more admissible
            // receiver: a record link, which `.*` expands rather than
            // indexes. A scalar still has nothing to splat.
            ast::IdiomPart::All
                if !crate::analyzer::expression::infer::is_splattable_kind(&receiver) =>
            {
                emit(
                    ctx,
                    part.span,
                    2030,
                    format!(
                        "a `{}` has nothing to splat — `.*` needs a collection, an object or a record link",
                        crate::render::render_offending(&receiver, None)
                    ),
                );
                return;
            }
            // The receiver is splattable but the row it names has no declared
            // shape, so the splat's type is `any`. Say so — a bare `any` with
            // no finding reads as "analyzed successfully".
            ast::IdiomPart::All => {
                if let Some(table) = crate::analyzer::expression::infer::unexpandable_splat_table(
                    &receiver,
                    ctx.schema(),
                ) {
                    emit_fieldless_splat(ctx, part.span, &table);
                }
            }
            ast::IdiomPart::Method { name, args } => {
                // A method's arguments are expressions in their own right, and
                // the `Expr::Call` arm above checks a plain call's. This arm
                // read their kinds and checked none of them, which is why
                // `$rows.filter(|$r| …)` was the one call spelling whose
                // closure body nothing reached.
                for arg in args {
                    check_value_expression(ctx, arg);
                }
                let arg_kinds: Vec<Kind> = std::iter::once(receiver.clone())
                    .chain(
                        args.iter()
                            .map(|arg| infer_expression_fact(arg, ctx).kind.unwrap_or(Kind::Any)),
                    )
                    .collect();
                if crate::analyzer::expression::infer::method_result(
                    &receiver, &name.node, &arg_kinds, args, ctx,
                )
                .is_none()
                {
                    emit(
                        ctx,
                        name.span,
                        5001,
                        format!(
                            "`{}` has no method `{}`",
                            crate::render::render_offending(&receiver, None),
                            name.node
                        ),
                    );
                    return;
                }
            }
            _ => {}
        }
    }
}

fn check_binary(
    ctx: &mut AnalysisContext<'_>,
    whole: &ast::Spanned<ast::Expr>,
    lhs: &ast::Spanned<ast::Expr>,
    op: &ast::BinaryOp,
    rhs: &ast::Spanned<ast::Expr>,
) {
    use ast::BinaryOp as Op;
    check_index_backed_operator(ctx, whole, lhs, op);
    constrain_comparison_params(ctx, op, lhs, rhs);

    let (Some(left), Some(right)) = (known_kind(ctx, lhs), known_kind(ctx, rhs)) else {
        return;
    };

    // Custom operators carry their own contracts (regex patterns,
    // membership) and never reach the arithmetic/comparison checks below.
    if let Op::Other(name) = op {
        check_regex_pattern_operand(ctx, name, rhs);
        check_empty_membership(ctx, whole, name, lhs, rhs);
        check_membership_kind(ctx, whole, name, &left, &right);
        return;
    }

    check_none_arithmetic(ctx, op, lhs, &left, rhs, &right);

    // One contract, one code: the operands must make sense together for
    // the operator (2004). Whether SurrealDB throws (arithmetic) or
    // silently kind-orders (comparisons) is irrelevant — tolerated misuse
    // is still misuse; surfacing it is this tool's entire purpose.
    //
    // Several shapes make an `=`/`!=` provably constant, all sharing the
    // 7005 always-false/true family:
    //   - a literal-union side compared against a constant outside the
    //     union (`WHEN $event = 'CRATE'`);
    //   - a `NONE`/`NULL` sentinel against a kind that structurally cannot
    //     be that sentinel (`option<datetime> = NULL`, `string = NONE`);
    //   - two record links whose declared table sets are disjoint
    //     (`record<file> = record<folder>`).
    if matches!(op, Op::Eq | Op::NotEq)
        && (literal_union_excludes(ctx, &left, rhs)
            || literal_union_excludes(ctx, &right, lhs)
            || sentinel_mismatch(lhs, rhs, &right)
            || sentinel_mismatch(rhs, lhs, &left)
            || disjoint_records(&left, &right))
    {
        emit(
            ctx,
            whole.span,
            7005,
            format!(
                "`{}` between `{}` and `{}` is always {}",
                op_text(op),
                crate::render::render_offending(&left, Some(&right)),
                crate::render::render_offending(&right, Some(&left)),
                if matches!(op, Op::Eq) {
                    "false"
                } else {
                    "true"
                },
            ),
        );
        return;
    }

    let violated = match op {
        Op::Add | Op::Sub | Op::Mul | Op::Div => binary_result_kind(op, &left, &right).is_none(),
        // NONE/NULL comparisons are the idiomatic existence checks;
        // numerics widen. Truthiness makes AND/OR legal on anything; ??
        // accepts anything by design.
        Op::Eq | Op::NotEq | Op::Lt | Op::LtEq | Op::Gt | Op::GtEq => !comparable(&left, &right),
        _ => false,
    };
    if violated {
        emit(
            ctx,
            whole.span,
            2004,
            format!(
                "`{}` can't combine a `{}` and a `{}`",
                op_text(op),
                crate::render::render_offending(&left, Some(&right)),
                crate::render::render_offending(&right, Some(&left))
            ),
        );
    }
}

/// Index-backed operators (`@@`/`@...` full-text, `<|...>` vector) require a
/// supporting index on the left-hand field (1027).
fn check_index_backed_operator(
    ctx: &mut AnalysisContext<'_>,
    whole: &ast::Spanned<ast::Expr>,
    lhs: &ast::Spanned<ast::Expr>,
    op: &ast::BinaryOp,
) {
    let ast::BinaryOp::Other(name) = op else {
        return;
    };
    let needs = if name == "@@" || name.to_ascii_uppercase().starts_with("@") {
        Some(crate::schema::IndexKind::Search)
    } else if name.starts_with("<|") {
        Some(crate::schema::IndexKind::Vector)
    } else {
        None
    };
    let Some(required) = needs else {
        return;
    };
    let ast::Expr::Idiom(idiom) = &lhs.node else {
        return;
    };
    let Some(segments) = crate::analyzer::expression::infer::plain_field_segments(idiom) else {
        return;
    };
    let Some(table) = ctx.row_table() else {
        return;
    };
    let path = segments.join(".");
    let table_name = table.name.clone();
    let covered = table
        .indexes
        .values()
        .any(|index| index.kind == required && index.covers(&path));
    if !covered {
        let (what, clause) = match required {
            crate::schema::IndexKind::Search => {
                ("a SEARCH ANALYZER index", "SEARCH ANALYZER <analyzer>")
            }
            _ => ("an MTREE or HNSW index", "MTREE DIMENSION <n>"),
        };
        ctx.emit(
            surrealguard_diagnostics::catalog::finding(
                SourceSpan::new(ctx.source().clone(), whole.span),
                1027,
                format!("`{name}` needs {what} on `{path}`, and there isn't one"),
            )
            .with_help(format!(
                "define one: `DEFINE INDEX ... ON {table_name} FIELDS {path} {clause}`"
            )),
        );
    }
}

/// A comparison against an unbound parameter constrains it to the other
/// side's kind.
fn constrain_comparison_params(
    ctx: &mut AnalysisContext<'_>,
    op: &ast::BinaryOp,
    lhs: &ast::Spanned<ast::Expr>,
    rhs: &ast::Spanned<ast::Expr>,
) {
    use ast::BinaryOp as Op;
    if !matches!(
        op,
        Op::Eq | Op::NotEq | Op::Lt | Op::LtEq | Op::Gt | Op::GtEq
    ) {
        return;
    }
    let sides = [(lhs, rhs), (rhs, lhs)];
    for (param_side, typed_side) in sides {
        if let ast::Expr::Param(param) = &param_side.node {
            if let Some(kind) = known_kind(ctx, typed_side) {
                let span = SourceSpan::new(ctx.source().clone(), param_side.span);
                // A comparison requires only comparability, not that the param
                // BE the other side's kind — reconcile with comparison rules.
                ctx.constrain_param_comparable(param, span, kind, None);
            }
        }
    }
}

/// A `~`/`!~` pattern operand is a plain string and must compile as a
/// regex (2031).
fn check_regex_pattern_operand(
    ctx: &mut AnalysisContext<'_>,
    name: &str,
    rhs: &ast::Spanned<ast::Expr>,
) {
    if !matches!(name, "~" | "!~") {
        return;
    }
    if let Some(surrealdb_types::Value::String(pattern)) = infer_expression_fact(rhs, ctx).value {
        use std::str::FromStr;
        if surrealdb_types::Regex::from_str(&pattern).is_err() {
            emit(
                ctx,
                rhs.span,
                2031,
                format!("`{pattern}` is not a valid regex"),
            );
        }
    }
}

/// Membership (`IN`/`INSIDE`/`CONTAINS`) against a provably empty collection
/// never matches (7006).
fn check_empty_membership(
    ctx: &mut AnalysisContext<'_>,
    whole: &ast::Spanned<ast::Expr>,
    name: &str,
    lhs: &ast::Spanned<ast::Expr>,
    rhs: &ast::Spanned<ast::Expr>,
) {
    if !matches!(
        name.to_ascii_uppercase().as_str(),
        "IN" | "INSIDE" | "CONTAINS"
    ) {
        return;
    }
    let collection = if name.eq_ignore_ascii_case("contains") {
        lhs
    } else {
        rhs
    };
    if let Some(surrealdb_types::Value::Array(values)) =
        infer_expression_fact(collection, ctx).value
    {
        if values.is_empty() {
            emit(
                ctx,
                whole.span,
                7006,
                "membership test against an empty collection is always false".to_string(),
            );
        }
    }
}

/// Arithmetic on a possibly-NONE value fails whenever the NONE side shows up
/// at runtime (2015).
fn check_none_arithmetic(
    ctx: &mut AnalysisContext<'_>,
    op: &ast::BinaryOp,
    lhs: &ast::Spanned<ast::Expr>,
    left: &Kind,
    rhs: &ast::Spanned<ast::Expr>,
    right: &Kind,
) {
    use ast::BinaryOp as Op;
    if !matches!(op, Op::Add | Op::Sub | Op::Mul | Op::Div) {
        return;
    }
    for (side, kind) in [(lhs, left), (rhs, right)] {
        if let Kind::Either(variants) = kind {
            if variants
                .iter()
                .any(|v| matches!(v, Kind::None | Kind::Null))
                && variants
                    .iter()
                    .any(|v| !matches!(v, Kind::None | Kind::Null))
            {
                ctx.emit(
                    surrealguard_diagnostics::catalog::finding(
                        SourceSpan::new(ctx.source().clone(), side.span),
                        2015,
                        "this value may be NONE here, and arithmetic on NONE fails".to_string(),
                    )
                    .with_help(format!(
                        "it is `{}`; coalesce with `?? <default>` or narrow before the operation",
                        crate::render::render_offending(kind, None)
                    )),
                );
            }
        }
    }
}

/// Whether `kind` is a union/literal of string constants that provably
/// excludes the other side's constant value.
fn literal_union_excludes(
    ctx: &mut AnalysisContext<'_>,
    kind: &Kind,
    other: &ast::Spanned<ast::Expr>,
) -> bool {
    let members: Vec<&str> = match kind {
        Kind::Either(variants) => variants
            .iter()
            .filter_map(|variant| match variant {
                Kind::Literal(surrealdb_types::KindLiteral::String(s)) => Some(s.as_str()),
                _ => None,
            })
            .collect(),
        Kind::Literal(surrealdb_types::KindLiteral::String(s)) => vec![s.as_str()],
        _ => return false,
    };
    let expected_len = match kind {
        Kind::Either(variants) => variants.len(),
        _ => 1,
    };
    if members.is_empty() || members.len() != expected_len {
        return false;
    }
    let Some(surrealdb_types::Value::String(value)) = infer_expression_fact(other, ctx).value
    else {
        return false;
    };
    !members.contains(&value.as_str())
}

/// The sentinel kind an operand is a bare literal of, if it is `NONE`/`NULL`.
fn sentinel_literal(expr: &ast::Spanned<ast::Expr>) -> Option<Kind> {
    use crate::analyzer::facts::{eval, Bindings, ConstValue, Term};
    match eval(&expr.node, Bindings::NONE) {
        Term::Const(ConstValue::None) => Some(Kind::None),
        Term::Const(ConstValue::Null) => Some(Kind::Null),
        _ => None,
    }
}

/// Whether `kind` can ever hold the given sentinel (`NONE`/`NULL`). `Any` and
/// a union with a matching variant admit it; every other closed kind does not
/// — an `option<T>` is `none | T`, so it admits NONE but never NULL.
fn kind_admits_sentinel(kind: &Kind, sentinel: &Kind) -> bool {
    match kind {
        Kind::Any => true,
        Kind::Either(variants) => variants
            .iter()
            .any(|variant| kind_admits_sentinel(variant, sentinel)),
        other => other == sentinel,
    }
}

/// C1: a `= NONE`/`= NULL` (or `!=`) whose other side has a known kind that
/// structurally excludes that sentinel can never match. `sentinel_side` is the
/// operand that must be the bare literal; `other_side`/`other_kind` are the
/// counterpart expression and its known (non-`Any`) kind.
fn sentinel_mismatch(
    sentinel_side: &ast::Spanned<ast::Expr>,
    other_side: &ast::Spanned<ast::Expr>,
    other_kind: &Kind,
) -> bool {
    let Some(sentinel) = sentinel_literal(sentinel_side) else {
        return false;
    };
    if matches!(other_kind, Kind::Any) {
        return false;
    }
    // NONE is genuinely reachable for a param regardless of its declared kind:
    // write-time/computed contexts bind `$value`/`$before`/`$after` and the
    // idiomatic `ASSERT $value = NONE OR ...` guard depends on it. So the NONE
    // branch fires only against a non-param operand (a stored field read, which
    // is never NONE for a required kind). NULL is never inhabited by any known
    // kind, so it fires everywhere.
    if matches!(sentinel, Kind::None) && matches!(other_side.node, ast::Expr::Param(_)) {
        return false;
    }
    !kind_admits_sentinel(other_kind, &sentinel)
}

/// Two non-empty record table sets share no table, so no record id of one can
/// equal a record id of the other. An empty set is the unconstrained
/// `record<>` (any table) and never counts as disjoint.
fn tables_disjoint(left: &[surrealdb_types::Table], right: &[surrealdb_types::Table]) -> bool {
    !left.is_empty() && !right.is_empty() && !left.iter().any(|table| right.contains(table))
}

/// C3: `=`/`!=` between two record links whose declared table sets are
/// non-empty and disjoint is provably constant.
fn disjoint_records(left: &Kind, right: &Kind) -> bool {
    match (left, right) {
        (Kind::Record(lt), Kind::Record(rt)) => tables_disjoint(lt, rt),
        _ => false,
    }
}

/// C2/D2: whether a value of kind `a` could ever equal a value of kind `b` —
/// the membership-element predicate. Same kind, numeric-compatible, records
/// whose table sets overlap, or any variant of a union. `Any`/`NONE`/`NULL`
/// are never provably-incomparable, so they short-circuit to comparable.
/// Kept separate from [`comparable`] (which treats all record pairs as
/// orderable) so it can back both the membership check here and the future
/// ASSERT `$value IN [...]` check without disturbing the 2004 ordering path.
fn comparable_element(a: &Kind, b: &Kind) -> bool {
    if a == b
        || (is_numeric(a) && is_numeric(b))
        || matches!(a, Kind::Any | Kind::None | Kind::Null)
        || matches!(b, Kind::Any | Kind::None | Kind::Null)
    {
        return true;
    }
    match (a, b) {
        (Kind::Record(at), Kind::Record(bt)) => !tables_disjoint(at, bt),
        (Kind::Either(variants), other) | (other, Kind::Either(variants)) => variants
            .iter()
            .any(|variant| comparable_element(variant, other)),
        _ => {
            let a = crate::kinds::literal_base_kind(a).unwrap_or_else(|| a.clone());
            let b = crate::kinds::literal_base_kind(b).unwrap_or_else(|| b.clone());
            a == b
        }
    }
}

/// C2: `IN`/`INSIDE`/`CONTAINS` whose element operand can never equal any
/// element of a known-element collection is always false (7006 family). Fires
/// only when the collection kind is `Array(e)`/`Set(e)` with a concrete `e`
/// and the element side has a concrete kind — bare `array`/`set` (element
/// `Any`) and unknown operands never trigger.
fn check_membership_kind(
    ctx: &mut AnalysisContext<'_>,
    whole: &ast::Spanned<ast::Expr>,
    name: &str,
    left: &Kind,
    right: &Kind,
) {
    if !matches!(
        name.to_ascii_uppercase().as_str(),
        "IN" | "INSIDE" | "CONTAINS"
    ) {
        return;
    }
    // CONTAINS: the left side is the collection. IN/INSIDE: the right side is.
    let (collection, element) = if name.eq_ignore_ascii_case("contains") {
        (left, right)
    } else {
        (right, left)
    };
    let elem = match collection {
        Kind::Array(elem, _) | Kind::Set(elem, _) => elem.as_ref(),
        _ => return,
    };
    if matches!(elem, Kind::Any) || matches!(element, Kind::Any) {
        return;
    }
    if !comparable_element(element, elem) {
        emit(
            ctx,
            whole.span,
            7006,
            format!(
                "membership of `{}` in a collection of `{}` is always false",
                crate::render::render_offending(element, Some(elem)),
                crate::render::render_offending(elem, Some(element))
            ),
        );
    }
}

fn comparable(left: &Kind, right: &Kind) -> bool {
    if left == right
        || (is_numeric(left) && is_numeric(right))
        || matches!(left, Kind::None | Kind::Null)
        || matches!(right, Kind::None | Kind::Null)
    {
        return true;
    }
    // A table value compares equal to its name as a string — `type::table($x)
    // = 'folder'` is the idiomatic record-discriminant guard.
    if matches!(
        (left, right),
        (Kind::Table(_), Kind::String | Kind::Table(_)) | (Kind::String, Kind::Table(_))
    ) {
        return true;
    }
    // Records compare across tables (id ordering); literals compare with
    // their base kind; unions compare if any variant does.
    match (left, right) {
        (Kind::Record(_), Kind::Record(_)) => true,
        (Kind::Either(variants), other) | (other, Kind::Either(variants)) => {
            variants.iter().any(|variant| comparable(variant, other))
        }
        _ => {
            let left = crate::kinds::literal_base_kind(left);
            let right = crate::kinds::literal_base_kind(right);
            match (left, right) {
                (None, None) => false,
                (l, r) => {
                    let l = l.unwrap_or(Kind::Any);
                    let r = r.unwrap_or(Kind::Any);
                    l == r || matches!(l, Kind::Any) || matches!(r, Kind::Any)
                }
            }
        }
    }
}

/// An array literal whose elements have several kinds usually wants a
/// review (7003) — heterogeneous arrays are legal but rarely intended.
fn check_mixed_array(
    ctx: &mut AnalysisContext<'_>,
    whole: &ast::Spanned<ast::Expr>,
    elements: &[ast::Spanned<ast::Expr>],
) {
    let mut kinds: Vec<Kind> = Vec::new();
    for element in elements {
        let Some(kind) = known_kind(ctx, element) else {
            return;
        };
        let base = crate::kinds::literal_base_kind(&kind).unwrap_or(kind);
        if !kinds.contains(&base) {
            kinds.push(base);
        }
    }
    if kinds.len() > 1 {
        emit(
            ctx,
            whole.span,
            7003,
            format!(
                "array literal mixes kinds: {}",
                kinds
                    .iter()
                    .map(|k| format!("`{k}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
    }
}

/// An object with `type` + `coordinates` keys is GeoJSON-shaped; its
/// `type` must name a geometry kind (2036).
fn check_geometry_shape(
    ctx: &mut AnalysisContext<'_>,
    whole: &ast::Spanned<ast::Expr>,
    fields: &[(ast::Spanned<String>, ast::Spanned<ast::Expr>)],
) {
    const TYPES: &[&str] = &[
        "Point",
        "LineString",
        "Polygon",
        "MultiPoint",
        "MultiLineString",
        "MultiPolygon",
        "GeometryCollection",
    ];
    let has_coordinates = fields
        .iter()
        .any(|(key, _)| key.node == "coordinates" || key.node == "geometries");
    if !has_coordinates {
        return;
    }
    let Some((_, type_value)) = fields.iter().find(|(key, _)| key.node == "type") else {
        return;
    };
    let ast::Expr::Literal(ast::Literal::String(name)) = &type_value.node else {
        return;
    };
    if !TYPES.contains(&name.as_str()) {
        emit(
            ctx,
            whole.span,
            2036,
            format!("`{name}` is not a GeoJSON geometry type"),
        );
    }
}

/// A kind that is actually informative: `Any` never violates anything.
fn known_kind(ctx: &mut AnalysisContext<'_>, expr: &ast::Spanned<ast::Expr>) -> Option<Kind> {
    let kind = infer_expression_fact(expr, ctx).kind?;
    (kind != Kind::Any).then_some(kind)
}

fn op_text(op: &ast::BinaryOp) -> &'static str {
    use ast::BinaryOp as Op;
    match op {
        Op::Add => "+",
        Op::Sub => "-",
        Op::Mul => "*",
        Op::Div => "/",
        Op::Eq => "=",
        Op::NotEq => "!=",
        Op::Lt => "<",
        Op::LtEq => "<=",
        Op::Gt => ">",
        Op::GtEq => ">=",
        Op::And => "AND",
        Op::Or => "OR",
        Op::NullCoalesce => "??",
        _ => "?",
    }
}

/// A field read out of the object a `.{…}` just built.
///
/// A destructure's key set is exactly what the author wrote, so a name that is
/// not among them is *provably* absent — there is no schema to consult and
/// nothing to be lenient about. The engine returns NONE (3.0.5:
/// `SELECT author.{name}.age FROM post` → `{r: null}`), and the site's type
/// would otherwise be a silent `any`. Same contract and code as any other
/// missing field (1002).
///
/// Deliberately narrow: only a selection whose every entry is a bare field
/// name is checked, because only then is the key set unambiguous, and only the
/// one field immediately behind the destructure — past that the walk has left
/// the shape it can vouch for.
pub(crate) fn check_field_after_destructure(
    ctx: &mut AnalysisContext<'_>,
    parts: &[ast::Spanned<ast::IdiomPart>],
) {
    let mut keys: Option<Vec<String>> = None;
    for part in parts {
        match &part.node {
            ast::IdiomPart::Destructure(selected) => {
                keys = selected
                    .iter()
                    .map(|sub| match sub.node.parts.as_slice() {
                        [only] => match &only.node {
                            ast::IdiomPart::Field(name) => Some(name.clone()),
                            _ => None,
                        },
                        _ => None,
                    })
                    .collect();
            }
            ast::IdiomPart::Field(name) => {
                if let Some(keys) = keys.take() {
                    if !keys.contains(name) {
                        emit(
                            ctx,
                            part.span,
                            1002,
                            match keys.len() {
                                0 => format!(
                                    "the empty destructure `.{{}}` selected nothing, so there is no `{name}` to read"
                                ),
                                _ => format!(
                                    "the destructure selected {}, so it has no `{name}`",
                                    keys.iter()
                                        .map(|key| format!("`{key}`"))
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                ),
                            },
                        );
                        return;
                    }
                }
            }
            _ => keys = None,
        }
    }
}

/// A `.*` whose row has no declared shape: the site's type is `any`, and the
/// author deserves to know why rather than read `any` as "analyzed fine".
/// Same contract, same code (7008) as a bare `SELECT *` on that table.
pub(crate) fn emit_fieldless_splat(
    ctx: &mut AnalysisContext<'_>,
    span: surrealguard_syntax::span::ByteRange,
    table: &str,
) {
    let source_span = SourceSpan::new(ctx.source().clone(), span);
    ctx.emit(
        surrealguard_diagnostics::catalog::finding(
            source_span,
            7008,
            format!("`{table}` has no declared fields, so `.*` expands to nothing typed"),
        )
        .with_help(format!(
            "add `DEFINE FIELD` declarations to `{table}` for full analysis"
        )),
    );
}

fn emit(
    ctx: &mut AnalysisContext<'_>,
    span: surrealguard_syntax::span::ByteRange,
    code: u16,
    message: String,
) {
    let span = SourceSpan::new(ctx.source().clone(), span);
    ctx.emit(surrealguard_diagnostics::catalog::finding(
        span, code, message,
    ));
}

#[cfg(test)]
mod tests {
    use crate::analysis::{analyze_query, Workspace};

    /// The rendered code strings (e.g. `"L7005"`) a query produces.
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

    // ---- parentheses are a semantic no-op, so every rule still applies ----

    #[test]
    fn parentheses_report_exactly_what_the_bare_expression_reports() {
        // `(expr)` IS `expr` in SurrealQL. While it lowered to an opaque
        // subquery node, every consumer that matches on expression shape fell
        // off its `_ =>` arm, so a user who parenthesized for readability lost
        // inference, narrowing, field validation and every expression-level
        // diagnostic with no signal at all.
        const SCHEMA: &str = concat!(
            "DEFINE TABLE t SCHEMAFULL;\n",
            "DEFINE FIELD name ON t TYPE string;\n",
            "DEFINE FIELD nick ON t TYPE option<string>;\n",
        );
        for (bare, parenthesized) in [
            // a projection
            (
                "SELECT VALUE nick + 1 FROM t;",
                "SELECT VALUE (nick + 1) FROM t;",
            ),
            // a method call
            (
                "SELECT VALUE name.nomethod() FROM t;",
                "SELECT VALUE (name.nomethod()) FROM t;",
            ),
            // a WHERE predicate
            (
                "SELECT name FROM t WHERE nofield = 1;",
                "SELECT name FROM t WHERE (nofield = 1);",
            ),
            // a FROM target
            ("SELECT nofield FROM t;", "SELECT nofield FROM (t);"),
            // a function argument
            ("RETURN string::len(1);", "RETURN string::len((1));"),
            // a LET value
            ("LET $x = 1 + 'a';", "LET $x = (1 + 'a');"),
            // a RETURN value
            ("RETURN 1 + 'a';", "RETURN (1 + 'a');"),
            // nested parentheses
            ("RETURN 1 + 'a';", "RETURN ((1 + 'a'));"),
            // parentheses around a subquery
            (
                "RETURN (SELECT nofield FROM t);",
                "RETURN ((SELECT nofield FROM t));",
            ),
        ] {
            let mut bare_codes = codes(&format!("{SCHEMA}{bare}"));
            let mut parenthesized_codes = codes(&format!("{SCHEMA}{parenthesized}"));
            bare_codes.sort();
            parenthesized_codes.sort();
            assert!(
                !bare_codes.is_empty(),
                "{bare:?} must report something for this pair to prove anything"
            );
            assert_eq!(
                bare_codes, parenthesized_codes,
                "{parenthesized:?} must report what {bare:?} reports"
            );
        }
    }

    #[test]
    fn a_valid_parenthesized_query_still_reports_nothing() {
        // The other direction: seeing through the parentheses must not invent
        // findings on valid SurrealQL. The first two were an error- and a
        // warning-severity false positive while `(…)` was opaque — the
        // aggregate-promotion and UNIQUE-index recognizers both failed to see
        // their own shape through the wrapper.
        const SCHEMA: &str = concat!(
            "DEFINE TABLE t SCHEMAFULL;\n",
            "DEFINE FIELD name  ON t TYPE string;\n",
            "DEFINE FIELD email ON t TYPE option<string>;\n",
            "DEFINE FIELD price ON t TYPE number;\n",
            "DEFINE INDEX t_email ON t FIELDS email UNIQUE;\n",
        );
        for query in [
            "SELECT (math::sum(price)) * 2 AS total FROM t GROUP ALL;",
            "SELECT name FROM ONLY t WHERE (email = 'a@b.c');",
            "SELECT name FROM (t);",
            "RETURN (1 + 2) * 3;",
            "RETURN (/* grouped */ 1);",
        ] {
            // Default-allowed hints (7014's whole-table SELECT) fire on the
            // bare spelling too and say nothing about parentheses; only
            // error/warning severities are the false positives at issue.
            let reported: Vec<String> = codes(&format!("{SCHEMA}{query}"))
                .into_iter()
                .filter(|code| code.starts_with('E') || code.starts_with('W'))
                .collect();
            assert!(
                reported.is_empty(),
                "{query:?} is valid SurrealQL, got {reported:?}"
            );
        }
    }

    // ---- a statement used as a value is checked inside, too ----

    #[test]
    fn a_statement_subquery_is_checked_inside() {
        // Compositional checking, the internal half: `check_value_expression`
        // had a `_ => {}` that swallowed every `Expr::Subquery`, while the
        // inference route read the statement kinds below with pure inference —
        // which never checks. So a value-position `THROW`/`RETURN` carried
        // whatever it liked.
        for query in [
            "RETURN (THROW 1 + 'a');",
            "RETURN (RETURN 1 + 'a');",
            "RETURN 1 ?? (THROW 'a' + 1);",
        ] {
            assert!(
                fires(query, "E2004"),
                "{query:?} must report its operand mismatch: {:?}",
                codes(query)
            );
        }
    }

    #[test]
    fn a_statement_subquery_that_owns_its_analyzer_reports_once() {
        // The other half of the rule: a statement kind whose own analyzer runs
        // on the inference route must not be re-entered here, or every finding
        // inside a subquery SELECT would be raised twice.
        let query = concat!(
            "DEFINE TABLE t SCHEMAFULL;\n",
            "DEFINE FIELD name ON t TYPE string;\n",
            "RETURN (SELECT VALUE name.nomethod() FROM t);\n",
        );
        assert_eq!(
            codes(query).iter().filter(|code| *code == "E5001").count(),
            1,
            "codes: {:?}",
            codes(query)
        );
    }

    #[test]
    fn a_block_used_as_a_value_is_analyzed_wherever_it_stands() {
        // A block is checked when `expr_fact` is handed one directly; every
        // other caller pairs *pure* inference — which reports a block as
        // partial and walks nothing — with `check_value_expression`, whose
        // catch-all swallowed it. So a block was analyzed by nobody at all in
        // a clause value, inside another expression, or in parentheses.
        for query in [
            // parenthesized (a genuine statement subquery)
            "RETURN ({ RETURN 1 + 'a'; });",
            // its `LET`s resolve inside the block
            "RETURN ({ LET $y = 1; $y + 'a' });",
            // nested inside another expression
            "RETURN 1 + { RETURN 'a'.nomethod(); };",
        ] {
            let expected = if query.contains("nomethod") {
                "E5001"
            } else {
                "E2004"
            };
            assert!(
                fires(query, expected),
                "{query:?} must report {expected}: {:?}",
                codes(query)
            );
        }

        // …and in a clause value, where nothing reached it before.
        let clause = concat!(
            "DEFINE TABLE t SCHEMAFULL;\n",
            "DEFINE FIELD v ON t VALUE { RETURN 1 + 'a'; };\n",
        );
        assert!(fires(clause, "E2004"), "codes: {:?}", codes(clause));
    }

    #[test]
    fn a_blocks_bindings_do_not_escape_it() {
        // The child scope is the boundary a block *is*. A `LET` inside one
        // must not be visible after it, or checking a block would leak
        // bindings into the expression that contains it.
        let query = "RETURN ({ LET $inner = 1; $inner }) + $inner;";
        // `$inner` outside the block is an unbound host parameter, not the
        // block's binding — so the addition proves nothing and stays silent.
        assert!(!fires(query, "E2004"), "codes: {:?}", codes(query));
    }

    #[test]
    fn a_closure_body_is_checked_in_the_closures_own_scope() {
        // No closure body had ever been checked, in any spelling: inference
        // walks one to type the return, and the checking walk's catch-all
        // swallowed `Expr::Closure` whole.
        for query in [
            // a closure bound to a name
            "LET $f = |$v: int| $v + 'a';",
            // a block body
            "LET $f = |$v: int| { RETURN $v + 'a'; };",
            // as a call argument
            "RETURN array::map([1, 2], |$v: int| $v + 'a');",
            // …and as a METHOD argument, which nothing checked at all
            "RETURN [1, 2].map(|$v: int| $v + 'a');",
        ] {
            assert!(
                fires(query, "E2004"),
                "{query:?} must report its operand mismatch: {:?}",
                codes(query)
            );
        }
    }

    #[test]
    fn an_undeclared_closure_parameter_proves_nothing() {
        // The parameters are bound at their DECLARED kinds. The element kind a
        // `.map()` applies the closure to is a fact about the call site, and
        // reading it here would be inventing one — so an undeclared parameter
        // is `any` and a body that depends on it stays silent. Checking a
        // position that used to be unconditionally silent is exactly where a
        // false-positive wave would come from.
        let query = "RETURN array::map([1, 2], |$v| $v + 'a');";
        assert!(!fires(query, "E2004"), "codes: {:?}", codes(query));

        // A body that does NOT depend on the parameter is still checked.
        let independent = "RETURN array::map([1, 2], |$v| 'a'.nomethod());";
        assert!(
            fires(independent, "E5001"),
            "codes: {:?}",
            codes(independent)
        );
    }

    // ---- a check inside a guarded region reads the guarded kind ----

    /// A function whose parameter is an object with one optional field. A
    /// declared param is the shortest way to a narrowable field path whose
    /// declared kind the checker can see.
    fn guarded(params: &str, body: &str) -> String {
        format!("DEFINE FUNCTION fn::f({params}) {{ {body} RETURN 0; }};")
    }

    #[test]
    fn a_guarded_field_path_is_checked_at_the_kind_the_guard_proved() {
        // Inference resolved `$r.nick` to `string` inside the guard for as long
        // as narrowing has existed; *checking* read the declared kind, so the
        // analyzer reported a method missing from a kind it had itself proven
        // the value could not have — an error-severity false positive on code
        // it proved safe (F31). An error aborts codegen for a whole workspace.
        let query = guarded(
            "$r: { nick: option<string> }",
            "IF $r.nick != NONE { RETURN $r.nick.len(); };",
        );
        assert!(
            !fires(&query, "E5001"),
            "the guard proved `$r.nick` is a `string`: {:?}",
            codes(&query)
        );
    }

    #[test]
    fn a_guard_on_one_path_says_nothing_about_another() {
        // The negative half, and the reason a whole `Place` is compared:
        // suppressing a TRUE finding is the one way reading through narrowing
        // can go wrong, and a prefix lookup that matched loosely would do it.
        for (params, guard, read) in [
            // a sibling binding
            (
                "$a: { nick: option<string> }, $b: { nick: option<string> }",
                "$a.nick",
                "$b.nick",
            ),
            // a sibling field of the same binding
            (
                "$a: { nick: option<string>, other: option<string> }",
                "$a.nick",
                "$a.other",
            ),
        ] {
            let query = guarded(
                params,
                &format!("IF {guard} != NONE {{ RETURN {read}.len(); }};"),
            );
            assert!(
                fires(&query, "E5001"),
                "`{guard}` proves nothing about `{read}`: {:?}",
                codes(&query)
            );
        }
    }

    #[test]
    fn a_narrowed_prefix_carries_into_a_read_beneath_it() {
        // Longest prefix, not exact key. The guard names `$r.inner`; the read
        // names `$r.inner.nick`. Stepping an `option<{...}>` through a field
        // access fails, so without the prefix the whole path was unresolvable —
        // the narrowing sat in the environment, unreachable from the read.
        let query = guarded(
            "$r: { inner: option<{ nick: string }> }",
            "IF $r.inner != NONE { RETURN $r.inner.nick.nomethod(); };",
        );
        assert!(
            fires(&query, "E5001"),
            "the guard makes `$r.inner.nick` a `string`: {:?}",
            codes(&query)
        );
    }

    // ---- sentinel guards eliminate only their own sentinel ----

    #[test]
    fn an_in_expression_none_guard_leaves_null_reachable() {
        // `$x != NONE AND f($x)` runs `f` exactly where the left held — and
        // `RETURN NULL != NONE` is `true` on the engine, so a NULL reaches
        // `f`. `string::len(NULL)` is a runtime error ("Expected `string` but
        // found `NULL`"), so this must fire; narrowing `$x` to `string` here
        // hid a genuine failure.
        const NULLABLE: &str = "DEFINE FUNCTION fn::f($x: option<string | null>) { RETURN ";
        for guard in [
            "$x != NONE AND string::len($x) > 0",
            "$x = NONE OR string::len($x) > 0",
        ] {
            let query = format!("{NULLABLE}{guard}; }};");
            assert!(fires(&query, "E5002"), "{guard}: {:?}", codes(&query));
        }

        // The must-still-narrow boundary: with no `null` in the declared type
        // the same guards make `$x` a plain `string`, and nothing fires.
        const OPTIONAL: &str = "DEFINE FUNCTION fn::f($x: option<string>) { RETURN ";
        for guard in [
            "$x != NONE AND string::len($x) > 0",
            "$x = NONE OR string::len($x) > 0",
        ] {
            let query = format!("{OPTIONAL}{guard}; }};");
            assert!(!fires(&query, "E5002"), "{guard}: {:?}", codes(&query));
        }
    }

    // ---- `??` strips NONE, so a coalesced option is a plain value (2004) ----

    #[test]
    fn coalesce_strips_both_sentinels_unlike_a_none_guard() {
        // `??` is not a guard: it falls through on BOTH sentinels (`NONE ?? 'd'`
        // and `NULL ?? 'd'` both yield `'d'` on the engine), so a coalesced
        // `option<string | null>` really is a plain `string`.
        let query = concat!(
            "DEFINE TABLE t SCHEMAFULL;\n",
            "DEFINE FIELD note ON t TYPE option<string | null>;\n",
            "SELECT VALUE (note ?? 'd') + '!' FROM t;\n",
        );
        assert!(!fires(query, "E2004"), "codes: {:?}", codes(query));
    }

    #[test]
    fn coalesced_option_is_usable_in_arithmetic() {
        // The user did exactly what the 2004 help text asks for; the result
        // must not still be an `option<string>`.
        let query = concat!(
            "DEFINE TABLE t SCHEMAFULL;\n",
            "DEFINE FIELD nick ON t TYPE option<string>;\n",
            "SELECT VALUE (nick ?? 'd') + '!' FROM t;\n",
        );
        assert!(!fires(query, "E2004"), "codes: {:?}", codes(query));
    }

    #[test]
    fn coalesce_does_not_hide_a_genuine_operand_mismatch() {
        // The must-still-fire boundary: coalescing narrows the left side to
        // `string`, which still cannot be added to an `int`.
        let query = concat!(
            "DEFINE TABLE t SCHEMAFULL;\n",
            "DEFINE FIELD nick ON t TYPE option<string>;\n",
            "SELECT VALUE (nick ?? 'd') + 1 FROM t;\n",
        );
        assert!(fires(query, "E2004"), "codes: {:?}", codes(query));
    }

    // ---- index/filter and method contracts distribute over a union ----

    #[test]
    fn indexing_a_union_of_collections_is_not_a_contract_violation() {
        // `[[1, 2], [3]]` is `array<int, 2> | array<int, 1>`; both arms are
        // arrays, so indexing and `.len()` are valid SurrealQL.
        for query in [
            "RETURN { LET $a = [[1,2],[3]]; RETURN $a[0][0]; };",
            "RETURN { LET $a = [[1,2],[3]]; RETURN $a[0].len(); };",
            "RETURN { LET $a = [[1,2],['x']]; RETURN $a[0][0]; };",
        ] {
            let codes = codes(query);
            assert!(
                !codes.iter().any(|code| code == "E2030" || code == "E5001"),
                "codes for {query:?}: {codes:?}"
            );
        }
    }

    #[test]
    fn indexing_a_non_collection_still_fires() {
        // The must-still-fire boundaries: a scalar receiver, a union with a
        // definitely-non-collection arm, and a method no arm defines.
        let scalar = "RETURN { LET $a = 5; RETURN $a[0]; };";
        assert!(fires(scalar, "E2030"), "codes: {:?}", codes(scalar));

        let optional = concat!(
            "DEFINE TABLE t SCHEMAFULL;\n",
            "DEFINE FIELD maybe ON t TYPE option<array<string>>;\n",
            "SELECT VALUE maybe[0] FROM t;\n",
        );
        assert!(fires(optional, "E2030"), "codes: {:?}", codes(optional));

        let method = "RETURN { LET $a = [[1,2],[3]]; RETURN $a[0].bogusmethod(); };";
        assert!(fires(method, "E5001"), "codes: {:?}", codes(method));
    }

    /// The schema the splat-contract tests share.
    const LINKS: &str = concat!(
        "DEFINE TABLE user SCHEMAFULL;\n",
        "DEFINE FIELD name ON user TYPE string;\n",
        "DEFINE TABLE post SCHEMAFULL;\n",
        "DEFINE FIELD title ON post TYPE string;\n",
        "DEFINE FIELD author ON post TYPE record<user>;\n",
        "DEFINE FIELD editors ON post TYPE array<record<user>>;\n",
        "DEFINE FIELD maybe_author ON post TYPE option<record<user>>;\n",
    );

    #[test]
    fn splatting_a_record_link_is_not_a_contract_violation() {
        // `.*` is not an index: on a link it names the row the link points at,
        // and the engine returns that row (verified on 3.0.5 —
        // `SELECT author.* FROM ONLY post:p1` -> `{author: {id, name}}`).
        // Reporting 2030 here refused valid SurrealQL.
        for tail in ["author.*", "author.*.name", "editors.*", "editors.*.name"] {
            let query = format!("{LINKS}SELECT VALUE {tail} FROM post;\n");
            let codes = codes(&query);
            assert!(
                !codes.iter().any(|code| code == "E2030"),
                "codes for {tail:?}: {codes:?}"
            );
        }
    }

    #[test]
    fn splatting_something_with_no_row_still_fires() {
        // The boundaries the splat keeps: a scalar has nothing to splat, and —
        // exactly as an index does — a union with one arm that has no row is a
        // contract violation, not a shape to guess through.
        let scalar = format!("{LINKS}SELECT VALUE title.* FROM post;\n");
        assert!(fires(&scalar, "E2030"), "codes: {:?}", codes(&scalar));

        let optional = format!("{LINKS}SELECT VALUE maybe_author.* FROM post;\n");
        assert!(fires(&optional, "E2030"), "codes: {:?}", codes(&optional));

        // A splat is the ONLY position a record admits: indexing one is still
        // a violation (the engine answers NONE).
        let indexed = format!("{LINKS}SELECT VALUE author[0] FROM post;\n");
        assert!(fires(&indexed, "E2030"), "codes: {:?}", codes(&indexed));
    }

    #[test]
    fn a_splat_with_no_row_shape_says_why() {
        // Prove-or-stay-silent applies to the type; the author staring at the
        // resulting `any` still deserves to know which table has nothing to
        // expand to. Same contract, same code, as a bare `SELECT *` on it.
        let query = concat!(
            "DEFINE TABLE opaque SCHEMALESS;\n",
            "DEFINE TABLE holder SCHEMAFULL;\n",
            "DEFINE FIELD link ON holder TYPE record<opaque>;\n",
            "SELECT VALUE link.* FROM holder;\n",
        );
        assert!(fires(query, "L7008"), "codes: {:?}", codes(query));
    }

    #[test]
    fn closure_taking_methods_do_not_fire_5001() {
        // Error-severity 5001 on valid SurrealQL: `$rows.map(|$o| …)` was
        // rejected while the identical `array::map($rows, |$o| …)` checked
        // clean. Method sugar and the function form are one call.
        let query = concat!(
            "DEFINE TABLE t SCHEMAFULL;\n",
            "DEFINE FIELD name ON t TYPE string;\n",
            "LET $rows = SELECT name FROM t;\n",
            "RETURN $rows.map(|$o| $o.name);\n",
            "RETURN $rows.filter(|$o| $o.name != '');\n",
            "RETURN [1, 2].fold(0, |$acc, $v| $acc + $v);\n",
            "RETURN [1, 2].reduce(|$acc, $v| $acc + $v);\n",
            "RETURN [1, 2].chain(|$v| $v);\n",
        );
        assert!(!fires(query, "E5001"), "codes: {:?}", codes(query));
    }

    #[test]
    fn a_builtin_returning_any_does_not_fire_5001() {
        // `record::id` returns an honest `any`; that is not "no such method".
        let query = concat!(
            "DEFINE TABLE t SCHEMAFULL;\n",
            "DEFINE FIELD owner ON t TYPE record<t>;\n",
            "SELECT owner.id() FROM t;\n",
        );
        assert!(!fires(query, "E5001"), "codes: {:?}", codes(query));
    }

    #[test]
    fn an_unknown_method_still_fires_5001() {
        // The counterpart: resolving by catalog existence must not accept
        // every name. Neither the family nor the generic set has these.
        for query in [
            "RETURN [1, 2].frobnicate();",
            "RETURN 'abc'.map(|$v| $v);",
            "RETURN (5).filter(|$v| $v);",
        ] {
            assert!(
                fires(query, "E5001"),
                "codes for {query:?}: {:?}",
                codes(query)
            );
        }
    }

    // ---- C1: comparison to NULL/NONE a kind cannot be (7005) ----

    #[test]
    fn c1_null_against_option_field_is_always_false() {
        // option<datetime> is `none | datetime` — it never holds NULL.
        let query = concat!(
            "DEFINE TABLE post SCHEMAFULL;\n",
            "DEFINE FIELD deleted_at ON post TYPE option<datetime>;\n",
            "SELECT * FROM post WHERE deleted_at = NULL;\n",
        );
        assert!(fires(query, "L7005"), "codes: {:?}", codes(query));
    }

    #[test]
    fn c1_none_against_required_field_is_always_false() {
        let query = concat!(
            "DEFINE TABLE post SCHEMAFULL;\n",
            "DEFINE FIELD name ON post TYPE string;\n",
            "SELECT * FROM post WHERE name = NONE;\n",
        );
        assert!(fires(query, "L7005"), "codes: {:?}", codes(query));
    }

    #[test]
    fn c1_none_against_option_field_does_not_fire() {
        // The must-not-fire boundary: option<T> genuinely admits NONE.
        let query = concat!(
            "DEFINE TABLE post SCHEMAFULL;\n",
            "DEFINE FIELD deleted_at ON post TYPE option<datetime>;\n",
            "SELECT * FROM post WHERE deleted_at = NONE;\n",
        );
        assert!(!fires(query, "L7005"), "codes: {:?}", codes(query));
    }

    #[test]
    fn c1_value_param_equals_none_in_assert_does_not_fire() {
        // The idiomatic write-time guard: `$value` is genuinely NONE-reachable
        // in an ASSERT/VALUE body, so this must stay silent (workshop pattern).
        let query = concat!(
            "DEFINE TABLE country SCHEMAFULL;\n",
            "DEFINE FIELD timezones ON country TYPE array<string>\n",
            "  ASSERT $value = NONE OR array::len($value) = 0;\n",
        );
        assert!(!fires(query, "L7005"), "codes: {:?}", codes(query));
    }

    // ---- C2: IN/CONTAINS/INSIDE element-kind mismatch (7006) ----

    #[test]
    fn c2_scalar_element_mismatch_is_always_false() {
        let query = concat!(
            "DEFINE TABLE post SCHEMAFULL;\n",
            "DEFINE FIELD tags ON post TYPE array<string>;\n",
            "SELECT * FROM post WHERE tags CONTAINS 5;\n",
        );
        assert!(fires(query, "L7006"), "codes: {:?}", codes(query));
    }

    #[test]
    fn c2_disjoint_record_element_is_always_false() {
        let query = concat!(
            "DEFINE TABLE doc SCHEMAFULL;\n",
            "DEFINE FIELD editors ON doc TYPE array<record<user>>;\n",
            "SELECT * FROM doc WHERE post:1 IN editors;\n",
        );
        assert!(fires(query, "L7006"), "codes: {:?}", codes(query));
    }

    #[test]
    fn c2_comparable_element_does_not_fire() {
        // The must-not-fire boundary: a matching element kind.
        let query = concat!(
            "DEFINE TABLE post SCHEMAFULL;\n",
            "DEFINE FIELD tags ON post TYPE array<string>;\n",
            "SELECT * FROM post WHERE tags CONTAINS 'draft';\n",
        );
        assert!(!fires(query, "L7006"), "codes: {:?}", codes(query));
    }

    #[test]
    fn c2_bare_array_any_element_does_not_fire() {
        // A collection with an `Any` element makes no membership provable.
        let query = concat!(
            "DEFINE TABLE post SCHEMAFULL;\n",
            "DEFINE FIELD tags ON post TYPE array;\n",
            "SELECT * FROM post WHERE tags CONTAINS 5;\n",
        );
        assert!(!fires(query, "L7006"), "codes: {:?}", codes(query));
    }

    // ---- C3: `=`/`!=` between disjoint record links (7005) ----

    #[test]
    fn c3_disjoint_record_equality_is_always_false() {
        let query = concat!(
            "DEFINE TABLE edge SCHEMAFULL;\n",
            "DEFINE FIELD a ON edge TYPE record<file>;\n",
            "DEFINE FIELD b ON edge TYPE record<folder>;\n",
            "SELECT * FROM edge WHERE a = b;\n",
        );
        assert!(fires(query, "L7005"), "codes: {:?}", codes(query));
    }

    #[test]
    fn c3_overlapping_record_equality_does_not_fire() {
        // The must-not-fire boundary: table sets share `file`.
        let query = concat!(
            "DEFINE TABLE edge SCHEMAFULL;\n",
            "DEFINE FIELD a ON edge TYPE record<file>;\n",
            "DEFINE FIELD b ON edge TYPE record<file | folder>;\n",
            "SELECT * FROM edge WHERE a = b;\n",
        );
        assert!(!fires(query, "L7005"), "codes: {:?}", codes(query));
    }

    // ---- Index-backed operators: full-text `@@` and KNN `<|K|>` (1027) ----

    #[test]
    fn full_text_match_without_search_index_fires() {
        let query = concat!(
            "DEFINE TABLE post SCHEMAFULL;\n",
            "DEFINE FIELD body ON post TYPE string;\n",
            "SELECT * FROM post WHERE body @@ 'hello';\n",
        );
        assert!(fires(query, "E1027"), "codes: {:?}", codes(query));
    }

    #[test]
    fn full_text_match_with_search_index_does_not_fire() {
        // The must-not-fire boundary: a SEARCH index covers `body`.
        let query = concat!(
            "DEFINE TABLE post SCHEMAFULL;\n",
            "DEFINE FIELD body ON post TYPE string;\n",
            "DEFINE ANALYZER simple TOKENIZERS blank;\n",
            "DEFINE INDEX ft ON post FIELDS body SEARCH ANALYZER simple;\n",
            "SELECT * FROM post WHERE body @@ 'hello';\n",
        );
        assert!(!fires(query, "E1027"), "codes: {:?}", codes(query));
    }

    #[test]
    fn knn_vector_search_without_index_fires() {
        let query = concat!(
            "DEFINE TABLE doc SCHEMAFULL;\n",
            "DEFINE FIELD embedding ON doc TYPE array<float>;\n",
            "SELECT * FROM doc WHERE embedding <|3|> $q;\n",
        );
        assert!(fires(query, "E1027"), "codes: {:?}", codes(query));
    }

    #[test]
    fn knn_vector_search_with_hnsw_index_does_not_fire() {
        // The must-not-fire boundary: an HNSW vector index covers `embedding`.
        let query = concat!(
            "DEFINE TABLE doc SCHEMAFULL;\n",
            "DEFINE FIELD embedding ON doc TYPE array<float>;\n",
            "DEFINE INDEX hnsw_idx ON doc FIELDS embedding HNSW DIMENSION 3;\n",
            "SELECT * FROM doc WHERE embedding <|3|> $q;\n",
        );
        assert!(!fires(query, "E1027"), "codes: {:?}", codes(query));
    }

    #[test]
    fn knn_vector_search_with_mtree_index_does_not_fire() {
        // MTREE also backs KNN — the same Vector index kind.
        let query = concat!(
            "DEFINE TABLE doc SCHEMAFULL;\n",
            "DEFINE FIELD embedding ON doc TYPE array<float>;\n",
            "DEFINE INDEX mtree_idx ON doc FIELDS embedding MTREE DIMENSION 3;\n",
            "SELECT * FROM doc WHERE embedding <|3|> $q;\n",
        );
        assert!(!fires(query, "E1027"), "codes: {:?}", codes(query));
    }

    #[test]
    fn index_backed_operator_on_dynamic_lhs_does_not_fire() {
        // Prove-or-silent: a param LHS is not a resolvable field, so no finding.
        let query = concat!(
            "DEFINE TABLE post SCHEMAFULL;\n",
            "DEFINE FIELD body ON post TYPE string;\n",
            "SELECT * FROM post WHERE $needle @@ 'hello';\n",
        );
        assert!(!fires(query, "E1027"), "codes: {:?}", codes(query));
    }
}
