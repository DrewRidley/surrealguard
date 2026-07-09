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
pub(crate) fn check_value_expression(
    ctx: &mut AnalysisContext<'_>,
    expr: &ast::Spanned<ast::Expr>,
) {
    match &expr.node {
        ast::Expr::Binary { lhs, op, rhs } => {
            check_value_expression(ctx, lhs);
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
    let base = crate::semantic::literal_base_kind(&kind).unwrap_or_else(|| kind.clone());
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
        let base =
            crate::semantic::literal_base_kind(&receiver).unwrap_or_else(|| receiver.clone());
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
    // Index-backed operators need their supporting index (1027).
    if let Op::Other(name) = op {
        let needs = if name == "@@" || name.to_ascii_uppercase().starts_with("@") {
            Some(crate::schema::IndexKind::Search)
        } else if name.starts_with("<|") {
            Some(crate::schema::IndexKind::Vector)
        } else {
            None
        };
        if let Some(required) = needs {
            if let ast::Expr::Idiom(idiom) = &lhs.node {
                if let Some(segments) =
                    crate::analyzer::expression::infer::plain_field_segments(idiom)
                {
                    if let Some(table) = ctx.row_table() {
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
                                format!("`{}` on `{path}` needs {what} on that field", name),
                            );
                        }
                    }
                }
            }
        }
    }

    // A comparison against an unbound parameter constrains it to the
    // other side's kind.
    if matches!(
        op,
        Op::Eq | Op::NotEq | Op::Lt | Op::LtEq | Op::Gt | Op::GtEq
    ) {
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

    let (Some(left), Some(right)) = (known_kind(ctx, lhs), known_kind(ctx, rhs)) else {
        return;
    };

    // A `~` pattern must compile (2031) — the pattern side is a plain
    // string.
    if let Op::Other(name) = op {
        if matches!(name.as_str(), "~" | "!~") {
            if let Some(surrealdb_types::Value::String(pattern)) =
                infer_expression_fact(rhs, ctx).value
            {
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
    }

    // Membership against a provably empty collection never matches (7006).
    if let Op::Other(name) = op {
        if matches!(
            name.to_ascii_uppercase().as_str(),
            "IN" | "INSIDE" | "CONTAINS"
        ) {
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
        return;
    }

    // Arithmetic on a possibly-NONE value fails whenever the NONE side
    // shows up (2015).
    if matches!(op, Op::Add | Op::Sub | Op::Mul | Op::Div) {
        for (side, kind) in [(lhs, &left), (rhs, &right)] {
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

    // One contract, one code: the operands must make sense together for
    // the operator (2004). Whether SurrealDB throws (arithmetic) or
    // silently kind-orders (comparisons) is irrelevant — tolerated misuse
    // is still misuse; surfacing it is this tool's entire purpose.
    // A literal-union side compared against a constant outside the union
    // never matches (7005) — `WHEN $event = 'CRATE'`.
    if matches!(op, Op::Eq | Op::NotEq)
        && (literal_union_excludes(ctx, &left, rhs) || literal_union_excludes(ctx, &right, lhs))
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

fn comparable(left: &Kind, right: &Kind) -> bool {
    if left == right
        || (is_numeric(left) && is_numeric(right))
        || matches!(left, Kind::None | Kind::Null)
        || matches!(right, Kind::None | Kind::Null)
    {
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
            let left = crate::semantic::literal_base_kind(left);
            let right = crate::semantic::literal_base_kind(right);
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
        let base = crate::semantic::literal_base_kind(&kind).unwrap_or(kind);
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
