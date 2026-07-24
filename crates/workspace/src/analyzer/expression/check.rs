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
            // The right operand of `$x = NONE OR ...` / `$x != NONE AND ...`
            // runs only when `$x` is non-none, so check it (and the operator
            // itself, which re-reads the rhs kind) with `$x` narrowed.
            match crate::analyzer::expression::infer::none_guarded_path(&op.node, lhs) {
                Some(path) => {
                    crate::analyzer::expression::infer::with_guard_narrowed(&path, ctx, |ctx| {
                        check_value_expression(ctx, rhs);
                        check_binary(ctx, expr, lhs, &op.node, rhs);
                    });
                }
                None => {
                    check_value_expression(ctx, rhs);
                    check_binary(ctx, expr, lhs, &op.node, rhs);
                }
            }
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
                            format!("incompatible operand for `-`: `{kind}`"),
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
            for arg in &call.args {
                check_value_expression(ctx, arg);
            }
        }
        ast::Expr::Cast { ty, expr: inner } => {
            check_value_expression(ctx, inner);
            check_cast(ctx, expr, ty, inner);
        }
        ast::Expr::Idiom(idiom) => check_idiom_positions(ctx, idiom),
        ast::Expr::Literal(literal) => check_literal_content(ctx, expr, literal),
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
            ctx.emit(surrealguard_diagnostics::catalog::finding(
                SourceSpan::new(ctx.source().clone(), name.span),
                2007,
                format!("`{}` is not a type", name.node),
            ));
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
                format!("`{text}` can never convert to `{target}`"),
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
            format!("a `{kind}` can never convert to `{target}`"),
        );
    }
}

/// Walks an idiom's parts with the kind in hand, enforcing position
/// contracts: index/filter/splat apply to collections (2030), and method
/// calls resolve on their receiver's kind (5001).
fn check_idiom_positions(ctx: &mut AnalysisContext<'_>, idiom: &ast::Idiom) {
    use crate::analyzer::expression::infer::idiom_prefix_kinds;

    // Idioms containing graph steps get the traversal contract checks,
    // from wherever they stand (the row table).
    if idiom
        .parts
        .iter()
        .any(|part| matches!(part.node, ast::IdiomPart::Graph { .. }))
    {
        if let Some(table) = ctx.row_table() {
            let name = table.name.clone();
            crate::analyzer::data::graph::check_graph_idiom(ctx, &name, idiom);
        }
        return;
    }

    for (part, receiver) in idiom_prefix_kinds(idiom, ctx) {
        let Some(receiver) = receiver else {
            continue;
        };
        if receiver == Kind::Any {
            continue;
        }
        let base = crate::kinds::literal_base_kind(&receiver).unwrap_or_else(|| receiver.clone());
        match &part.node {
            ast::IdiomPart::Index(_)
            | ast::IdiomPart::Where(_)
            | ast::IdiomPart::All
            | ast::IdiomPart::Last
                if !matches!(base, Kind::Array(_, _) | Kind::Set(_, _) | Kind::Object) =>
            {
                emit(
                    ctx,
                    part.span,
                    2030,
                    format!("cannot index or filter a value of type `{receiver}`"),
                );
                return;
            }
            ast::IdiomPart::Method { name, args } => {
                let arg_kinds: Vec<Kind> = std::iter::once(receiver.clone())
                    .chain(
                        args.iter()
                            .map(|arg| infer_expression_fact(arg, ctx).kind.unwrap_or(Kind::Any)),
                    )
                    .collect();
                if crate::analyzer::expression::infer::method_result(
                    &receiver, &name.node, &arg_kinds, ctx,
                )
                .is_none()
                {
                    emit(
                        ctx,
                        name.span,
                        5001,
                        format!("no method `{}` on `{receiver}`", name.node),
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
                "`{}` between `{left}` and `{right}` is always {}",
                op_text(op),
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
                "incompatible operands for `{}`: `{left}` and `{right}`",
                op_text(op)
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
    let covered = table
        .indexes
        .values()
        .any(|index| index.kind == required && index.covers(&path));
    if !covered {
        let what = match required {
            crate::schema::IndexKind::Search => "a SEARCH ANALYZER index",
            _ => "an MTREE or HNSW index",
        };
        emit(
            ctx,
            whole.span,
            1027,
            format!("`{name}` on `{path}` needs {what} on that field"),
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
                ctx.constrain_param(param, span, kind, None);
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
                emit(
                    ctx,
                    side.span,
                    2015,
                    format!("this value may be NONE at runtime (`{kind}`)"),
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
    match &expr.node {
        ast::Expr::Literal(ast::Literal::None) => Some(Kind::None),
        ast::Expr::Literal(ast::Literal::Null) => Some(Kind::Null),
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
fn tables_disjoint(
    left: &[surrealdb_types::Table],
    right: &[surrealdb_types::Table],
) -> bool {
    !left.is_empty()
        && !right.is_empty()
        && !left.iter().any(|table| right.contains(table))
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
        (Kind::Either(variants), other) | (other, Kind::Either(variants)) => {
            variants.iter().any(|variant| comparable_element(variant, other))
        }
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
            format!("membership of `{element}` in a collection of `{elem}` is always false"),
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
        (Kind::Table(_), Kind::String | Kind::Table(_))
            | (Kind::String, Kind::Table(_))
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
}
