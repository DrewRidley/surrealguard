//! Thin expression utilities for resolving leaf expressions.
//!
//! These are small focused functions, NOT a god resolver.
//! Statement modules call these directly for their specific needs.

use tree_sitter::Node;
use crate::context::Context;
use crate::parser::node_text;
use crate::types::{Kind, Literal, is_assignable};

/// Resolve a literal value to its type.
pub fn resolve_literal(node: &Node, source: &str) -> Kind {
    match node.kind() {
        "string" | "prefixed_string" => Kind::String,
        "int" => Kind::Int,
        "float" => Kind::Float,
        "decimal" => Kind::Decimal,
        "number" => Kind::Number,
        "keyword_true" | "keyword_false" => Kind::Bool,
        "keyword_none" | "keyword_null" => Kind::Null,
        "datetime" => Kind::Datetime,
        "duration" | "duration_part" => Kind::Duration,
        "uuid" => Kind::Uuid,
        _ => Kind::Any,
    }
}

/// Check if a node is a literal.
pub fn is_literal(node: &Node) -> bool {
    matches!(
        node.kind(),
        "string" | "prefixed_string" | "int" | "float" | "decimal" | "number"
        | "keyword_true" | "keyword_false" | "keyword_none" | "keyword_null"
        | "datetime" | "duration" | "duration_part" | "uuid"
    )
}

/// Resolve a variable reference to its type from scope.
pub fn resolve_variable(node: &Node, source: &str, ctx: &Context) -> Kind {
    let name = node_text(node, source);
    let var_name = if name.starts_with('$') {
        name.to_string()
    } else {
        format!("${}", name)
    };
    ctx.scope.lookup(&var_name)
        .map(|b| b.typ.clone())
        .unwrap_or(Kind::Any)
}

/// Resolve the type of a simple expression (literal, variable, or identifier field).
///
/// This handles the leaf cases. For complex expressions (function calls,
/// binary ops, paths), the calling statement module handles them directly.
pub fn resolve_simple(node: &Node, source: &str, ctx: &Context, table: Option<&str>) -> Kind {
    match node.kind() {
        // Literals
        k if is_literal_kind(k) => resolve_literal(node, source),

        // Variables
        "variable_name" | "variable" | "parameter" => resolve_variable(node, source, ctx),

        // Identifier — look up as field on the table
        "identifier" => {
            let name = node_text(node, source);
            if let Some(tbl) = table {
                if let Some(field) = ctx.get_field(tbl, name) {
                    return field.typ.clone().unwrap_or(Kind::Any);
                }
            }
            Kind::Any
        }

        // Transparent wrappers — descend
        "value" | "base_value" | "expression" | "subquery_statement"
        | "primary_statement" | "inclusive_predicate" | "predicate" => {
            let mut cursor = node.walk();
            let children: Vec<_> = node.named_children(&mut cursor).collect();
            if let Some(child) = children.first() {
                resolve_simple(child, source, ctx, table)
            } else {
                Kind::Any
            }
        }

        _ => Kind::Any,
    }
}

fn is_literal_kind(kind: &str) -> bool {
    matches!(
        kind,
        "string" | "prefixed_string" | "int" | "float" | "decimal" | "number"
        | "keyword_true" | "keyword_false" | "keyword_none" | "keyword_null"
        | "datetime" | "duration" | "duration_part" | "uuid"
    )
}

/// Infer the result type of a binary operation.
pub fn binary_op_type(left: &Kind, op: &str, right: &Kind) -> Kind {
    match op {
        // Comparison operators → bool
        "=" | "!=" | "==" | ">" | "<" | ">=" | "<=" | "IS" | "IS NOT"
        | "CONTAINS" | "CONTAINSNOT" | "CONTAINSALL" | "CONTAINSANY"
        | "CONTAINSNONE" | "INSIDE" | "NOTINSIDE" | "ALLINSIDE"
        | "ANYINSIDE" | "NONEINSIDE" | "OUTSIDE" | "INTERSECTS"
        | "MATCHES" | "~" | "!~" | "*~" => Kind::Bool,

        // Logical operators → bool
        "AND" | "OR" | "&&" | "||" => Kind::Bool,

        // Arithmetic
        "+" => {
            if matches!(left, Kind::String) || matches!(right, Kind::String) {
                Kind::String // string concatenation
            } else {
                Kind::Number
            }
        }
        "-" | "*" | "/" | "%" | "**" => Kind::Number,

        // Null coalescing
        "??" => right.clone(),

        _ => Kind::Any,
    }
}
