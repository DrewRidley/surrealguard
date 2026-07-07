//! Expression fact inference over the typed AST.
//!
//! Pure fact inference over `ast::Expr` — no source-text reading for
//! structure, no CST shapes. (`crate::expression` holds the node-based
//! equivalent that the remaining validators in `crate::semantic` still
//! use.)
//!
//! [`infer_expression_fact`] is pure over an [`InferScope`]; the
//! ctx-carrying [`super::analyze_expr`] adds what purity can't:
//! statement-position dispatch for blocks and parameter-use recording.

use surrealdb_types::{Kind, KindLiteral};
use surrealguard_syntax::ast;
use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::SourceSpan;

use crate::expression::{ExpressionFact, ExpressionValueClass, PartialReason};
use crate::schema::{SchemaIndex, TableDef};
use crate::statement_env::StatementEnv;

/// Everything pure inference can consult: the schema (record links,
/// subqueries, user-defined functions), the source text (naming only),
/// the row context, and the statement environment (`LET` bindings).
pub(crate) struct InferScope<'a> {
    pub source: &'a SourceId,
    pub text: &'a str,
    pub schema: &'a SchemaIndex,
    pub row_table: Option<&'a TableDef>,
    pub env: &'a StatementEnv,
}

impl<'a> InferScope<'a> {
    pub(crate) fn from_ctx(ctx: &'a crate::analyzer::context::AnalysisContext<'a>) -> Self {
        InferScope {
            source: ctx.source(),
            text: ctx.source_text(),
            schema: ctx.schema(),
            row_table: ctx.row_table(),
            env: ctx.env(),
        }
    }
}

pub(crate) fn infer_expression_fact(
    expr: &ast::Spanned<ast::Expr>,
    scope: &InferScope<'_>,
) -> ExpressionFact {
    let span = source_span(scope.source, expr.span);
    match &expr.node {
        ast::Expr::Literal(literal) => literal_fact(literal, span),
        ast::Expr::Param(name) => param_fact(name, span, scope.env),
        ast::Expr::Table(name) => scalar_fact(
            span,
            ExpressionValueClass::Literal,
            Kind::Table(vec![name.node.as_str().into()]),
        ),
        ast::Expr::RecordId { table, .. } => scalar_fact(
            span,
            ExpressionValueClass::Literal,
            Kind::Record(vec![table.node.as_str().into()]),
        ),
        ast::Expr::Idiom(idiom) => idiom_fact(idiom, span, scope),
        ast::Expr::Binary { lhs, op, rhs } => binary_fact(lhs, op, rhs, span, scope),
        ast::Expr::Prefix { op, expr } => prefix_fact(op, expr, span, scope),
        ast::Expr::Object(fields) => object_fact(fields, span, scope),
        ast::Expr::Array(elements) => array_fact(elements, span, scope),
        ast::Expr::Call(call) => call_fact(call, span, scope),
        ast::Expr::Cast { ty, .. } => cast_fact(ty, span),
        ast::Expr::Subquery(inner) => {
            let fact = ExpressionFact::new(span, ExpressionValueClass::Subquery);
            match statement_value_kind(inner, scope) {
                Some(kind) => fact.with_kind(kind),
                None => fact.with_partial(PartialReason::UnsupportedSyntax("SubQuery".into())),
            }
        }
        // Blocks thread an environment through their statements, which needs
        // the ctx path (`analyze_expr`); pure inference reports them as
        // partial rather than guessing.
        ast::Expr::Block(_) => partial_fact(span, ExpressionValueClass::Block, "Block".into()),
        ast::Expr::Closure(closure) => closure_fact(closure, span, scope),
        ast::Expr::Partial(partial) => partial_fact(
            span,
            ExpressionValueClass::Unknown,
            partial.cst_kind.clone(),
        ),
    }
}

/// The response kind of a statement used as a value (subqueries,
/// `FROM (SELECT ...)` sources). Covers the statement kinds whose response
/// typing is pure; environment-threading statements need the ctx path.
pub(crate) fn statement_value_kind(
    stmt: &ast::Spanned<ast::Statement>,
    scope: &InferScope<'_>,
) -> Option<Kind> {
    let (source, text, schema, env) = (scope.source, scope.text, scope.schema, scope.env);
    let kind = match &stmt.node {
        ast::Statement::Select(s) => {
            crate::analyzer::data::select::select_response_kind(s, source, text, schema, env)
        }
        ast::Statement::Create(s) => {
            crate::analyzer::data::create::create_response_kind(s, source, text, schema, env)
        }
        ast::Statement::Update(s) => {
            crate::analyzer::data::update::update_response_kind(s, source, text, schema, env)
        }
        ast::Statement::Upsert(s) => {
            crate::analyzer::data::upsert::upsert_response_kind(s, source, text, schema, env)
        }
        ast::Statement::Delete(s) => {
            crate::analyzer::data::delete::delete_response_kind(s, source, text, schema, env)
        }
        ast::Statement::Insert(s) => {
            crate::analyzer::data::insert::insert_response_kind(s, source, text, schema, env)
        }
        ast::Statement::Relate(s) => {
            crate::analyzer::data::relate::relate_response_kind(s, source, text, schema, env)
        }
        ast::Statement::Return(s) => match &s.value {
            Some(value) => infer_expression_fact(value, scope)
                .kind
                .unwrap_or(Kind::Any),
            None => Kind::None,
        },
        ast::Statement::Expr(e) => infer_expression_fact(e, scope).kind.unwrap_or(Kind::Any),
        _ => return None,
    };
    Some(kind)
}

/// A closure value's own type: `Kind::Function(params, return)`. The
/// return kind comes from the declaration, or from the body inferred with
/// the declared parameter kinds — call sites re-infer with their actual
/// argument kinds ([`closure_return_kind`]).
fn closure_fact(
    closure: &ast::Closure,
    span: SourceSpan,
    scope: &InferScope<'_>,
) -> ExpressionFact {
    let param_kinds: Vec<Kind> = closure
        .params
        .iter()
        .map(|(_, ty)| declared_kind(ty.as_ref(), scope).unwrap_or(Kind::Any))
        .collect();
    let return_kind = closure_return_kind(closure, &param_kinds, scope);

    ExpressionFact::new(span, ExpressionValueClass::Literal)
        .with_kind(Kind::Function(Some(param_kinds), return_kind.map(Box::new)))
}

fn declared_kind(ty: Option<&ast::Spanned<ast::TypeExpr>>, scope: &InferScope<'_>) -> Option<Kind> {
    crate::schema::kind_from_type_expr(&ty?.node, scope.text).kind
}

/// The closure's return kind when invoked with `arg_kinds` — the declared
/// return type when present, otherwise the body inferred with each
/// parameter bound to its argument kind (falling back to the declared
/// parameter type, then `Any`).
pub(crate) fn closure_return_kind(
    closure: &ast::Closure,
    arg_kinds: &[Kind],
    scope: &InferScope<'_>,
) -> Option<Kind> {
    if let Some(declared) = declared_kind(closure.return_ty.as_ref(), scope) {
        return Some(declared);
    }

    let mut env = scope.env.fork_child_scope();
    for (index, (name, ty)) in closure.params.iter().enumerate() {
        let kind = match arg_kinds.get(index) {
            Some(kind) if *kind != Kind::Any => kind.clone(),
            _ => declared_kind(ty.as_ref(), scope).unwrap_or(Kind::Any),
        };
        let mut fact = ExpressionFact::new(
            SourceSpan::new(scope.source.clone(), name.span),
            ExpressionValueClass::Variable,
        );
        fact.kind = Some(kind);
        env.define_let(name.node.clone(), fact);
    }

    let body_scope = InferScope {
        source: scope.source,
        text: scope.text,
        schema: scope.schema,
        row_table: scope.row_table,
        env: &env,
    };
    match &closure.body.node {
        ast::Expr::Block(block) => pure_block_kind(block, &body_scope),
        _ => infer_expression_fact(&closure.body, &body_scope).kind,
    }
}

/// The value of a block in pure inference: threads `LET` bindings through
/// a child scope and returns on `RETURN` or the final statement's value.
/// Environment-mutating statements beyond `LET` (IF/FOR with effects) are
/// out of pure reach.
fn pure_block_kind(block: &ast::Block, scope: &InferScope<'_>) -> Option<Kind> {
    let mut env = scope.env.fork_child_scope();
    let mut last = Kind::None;

    for statement in &block.statements {
        let inner = InferScope {
            source: scope.source,
            text: scope.text,
            schema: scope.schema,
            row_table: scope.row_table,
            env: &env,
        };
        match &statement.node {
            ast::Statement::Let(stmt) => {
                let fact = infer_expression_fact(&stmt.value, &inner);
                env.define_let(stmt.name.node.clone(), fact);
            }
            ast::Statement::Return(stmt) => {
                return match &stmt.value {
                    Some(value) => infer_expression_fact(value, &inner).kind,
                    None => Some(Kind::None),
                };
            }
            _ => last = statement_value_kind(statement, &inner)?,
        }
    }
    Some(last)
}

fn literal_fact(literal: &ast::Literal, span: SourceSpan) -> ExpressionFact {
    let kind = match literal {
        ast::Literal::Int(_) => Kind::Int,
        ast::Literal::Float(_) => Kind::Float,
        ast::Literal::Decimal => Kind::Decimal,
        ast::Literal::String(_) => Kind::String,
        ast::Literal::Bool(_) => Kind::Bool,
        ast::Literal::None => Kind::None,
        ast::Literal::Null => Kind::Null,
        ast::Literal::Duration => Kind::Duration,
        ast::Literal::Datetime => Kind::Datetime,
        ast::Literal::Uuid => Kind::Uuid,
        ast::Literal::Regex => Kind::Regex,
    };
    let mut fact = scalar_fact(span, ExpressionValueClass::Literal, kind);
    fact.value = const_literal_value(literal);
    fact
}

/// The literal's static value, for the variants whose value the AST
/// retains. (Duration/datetime/uuid literals keep only their kind.)
fn const_literal_value(literal: &ast::Literal) -> Option<surrealdb_types::Value> {
    use surrealdb_types::Value;
    let value = match literal {
        ast::Literal::String(value) => Value::String(value.clone()),
        ast::Literal::Int(value) => Value::Number(surrealdb_types::Number::Int(*value)),
        ast::Literal::Float(value) => Value::Number(surrealdb_types::Number::Float(*value)),
        ast::Literal::Bool(value) => Value::Bool(*value),
        ast::Literal::None => Value::None,
        ast::Literal::Null => Value::Null,
        _ => return None,
    };
    Some(value)
}

fn param_fact(name: &str, span: SourceSpan, env: &StatementEnv) -> ExpressionFact {
    if let Some(bound) = env.let_fact(name) {
        let mut fact = ExpressionFact::new(span, ExpressionValueClass::Variable);
        fact.kind = bound.kind.clone();
        fact.value = bound.value.clone();
        return fact;
    }

    let mut fact = ExpressionFact::new(span, ExpressionValueClass::Variable)
        .with_partial(PartialReason::Unresolved);
    fact.dependencies.params.push(name.to_string());
    fact
}

// ---------------------------------------------------------------------------
// Idioms
// ---------------------------------------------------------------------------

fn idiom_fact(idiom: &ast::Idiom, span: SourceSpan, scope: &InferScope<'_>) -> ExpressionFact {
    let mut fact = ExpressionFact::new(span, ExpressionValueClass::FieldPath);

    if let Some(segments) = plain_field_segments(idiom) {
        fact.dependencies.field_paths.push(segments.join("."));
    }

    // Graph traversals are rooted in the row context and resolve through
    // the schema's relations.
    if matches!(
        idiom.parts.first().map(|p| &p.node),
        Some(ast::IdiomPart::Graph { .. })
    ) {
        let Some(table) = scope.row_table else {
            return fact.with_partial(PartialReason::Unresolved);
        };
        return match crate::analyzer::data::select::graph_projection_kind(
            &table.name,
            idiom,
            scope.schema,
            false,
        ) {
            Some(kind) => fact.with_kind(kind),
            None => fact.with_partial(PartialReason::Unresolved),
        };
    }

    match step_idiom_kind(idiom, scope) {
        Some(kind) => fact.with_kind(kind),
        None => fact.with_partial(PartialReason::Unresolved),
    }
}

/// The segments of an idiom made purely of `Field` parts, if it is one.
pub(crate) fn plain_field_segments(idiom: &ast::Idiom) -> Option<Vec<String>> {
    idiom
        .parts
        .iter()
        .map(|part| match &part.node {
            ast::IdiomPart::Field(name) => Some(name.clone()),
            _ => None,
        })
        .collect()
}

/// Walks an idiom part by part, tracking the kind of the value so far:
/// row fields, `LET`-bound starts (`$user.name`), record links (stepping
/// through the schema), literal objects, collection indexes, and method
/// calls dispatched by receiver kind.
fn step_idiom_kind(idiom: &ast::Idiom, scope: &InferScope<'_>) -> Option<Kind> {
    let mut parts = idiom.parts.iter();
    let mut current: Kind = match &parts.next()?.node {
        ast::IdiomPart::Start(expr) => infer_expression_fact(expr, scope).kind?,
        ast::IdiomPart::Field(name) => {
            let table = scope.row_table?;
            crate::analyzer::data::select::kind_for_path(table, std::slice::from_ref(name))?
        }
        _ => return None,
    };

    for part in parts {
        current = match &part.node {
            ast::IdiomPart::Field(name) => field_of_kind(&current, name, scope.schema)?,
            ast::IdiomPart::Index(_) => match current {
                Kind::Array(element, _) | Kind::Set(element, _) => *element,
                _ => return None,
            },
            // `.*`, `[WHERE ...]`, and `?.` preserve the value's kind.
            ast::IdiomPart::All | ast::IdiomPart::Where(_) | ast::IdiomPart::Optional => current,
            ast::IdiomPart::Last => match current {
                Kind::Array(element, _) | Kind::Set(element, _) => *element,
                _ => return None,
            },
            ast::IdiomPart::Method { name, args } => {
                let arg_kinds: Vec<Kind> = std::iter::once(current.clone())
                    .chain(
                        args.iter()
                            .map(|arg| infer_expression_fact(arg, scope).kind.unwrap_or(Kind::Any)),
                    )
                    .collect();
                method_return_kind(&current, &name.node, &arg_kinds, scope)?
            }
            ast::IdiomPart::Destructure(selected) => {
                let mut fields = std::collections::BTreeMap::new();
                for sub in selected {
                    let segments = plain_field_segments(&sub.node)?;
                    let mut kind = current.clone();
                    for segment in &segments {
                        kind = field_of_kind(&kind, segment, scope.schema)?;
                    }
                    fields.insert(segments.join("."), kind);
                }
                Kind::Literal(KindLiteral::Object(fields))
            }
            ast::IdiomPart::Start(_)
            | ast::IdiomPart::Graph { .. }
            | ast::IdiomPart::Partial(_) => {
                return None;
            }
        };
    }
    Some(current)
}

/// The kind of `value.field`, stepping through closed objects and record
/// links (via the schema).
fn field_of_kind(value: &Kind, field: &str, schema: &SchemaIndex) -> Option<Kind> {
    match value {
        Kind::Literal(KindLiteral::Object(fields)) => fields.get(field).cloned(),
        Kind::Record(targets) => {
            let [target] = targets.as_slice() else {
                return None;
            };
            let table = schema.tables.get(&target.to_string())?;
            crate::analyzer::data::select::kind_for_path(table, &[field.to_string()])
        }
        // Field access distributes over collections (`friends.name`).
        Kind::Array(element, max_len) => {
            let stepped = field_of_kind(element, field, schema)?;
            Some(Kind::Array(Box::new(stepped), *max_len))
        }
        _ => None,
    }
}

/// Method calls dispatch to the receiver kind's function family:
/// `array.len()` is `array::len(array)`.
fn method_return_kind(
    receiver: &Kind,
    method: &str,
    args: &[Kind],
    scope: &InferScope<'_>,
) -> Option<Kind> {
    let base = crate::semantic::literal_base_kind(receiver).unwrap_or_else(|| receiver.clone());
    let family = match base {
        Kind::Array(_, _) => "array",
        Kind::Set(_, _) => "set",
        Kind::String => "string",
        Kind::Object => "object",
        Kind::Duration => "duration",
        Kind::Datetime => "time",
        Kind::Bytes => "bytes",
        Kind::Record(_) => "record",
        Kind::Int | Kind::Float | Kind::Decimal | Kind::Number => "math",
        _ => return None,
    };
    let kind =
        crate::analyzer::function::builtin_return_kind(scope, &format!("{family}::{method}"), args);
    (kind != Kind::Any).then_some(kind)
}

// ---------------------------------------------------------------------------
// Compound expressions
// ---------------------------------------------------------------------------

fn binary_fact(
    lhs: &ast::Spanned<ast::Expr>,
    op: &ast::Spanned<ast::BinaryOp>,
    rhs: &ast::Spanned<ast::Expr>,
    span: SourceSpan,
    scope: &InferScope<'_>,
) -> ExpressionFact {
    let lhs_fact = infer_expression_fact(lhs, scope);
    let rhs_fact = infer_expression_fact(rhs, scope);

    let mut fact = ExpressionFact::new(span, ExpressionValueClass::Unknown);
    merge_dependencies(&mut fact, lhs_fact.dependencies);
    merge_dependencies(&mut fact, rhs_fact.dependencies);
    fact.partial.extend(lhs_fact.partial.iter().cloned());
    fact.partial.extend(rhs_fact.partial.iter().cloned());

    if let (Some(lhs_kind), Some(rhs_kind)) = (&lhs_fact.kind, &rhs_fact.kind) {
        if let Some(kind) = binary_result_kind(&op.node, lhs_kind, rhs_kind) {
            fact.kind = Some(kind);
            fact.partial.clear();
            return fact;
        }
    }

    fact.with_partial(PartialReason::UnsupportedSyntax("BinaryExpression".into()))
}

fn prefix_fact(
    op: &ast::Spanned<ast::PrefixOp>,
    operand: &ast::Spanned<ast::Expr>,
    span: SourceSpan,
    scope: &InferScope<'_>,
) -> ExpressionFact {
    let operand_fact = infer_expression_fact(operand, scope);
    let mut fact = ExpressionFact::new(span, ExpressionValueClass::Unknown);
    fact.dependencies = operand_fact.dependencies;
    fact.partial = operand_fact.partial;

    match (&op.node, &operand_fact.kind) {
        // `!` negates truthiness, so it is a bool for any operand.
        (ast::PrefixOp::Not, _) => {
            fact.kind = Some(Kind::Bool);
            fact.partial.clear();
            fact
        }
        (ast::PrefixOp::Neg | ast::PrefixOp::Pos, Some(kind)) if is_numeric(kind) => {
            fact.kind = Some(kind.clone());
            fact.partial.clear();
            fact
        }
        _ => fact.with_partial(PartialReason::UnsupportedSyntax("PrefixExpression".into())),
    }
}

fn object_fact(
    fields: &[(ast::Spanned<String>, ast::Spanned<ast::Expr>)],
    span: SourceSpan,
    scope: &InferScope<'_>,
) -> ExpressionFact {
    let mut kinds = std::collections::BTreeMap::new();
    let mut partial = Vec::new();

    for (key, value) in fields {
        let value_fact = infer_expression_fact(value, scope);
        partial.extend(value_fact.partial.iter().cloned());
        kinds.insert(key.node.clone(), value_fact.kind.unwrap_or(Kind::Any));
    }

    let mut fact = ExpressionFact::new(span, ExpressionValueClass::Object)
        .with_kind(Kind::Literal(KindLiteral::Object(kinds)));
    fact.partial = partial;
    let values: Option<std::collections::BTreeMap<String, surrealdb_types::Value>> = fields
        .iter()
        .map(|(key, value)| {
            infer_expression_fact(value, scope)
                .value
                .map(|value| (key.node.clone(), value))
        })
        .collect();
    if let Some(values) = values {
        fact.value = Some(surrealdb_types::Value::Object(values.into()));
    }
    fact
}

fn array_fact(
    elements: &[ast::Spanned<ast::Expr>],
    span: SourceSpan,
    scope: &InferScope<'_>,
) -> ExpressionFact {
    let facts: Vec<_> = elements
        .iter()
        .map(|element| infer_expression_fact(element, scope))
        .collect();
    let max_len = Some(facts.len() as u64);

    let element_kind = homogeneous(facts.iter().filter_map(|f| f.kind.clone()));

    let mut fact = ExpressionFact::new(span, ExpressionValueClass::Array).with_kind(Kind::Array(
        Box::new(element_kind.clone().unwrap_or(Kind::Any)),
        max_len,
    ));
    fact.partial = facts
        .iter()
        .flat_map(|f| f.partial.iter().cloned())
        .collect();
    if facts.len() > 1 && element_kind.is_none() {
        fact.partial
            .push(PartialReason::UnsupportedSyntax("mixed array".into()));
    }
    if let Some(values) = facts
        .iter()
        .map(|f| f.value.clone())
        .collect::<Option<Vec<_>>>()
    {
        fact.value = Some(surrealdb_types::Value::Array(values.into()));
    }
    fact
}

fn call_fact(call: &ast::Call, span: SourceSpan, scope: &InferScope<'_>) -> ExpressionFact {
    let mut fact = ExpressionFact::new(span, ExpressionValueClass::FunctionCall);
    fact.dependencies.function = Some(call.path.node.clone());

    let args: Vec<Kind> = call
        .args
        .iter()
        .map(|arg| infer_expression_fact(arg, scope).kind.unwrap_or(Kind::Any))
        .collect();
    let kind = crate::analyzer::function::builtin_return_kind_for_call(scope, call, &args);
    fact.with_kind(kind)
}

fn cast_fact(ty: &ast::Spanned<ast::TypeExpr>, span: SourceSpan) -> ExpressionFact {
    let fact = ExpressionFact::new(span, ExpressionValueClass::Unknown);
    match cast_kind(&ty.node) {
        Some(kind) => fact.with_kind(kind),
        None => fact.with_partial(PartialReason::UnsupportedSyntax("TypeCast".into())),
    }
}

fn cast_kind(ty: &ast::TypeExpr) -> Option<Kind> {
    let ast::TypeExpr::Name(name) = ty else {
        return None;
    };
    let kind = match name.node.to_ascii_lowercase().as_str() {
        "int" => Kind::Int,
        "float" => Kind::Float,
        "decimal" => Kind::Decimal,
        "number" => Kind::Number,
        "string" => Kind::String,
        "bool" => Kind::Bool,
        "duration" => Kind::Duration,
        "datetime" => Kind::Datetime,
        "uuid" => Kind::Uuid,
        "bytes" => Kind::Bytes,
        _ => return None,
    };
    Some(kind)
}

fn binary_result_kind(op: &ast::BinaryOp, lhs: &Kind, rhs: &Kind) -> Option<Kind> {
    use ast::BinaryOp as Op;
    match op {
        Op::Add if matches!(lhs, Kind::String) && matches!(rhs, Kind::String) => Some(Kind::String),
        Op::Add | Op::Sub | Op::Mul | Op::Div if is_numeric(lhs) && is_numeric(rhs) => {
            if matches!(lhs, Kind::Decimal) || matches!(rhs, Kind::Decimal) {
                Some(Kind::Decimal)
            } else if matches!(lhs, Kind::Float) || matches!(rhs, Kind::Float) {
                Some(Kind::Float)
            } else {
                Some(Kind::Int)
            }
        }
        // A comparison produces a bool no matter what it compares; mismatched
        // operands violate an invariant, not the result type.
        Op::Eq | Op::NotEq | Op::Lt | Op::LtEq | Op::Gt | Op::GtEq => Some(Kind::Bool),
        Op::And | Op::Or if matches!(lhs, Kind::Bool) && matches!(rhs, Kind::Bool) => {
            Some(Kind::Bool)
        }
        Op::NullCoalesce if matches!(lhs, Kind::None | Kind::Null) => Some(rhs.clone()),
        Op::NullCoalesce if matches!(rhs, Kind::None | Kind::Null) => Some(lhs.clone()),
        // Coalescing two known kinds yields one of them.
        Op::NullCoalesce => Some(Kind::either(vec![lhs.clone(), rhs.clone()])),
        _ => None,
    }
}

fn is_numeric(kind: &Kind) -> bool {
    matches!(kind, Kind::Int | Kind::Float | Kind::Decimal | Kind::Number)
}

fn homogeneous<T: PartialEq>(mut values: impl Iterator<Item = T>) -> Option<T> {
    let first = values.next()?;
    values.all(|value| value == first).then_some(first)
}

fn merge_dependencies(fact: &mut ExpressionFact, deps: crate::expression::ExpressionDependencies) {
    fact.dependencies.field_paths.extend(deps.field_paths);
    fact.dependencies.variables.extend(deps.variables);
    fact.dependencies.params.extend(deps.params);
}

fn scalar_fact(span: SourceSpan, class: ExpressionValueClass, kind: Kind) -> ExpressionFact {
    ExpressionFact::new(span, class).with_kind(kind)
}

fn partial_fact(span: SourceSpan, class: ExpressionValueClass, syntax: String) -> ExpressionFact {
    ExpressionFact::new(span, class).with_partial(PartialReason::UnsupportedSyntax(syntax))
}

fn source_span(source: &SourceId, range: surrealguard_syntax::span::ByteRange) -> SourceSpan {
    SourceSpan::new(source.clone(), range)
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealguard_syntax::lower::lower_expr;
    use surrealguard_syntax::parse::{parse_source, ParsedSource};

    use crate::schema::{extract_schema, SchemaIndex};

    fn parse(query: &str) -> ParsedSource {
        parse_source(SourceId::new("infer:test"), query).expect("test query parses")
    }

    fn infer_first(
        parsed: &ParsedSource,
        kind: &str,
        row_table: Option<&TableDef>,
        env: &StatementEnv,
    ) -> ExpressionFact {
        infer_first_with_schema(parsed, kind, &SchemaIndex::default(), row_table, env)
    }

    fn infer_first_with_schema(
        parsed: &ParsedSource,
        kind: &str,
        schema: &SchemaIndex,
        row_table: Option<&TableDef>,
        env: &StatementEnv,
    ) -> ExpressionFact {
        let node = find_first(parsed.tree().root_node(), kind)
            .unwrap_or_else(|| panic!("no {kind} in {:?}", parsed.text()));
        let expr = lower_expr(node, parsed.text());
        let scope = InferScope {
            source: parsed.source_id(),
            text: parsed.text(),
            schema,
            row_table,
            env,
        };
        infer_expression_fact(&expr, &scope)
    }

    fn find_first<'tree>(
        node: tree_sitter::Node<'tree>,
        kind: &str,
    ) -> Option<tree_sitter::Node<'tree>> {
        if node.kind() == kind {
            return Some(node);
        }
        let mut cursor = node.walk();
        let found = node
            .children(&mut cursor)
            .find_map(|child| find_first(child, kind));
        found
    }

    fn person_schema() -> SchemaIndex {
        let parsed = parse_source(
            SourceId::new("infer:schema"),
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD profile.email ON person TYPE string;",
        )
        .expect("schema parses");
        extract_schema(&[parsed]).schema
    }

    #[test]
    fn infers_literal_kinds_including_prefixed_strings() {
        let env = StatementEnv::default();
        let cases = [
            ("RETURN 42;", "Number", Kind::Int),
            ("RETURN 2.5;", "Number", Kind::Float),
            ("RETURN 'hi';", "String", Kind::String),
            ("RETURN d'2024-01-01T00:00:00Z';", "String", Kind::Datetime),
            ("RETURN u'0189-aa';", "String", Kind::Uuid),
            ("RETURN true;", "Bool", Kind::Bool),
            ("RETURN NONE;", "None", Kind::None),
            ("RETURN null;", "None", Kind::Null),
            ("RETURN 1h;", "Duration", Kind::Duration),
        ];

        for (query, node_kind, expected) in cases {
            let parsed = parse(query);
            let fact = infer_first(&parsed, node_kind, None, &env);
            assert_eq!(fact.kind, Some(expected), "query: {query}");
            assert!(fact.partial.is_empty(), "query: {query}");
        }
    }

    #[test]
    fn resolves_schema_backed_field_paths_including_nested() {
        let schema = person_schema();
        let table = schema.tables.get("person").unwrap();
        let env = StatementEnv::default();

        let parsed = parse("SELECT profile.email FROM person;");
        let fact = infer_first(&parsed, "Path", Some(table), &env);

        assert_eq!(fact.kind, Some(Kind::String));
        assert_eq!(fact.dependencies.field_paths, vec!["profile.email"]);
        assert!(fact.partial.is_empty());
    }

    #[test]
    fn unbound_params_are_recorded_dependencies_with_explicit_unresolved() {
        let env = StatementEnv::default();
        let parsed = parse("RETURN $name;");

        let fact = infer_first(&parsed, "VariableName", None, &env);

        assert_eq!(fact.kind, None);
        assert_eq!(fact.dependencies.params, vec!["name"]);
        assert_eq!(fact.partial, vec![PartialReason::Unresolved]);
    }

    #[test]
    fn let_bound_params_resolve_through_binary_expressions() {
        let mut env = StatementEnv::default();
        let parsed = parse("RETURN $age + 1;");
        env.define_let(
            "age".into(),
            ExpressionFact::new(
                SourceSpan::new(
                    parsed.source_id().clone(),
                    surrealguard_syntax::span::ByteRange::new(0, 1).unwrap(),
                ),
                ExpressionValueClass::Literal,
            )
            .with_kind(Kind::Int),
        );

        let fact = infer_first(&parsed, "BinaryExpression", None, &env);

        assert_eq!(fact.kind, Some(Kind::Int));
        assert!(fact.partial.is_empty());
    }

    #[test]
    fn comparison_boolean_and_coalesce_operators_infer_kinds() {
        let schema = person_schema();
        let table = schema.tables.get("person").unwrap();
        let env = StatementEnv::default();

        let ge = parse("SELECT age >= 18 AS adult FROM person;");
        let fact = infer_first(&ge, "BinaryExpression", Some(table), &env);
        assert_eq!(fact.kind, Some(Kind::Bool));

        let and = parse("RETURN true AND false;");
        let fact = infer_first(&and, "BinaryExpression", None, &env);
        assert_eq!(fact.kind, Some(Kind::Bool));

        let coalesce = parse("RETURN NONE ?? 'fallback';");
        let fact = infer_first(&coalesce, "BinaryExpression", None, &env);
        assert_eq!(fact.kind, Some(Kind::String));

        // Mismatched comparisons still produce a bool: the mismatch is an
        // invariant violation, not type ambiguity.
        let mismatched = parse("RETURN 'a' > 18;");
        let fact = infer_first(&mismatched, "BinaryExpression", None, &env);
        assert_eq!(fact.kind, Some(Kind::Bool));

        // Coalescing two known kinds yields one of them.
        let union = parse("RETURN 1 ?? 'fallback';");
        let fact = infer_first(&union, "BinaryExpression", None, &env);
        assert_eq!(fact.kind, Some(Kind::Either(vec![Kind::Int, Kind::String])));
    }

    #[test]
    fn prefix_not_and_negation_infer_kinds() {
        let env = StatementEnv::default();

        let not = parse("RETURN !true;");
        let fact = infer_first(&not, "PrefixExpression", None, &env);
        assert_eq!(fact.kind, Some(Kind::Bool));

        // `!` negates truthiness: bool for any operand kind.
        let truthy = parse("RETURN !'text';");
        let fact = infer_first(&truthy, "PrefixExpression", None, &env);
        assert_eq!(fact.kind, Some(Kind::Bool));
    }

    #[test]
    fn objects_and_arrays_infer_structured_kinds() {
        let schema = person_schema();
        let table = schema.tables.get("person").unwrap();
        let env = StatementEnv::default();

        let parsed = parse("SELECT { who: name, tags: ['a', 'b'], ok: true } AS data FROM person;");
        let fact = infer_first(&parsed, "Object", Some(table), &env);

        let Some(Kind::Literal(surrealdb_types::KindLiteral::Object(fields))) = fact.kind else {
            panic!("expected object literal kind, got {:?}", fact.kind);
        };
        assert_eq!(fields["who"], Kind::String);
        assert_eq!(fields["ok"], Kind::Bool);
        assert_eq!(fields["tags"], Kind::Array(Box::new(Kind::String), Some(2)));
    }

    #[test]
    fn known_function_calls_resolve_return_kinds_by_name() {
        let env = StatementEnv::default();
        let parsed = parse("RETURN string::len($name);");

        let fact = infer_first(&parsed, "FunctionCall", None, &env);

        assert_eq!(fact.kind, Some(Kind::Int));
        assert_eq!(fact.dependencies.function.as_deref(), Some("string::len"));
    }

    #[test]
    fn record_id_literals_infer_their_record_kind() {
        let env = StatementEnv::default();
        let parsed = parse("RETURN person:one;");

        let fact = infer_first(&parsed, "RecordId", None, &env);

        assert_eq!(fact.kind, Some(Kind::Record(vec!["person".into()])));
        assert!(fact.partial.is_empty());
    }

    #[test]
    fn closures_infer_function_kinds_from_declarations_and_bodies() {
        let env = StatementEnv::default();

        // Declared parameter type + inferred body.
        let parsed = parse("LET $f = |$x: int| $x + 1;");
        let fact = infer_first(&parsed, "Closure", None, &env);
        assert_eq!(
            fact.kind,
            Some(Kind::Function(
                vec![Kind::Int].into(),
                Some(Box::new(Kind::Int))
            ))
        );

        // Declared return type wins without body inference.
        let parsed = parse("LET $f = |$x| -> string { RETURN 'hi'; };");
        let fact = infer_first(&parsed, "Closure", None, &env);
        assert_eq!(
            fact.kind,
            Some(Kind::Function(
                vec![Kind::Any].into(),
                Some(Box::new(Kind::String))
            ))
        );

        // Block bodies infer through LET threading and RETURN.
        let parsed = parse("LET $f = |$x: int| { LET $y = $x * 2; RETURN $y; };");
        let fact = infer_first(&parsed, "Closure", None, &env);
        assert_eq!(
            fact.kind,
            Some(Kind::Function(
                vec![Kind::Int].into(),
                Some(Box::new(Kind::Int))
            ))
        );
    }

    #[test]
    fn casts_infer_the_target_kind() {
        let env = StatementEnv::default();
        let parsed = parse("RETURN <int> '42';");

        let fact = infer_first(&parsed, "TypeCast", None, &env);

        assert_eq!(fact.kind, Some(Kind::Int));
    }

    #[test]
    fn subqueries_infer_their_statement_response_kind() {
        let schema = person_schema();
        let env = StatementEnv::default();

        let sub = parse("RETURN (SELECT name FROM person);");
        let fact = infer_first_with_schema(&sub, "SubQuery", &schema, None, &env);

        assert_eq!(fact.value_class, ExpressionValueClass::Subquery);
        let Some(Kind::Array(element, _)) = fact.kind else {
            panic!("expected array kind, got {:?}", fact.kind);
        };
        let Kind::Literal(surrealdb_types::KindLiteral::Object(fields)) = *element else {
            panic!("expected object literal element");
        };
        assert_eq!(fields["name"], Kind::String);
    }

    #[test]
    fn blocks_stay_partial_in_pure_inference() {
        // Blocks thread an environment through their statements, which is
        // the dispatcher's job (`analyze_expr` covers them).
        let env = StatementEnv::default();
        let block = parse("RETURN { LET $x = 1; RETURN $x; };");
        let fact = infer_first(&block, "Block", None, &env);
        assert_eq!(fact.value_class, ExpressionValueClass::Block);
        assert!(!fact.partial.is_empty());
    }

    #[test]
    fn let_bound_objects_resolve_value_rooted_paths() {
        let mut env = StatementEnv::default();
        let mut fields = std::collections::BTreeMap::new();
        fields.insert("name".to_string(), Kind::String);
        env.define_let(
            "user".into(),
            ExpressionFact::new(
                SourceSpan::new(
                    SourceId::new("env"),
                    surrealguard_syntax::span::ByteRange::new(0, 1).unwrap(),
                ),
                ExpressionValueClass::Object,
            )
            .with_kind(Kind::Literal(surrealdb_types::KindLiteral::Object(fields))),
        );

        let parsed = parse("RETURN $user.name;");
        let fact = infer_first(&parsed, "Path", None, &env);

        assert_eq!(fact.kind, Some(Kind::String));
    }

    #[test]
    fn record_link_fields_step_through_the_schema() {
        let schema = schema_with_links();
        let table = schema.tables.get("person").unwrap();
        let env = StatementEnv::default();

        // best_friend is record<person>; stepping through it reaches the
        // target table's fields.
        let parsed = parse("SELECT best_friend.name FROM person;");
        let fact = infer_first_with_schema(&parsed, "Path", &schema, Some(table), &env);

        assert_eq!(fact.kind, Some(Kind::String));
    }

    #[test]
    fn method_calls_dispatch_by_receiver_kind() {
        let schema = person_schema();
        let table = schema.tables.get("person").unwrap();
        let env = StatementEnv::default();

        let parsed = parse("SELECT name.len() FROM person;");
        let fact = infer_first_with_schema(&parsed, "Path", &schema, Some(table), &env);

        assert_eq!(fact.kind, Some(Kind::Int));
    }

    fn schema_with_links() -> SchemaIndex {
        let parsed = parse_source(
            SourceId::new("infer:schema-links"),
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD best_friend ON person TYPE record<person>;",
        )
        .expect("schema parses");
        extract_schema(&[parsed]).schema
    }

    #[test]
    fn graph_idioms_are_explicit_partials_for_expression_inference() {
        let schema = person_schema();
        let table = schema.tables.get("person").unwrap();
        let env = StatementEnv::default();

        let parsed = parse("SELECT ->likes->post FROM person;");
        let fact = infer_first(&parsed, "Path", Some(table), &env);

        assert_eq!(fact.value_class, ExpressionValueClass::FieldPath);
        assert_eq!(fact.partial, vec![PartialReason::Unresolved]);
    }
}
