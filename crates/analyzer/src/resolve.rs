/// Expression type resolution — the heart of the analyzer.
///
/// Given any CST expression/value node, determines its [`Type`].
/// Handles literals, variables, field paths, binary ops, function calls,
/// casts, subqueries, closures, graph paths, arrays, objects, and more.
use tree_sitter::Node;

use crate::context::Context;
use crate::diagnostic::{Code, Diagnostic};
use crate::functions;
use crate::parser::{child_by_kind, node_text};
use crate::scope::{Binding, BindingKind};
use crate::span::Span;
use crate::types::{Kind, KindExt, Literal, Table};

/// Resolve the type of an expression node.
pub fn resolve_expr(node: &Node, source: &str, ctx: &mut Context, table: Option<&str>) -> Kind {
    match node.kind() {
        "value" | "base_value" | "expression" | "inclusive_predicate" | "closure_body" => {
            // Wrapper nodes — resolve the inner child.
            // closure_body can be either a simple expression or a binary
            // expression (closure_body operator closure_body). In the binary
            // case the first two named children are the operands with an
            // operator in between — we delegate to resolve_binary_expr.
            let mut cursor = node.walk();
            let children: Vec<Node> = node.named_children(&mut cursor).collect();
            if node.kind() == "closure_body" && children.len() >= 3 {
                // Binary form: closure_body operator closure_body
                resolve_binary_expr(node, source, ctx, table)
            } else if let Some(child) = children.first() {
                resolve_expr(child, source, ctx, table)
            } else {
                Kind::Any
            }
        }

        // ── Literals ──────────────────────────────────────────
        "string" | "single_quoted_string" | "double_quoted_string" | "raw_string" => Kind::String,
        "integer" | "int" => Kind::Int,
        "float" => Kind::Float,
        "decimal" => Kind::Decimal,
        "number" => Kind::Number,
        "true" | "false" | "bool" | "keyword_true" | "keyword_false" => Kind::Bool,
        "none" | "null" | "keyword_none" | "keyword_null" => Kind::Null,
        "duration" => Kind::Duration,
        "datetime" => Kind::Datetime,
        "uuid" => Kind::Uuid,
        "bytes" => Kind::Bytes,
        "point" => Kind::Geometry(vec!["point".into()]),
        "geometry" => Kind::Geometry(vec![]),
        "range" | "range_expression" => Kind::Range,
        "regex" => crate::types::regex_kind(),

        // ── Variables ─────────────────────────────────────────
        "variable" | "parameter" | "variable_name" => {
            let name = node_text(node, source);
            let var_name = if name.starts_with('$') {
                name.to_string()
            } else {
                format!("${}", name)
            };

            if let Some(binding) = ctx.scope.lookup(&var_name) {
                binding.typ.clone()
            } else {
                let span = Span::from_node(node);
                ctx.emit(Diagnostic::error(
                    span,
                    Code::UndefinedVariable,
                    format!("undefined variable `{}`", var_name),
                ));
                Kind::Any
            }
        }

        // ── Identifiers (field access in table context) ──────
        "identifier" | "field" => {
            let field_name = node_text(node, source);
            // Check if it's a known field on the current table
            if let Some(tbl) = table {
                if let Some(field_def) = ctx.get_field(tbl, field_name) {
                    return field_def.typ.clone().unwrap_or(Kind::Any);
                }
                // In strict mode on schemafull tables, emit error for unknown fields
                if ctx.strict {
                    if let Some(table_def) = ctx.get_table(tbl) {
                        if table_def.schema_mode == crate::context::SchemaMode::Schemafull {
                            let span = Span::from_node(node);
                            ctx.emit(Diagnostic::error(
                                span,
                                Code::FieldNotFound,
                                format!(
                                    "field `{}` not defined on schemafull table `{}`",
                                    field_name, tbl
                                ),
                            ));
                        }
                    }
                }
            }
            Kind::Any
        }

        // ── Record ID ─────────────────────────────────────────
        "record_id" | "record_id_value" => {
            // Try to extract the table name from the record ID
            let mut cursor = node.walk();
            let children: Vec<Node> = node.children(&mut cursor).collect();
            if let Some(ident) = children.iter().find(|c| c.kind() == "identifier") {
                let table_name = node_text(ident, source);

                // In strict mode, verify the table exists
                if ctx.strict && !ctx.has_table(table_name) {
                    let span = Span::from_node(ident);
                    ctx.emit(Diagnostic::error(
                        span,
                        Code::TableNotFound,
                        format!("table `{}` not defined", table_name),
                    ));
                }

                Kind::Record(vec![Table::from(table_name.to_string())])
            } else {
                Kind::Record(vec![])
            }
        }

        // ── Array literal ─────────────────────────────────────
        "array" => {
            let mut cursor = node.walk();
            let element_types: Vec<Kind> = node
                .named_children(&mut cursor)
                .map(|child| resolve_expr(&child, source, ctx, table))
                .collect();

            if element_types.is_empty() {
                Kind::Array(Box::new(Kind::Any), None)
            } else {
                let unified = unify_types(&element_types);
                Kind::Array(Box::new(unified), None)
            }
        }

        // ── Object literal ────────────────────────────────────
        "object" => {
            let mut fields = std::collections::BTreeMap::new();
            let mut seen_keys: std::collections::HashMap<String, Span> = std::collections::HashMap::new();
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() == "object_entry" || child.kind() == "object_property" || child.kind() == "object_content" {
                    // object_content is an intermediate wrapper — iterate its children
                    if child.kind() == "object_content" {
                        let mut content_cursor = child.walk();
                        for prop in child.named_children(&mut content_cursor) {
                            if prop.kind() == "object_entry" || prop.kind() == "object_property" {
                                resolve_object_property(&prop, source, ctx, table, &mut fields, &mut seen_keys);
                            }
                        }
                    } else {
                        resolve_object_property(&child, source, ctx, table, &mut fields, &mut seen_keys);
                    }
                }
            }
            Kind::Literal(Literal::Object(fields))
        }

        // ── Binary expressions ────────────────────────────────
        "binary_expression" => resolve_binary_expr(node, source, ctx, table),

        // ── Unary expressions ─────────────────────────────────
        "negated_expression" => {
            // negated_expression has unnamed `!` child and named operand
            let mut cursor = node.walk();
            let children: Vec<Node> = node.named_children(&mut cursor).collect();
            #[cfg(test)]
            eprintln!("DEBUG negated_expression: children={:?}", children.iter().map(|c| (c.kind(), node_text(c, source))).collect::<Vec<_>>());
            if let Some(operand) = children.first() {
                let inner = resolve_expr(operand, source, ctx, table);
                #[cfg(test)]
                eprintln!("DEBUG negated inner type: {:?}", inner);
                if !inner.is_any() && inner != Kind::Bool {
                    let span = Span::from_node(operand);
                    let mut diag = Diagnostic::warning(
                        span,
                        Code::TypeMismatch,
                        format!("`NOT` expects `bool` operand, found `{}`", inner),
                    );
                    if let Some((field_span, field_msg)) = field_def_span(operand, source, ctx, table) {
                        diag = diag.with_related(field_span, field_msg);
                    }
                    ctx.emit(diag);
                }
                Kind::Bool
            } else {
                Kind::Any
            }
        }
        "unary_expression" => {
            let mut cursor = node.walk();
            let children: Vec<Node> = node.named_children(&mut cursor).collect();
            if let Some(operand) = children.last() {
                let inner = resolve_expr(operand, source, ctx, table);
                // NOT produces bool, negation preserves numeric type
                if let Some(op_node) = children.first() {
                    let op_text = node_text(op_node, source).to_uppercase();
                    if op_text == "NOT" || op_text == "!" {
                        if !inner.is_any() && inner != Kind::Bool {
                            let span = Span::from_node(operand);
                            let mut diag = Diagnostic::warning(
                                span,
                                Code::TypeMismatch,
                                format!("`NOT` expects `bool` operand, found `{}`", inner),
                            );
                            if let Some(operand_node) = children.last() {
                                if let Some((field_span, field_msg)) = field_def_span(operand_node, source, ctx, table) {
                                    diag = diag.with_related(field_span, field_msg);
                                }
                            }
                            ctx.emit(diag);
                        }
                        return Kind::Bool;
                    }
                }
                inner
            } else {
                Kind::Any
            }
        }

        // ── Cast expressions ──────────────────────────────────
        "cast_expression" | "casting" => {
            // <type>expr — the result is the cast target type
            // Also validate the cast is legal
            let (target_type, type_text_owned) = if let Some(type_node) = child_by_kind(node, "cast_type") {
                let type_text = node_text(&type_node, source)
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .trim();
                let tt = type_text.to_string();
                (type_name_to_type(type_text), tt)
            } else if let Some(type_name) = child_by_kind(node, "type_name") {
                let text = node_text(&type_name, source).to_string();
                (type_name_to_type(&text), text)
            } else {
                return Kind::Any;
            };

            let target_type = match target_type {
                Some(t) => t,
                None => {
                    // Span the cast type node, not the whole expression
                    let span = child_by_kind(node, "cast_type")
                        .or_else(|| child_by_kind(node, "type_name"))
                        .map(|n| Span::from_node(&n))
                        .unwrap_or_else(|| Span::from_node(node));
                    ctx.emit(Diagnostic::warning(
                        span,
                        Code::UnknownCastType,
                        format!("unknown cast target type `{}`", type_text_owned),
                    ));
                    Kind::Any
                }
            };

            // Resolve the source expression type
            let mut cursor = node.walk();
            let source_children: Vec<Node> = node
                .named_children(&mut cursor)
                .filter(|c| c.kind() != "cast_type" && c.kind() != "type_name")
                .collect();
            let source_type = source_children
                .iter()
                .map(|c| resolve_expr(c, source, ctx, table))
                .last()
                .unwrap_or(Kind::Any);

            // Validate cast is legal
            if !crate::types::is_valid_cast(&target_type, &source_type) {
                // Span the source expression being cast
                let span = source_children.last()
                    .map(|c| Span::from_node(c))
                    .unwrap_or_else(|| Span::from_node(node));
                ctx.emit(Diagnostic::warning(
                    span,
                    Code::InvalidCast,
                    format!(
                        "invalid cast: `{}` cannot be cast to `{}`",
                        source_type, target_type
                    ),
                ));
            }

            target_type
        }

        // ── Function calls ────────────────────────────────────
        "function_call" | "function" => resolve_function_call(node, source, ctx, table),

        // ── Path expressions (a.b.c) ─────────────────────────
        "path" | "path_expression" | "predicate" | "accessor" => {
            resolve_path_expr(node, source, ctx, table)
        }

        // ── Subquery ──────────────────────────────────────────
        "subquery" | "sub_query" => {
            let mut cursor = node.walk();
            let children: Vec<Node> = node.named_children(&mut cursor).collect();
            if let Some(inner) = children.first() {
                resolve_expr(inner, source, ctx, table)
            } else {
                Kind::Any
            }
        }

        // ── Block expression ──────────────────────────────────
        "block" | "block_expression" => {
            ctx.scope.push(crate::scope::ScopeKind::Block);

            // Use control flow analysis to collect all return paths.
            // We still need to walk for unreachable code detection.
            let mut last_type = Kind::Null;
            let mut seen_terminator = false;
            let mut return_types: Vec<Kind> = Vec::new();

            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() == "expressions" {
                    let mut inner_cursor = child.walk();
                    for expr in child.named_children(&mut inner_cursor) {
                        if expr.kind() == "semi_colon" {
                            continue;
                        }
                        if seen_terminator {
                            let span = Span::from_node(&expr);
                            ctx.emit(Diagnostic::warning(
                                span,
                                Code::UnreachableCode,
                                "unreachable code after RETURN, THROW, BREAK, or CONTINUE"
                                    .to_string(),
                            ));
                            resolve_expr(&expr, source, ctx, table);
                            continue;
                        }
                        last_type = resolve_expr(&expr, source, ctx, table);
                        // Collect explicit return types
                        if is_return_node(&expr) {
                            return_types.push(last_type.clone());
                        }
                        if is_terminator(&expr) {
                            seen_terminator = true;
                        }
                    }
                } else if child.kind() == "semi_colon" {
                    continue;
                } else {
                    if seen_terminator {
                        let span = Span::from_node(&child);
                        ctx.emit(Diagnostic::warning(
                            span,
                            Code::UnreachableCode,
                            "unreachable code after RETURN, THROW, BREAK, or CONTINUE"
                                .to_string(),
                        ));
                        resolve_expr(&child, source, ctx, table);
                        continue;
                    }
                    last_type = resolve_expr(&child, source, ctx, table);
                    if is_return_node(&child) {
                        return_types.push(last_type.clone());
                    }
                    if is_terminator(&child) {
                        seen_terminator = true;
                    }
                }
            }

            ctx.scope.pop();

            // If we have explicit RETURN types, unify them with the implicit last type
            if return_types.is_empty() {
                last_type
            } else {
                if !seen_terminator {
                    // Block doesn't always terminate — implicit last value is also possible
                    return_types.push(last_type);
                }
                unify_types(&return_types)
            }
        }

        // ── IF expression ─────────────────────────────────────
        "if_expression" | "if_statement" => {
            // Returns the union of then/else branch types
            let mut branch_types = Vec::new();
            let mut cursor = node.walk();
            let mut seen_condition = false;
            for child in node.named_children(&mut cursor) {
                match child.kind() {
                    // Branch results
                    "if_then_result" | "block" | "block_expression" => {
                        branch_types.push(resolve_expr(&child, source, ctx, table));
                    }
                    // Else clause (contains keyword_else + result)
                    "else_then_clause" | "else_if_clause" | "else_clause" => {
                        branch_types.push(resolve_expr(&child, source, ctx, table));
                    }
                    // Skip keywords
                    k if k.starts_with("keyword_") => {}
                    // Everything else before THEN is a condition
                    _ => {
                        let cond_type = resolve_expr(&child, source, ctx, table);
                        if !seen_condition && !cond_type.is_any() && cond_type != Kind::Bool {
                            let span = Span::from_node(&child);
                            ctx.emit(Diagnostic::warning(
                                span,
                                Code::TypeMismatch,
                                format!("condition should be bool, got `{}`", cond_type),
                            ));
                        }
                        seen_condition = true;
                    }
                }
            }
            if branch_types.is_empty() {
                Kind::Null
            } else {
                unify_types(&branch_types)
            }
        }

        // ── SELECT statement (as expression) ──────────────────
        "select_statement" => {
            // Delegate to SELECT analyzer which returns the result type
            crate::statements::select::analyze(node, source, ctx)
        }

        // ── CREATE/UPDATE/etc as expressions ──────────────────
        "create_statement" | "update_statement" | "delete_statement" | "insert_statement"
        | "upsert_statement" | "relate_statement" => {
            crate::statements::analyze_statement(node, source, ctx)
        }

        // Administrative / infrastructure statements — no type analysis needed
        "remove_statement" | "alter_statement" | "rebuild_index_statement"
        | "info_statement" | "use_statement" | "option_statement"
        | "kill_statement" | "show_statement"
        | "define_table_statement" | "define_field_statement" | "define_index_statement"
        | "define_event_statement" | "define_function_statement" | "define_param_statement"
        | "define_scope_statement" | "define_namespace_statement"
        | "define_database_statement" | "define_analyzer_statement"
        | "define_token_statement" | "define_user_statement"
        | "define_access_statement" | "define_api_statement"
        | "define_module_statement" | "define_bucket_statement"
        | "define_config_statement" | "sleep_statement"
        | "begin_statement" | "commit_statement" | "cancel_statement"
        | "throw_statement" => Kind::Null,

        // LIVE SELECT returns a UUID (the live query ID)
        "live_select_statement" | "live_select_diff_statement" => Kind::Uuid,

        // ── Closure ───────────────────────────────────────────
        "closure" => {
            ctx.scope.push(crate::scope::ScopeKind::Closure);
            // Parse closure params — each is a `closure_param` node containing a `variable_name`
            let mut param_cursor = node.walk();
            for child in node.named_children(&mut param_cursor) {
                if child.kind() == "closure_param" {
                    let mut inner_cursor = child.walk();
                    for param in child.named_children(&mut inner_cursor) {
                        if param.kind() == "variable_name" || param.kind() == "variable" || param.kind() == "parameter" {
                            let name = node_text(&param, source).to_string();
                            let var_name = if name.starts_with('$') {
                                name
                            } else {
                                format!("${}", name)
                            };
                            ctx.scope.bind(Binding {
                                name: var_name,
                                typ: Kind::Any,
                                span: Span::from_node(&param),
                                mutable: false,
                                kind: BindingKind::ClosureParam,
                            });
                        }
                    }
                }
            }
            // Resolve body
            let mut body_type = Kind::Any;
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() != "closure_param" {
                    body_type = resolve_expr(&child, source, ctx, table);
                }
            }
            ctx.scope.pop();
            Kind::Function(None, Some(Box::new(body_type)))
        }

        // ── Graph traversal ───────────────────────────────────
        "graph_path" | "graph_expression" => {
            resolve_graph_expr(node, source, ctx, table)
        }

        // ── Destructuring ─────────────────────────────────────
        "destructure" => {
            // { field1, field2 } on a path — produces object with those fields
            let mut fields = std::collections::BTreeMap::new();
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() == "identifier" {
                    let name = node_text(&child, source).to_string();
                    let typ = if let Some(tbl) = table {
                        ctx.get_field(tbl, &name)
                            .and_then(|f| f.typ.clone())
                            .unwrap_or(Kind::Any)
                    } else {
                        Kind::Any
                    };
                    fields.insert(name, typ);
                }
            }
            Kind::Literal(Literal::Object(fields))
        }

        // ── LET statement ─────────────────────────────────────
        "let_statement" => {
            resolve_let(node, source, ctx, table);
            Kind::Null
        }

        // ── RETURN statement ──────────────────────────────────
        "return_statement" | "return_clause" => {
            let mut cursor = node.walk();
            let children: Vec<Node> = node.named_children(&mut cursor).collect();
            if let Some(val) = children.iter().find(|c| c.kind() != "keyword_return") {
                resolve_expr(val, source, ctx, table)
            } else {
                Kind::Null
            }
        }

        // ── FOR loop ──────────────────────────────────────────
        "for_statement" => {
            resolve_for(node, source, ctx, table);
            Kind::Null
        }

        // ── BREAK / CONTINUE ─────────────────────────────────
        "break_statement" => {
            if !ctx.scope.is_in_loop() {
                let span = Span::from_node(node);
                ctx.emit(Diagnostic::error(
                    span,
                    Code::BreakOutsideLoop,
                    "BREAK used outside of a loop".to_string(),
                ));
            }
            Kind::Null
        }
        "continue_statement" => {
            if !ctx.scope.is_in_loop() {
                let span = Span::from_node(node);
                ctx.emit(Diagnostic::error(
                    span,
                    Code::ContinueOutsideLoop,
                    "CONTINUE used outside of a loop".to_string(),
                ));
            }
            Kind::Null
        }

        // ── Parenthesized expression ──────────────────────────
        "parenthesized_expression" | "paren_expression" => {
            let mut cursor = node.walk();
            let children: Vec<Node> = node.named_children(&mut cursor).collect();
            if let Some(inner) = children.first() {
                resolve_expr(inner, source, ctx, table)
            } else {
                Kind::Any
            }
        }

        // ── Ternary / inline IF ───────────────────────────────
        "ternary_expression" | "inline_if" => {
            let mut cursor = node.walk();
            let children: Vec<Node> = node.named_children(&mut cursor).collect();
            // condition, then, else
            if children.len() >= 3 {
                let cond_type = resolve_expr(&children[0], source, ctx, table);
                if !cond_type.is_any() && cond_type != Kind::Bool {
                    let span = Span::from_node(&children[0]);
                    ctx.emit(Diagnostic::warning(
                        span,
                        Code::TypeMismatch,
                        format!("condition should be bool, got `{}`", cond_type),
                    ));
                }
                let then_type = resolve_expr(&children[1], source, ctx, table);
                let else_type = resolve_expr(&children[2], source, ctx, table);
                unify_types(&[then_type, else_type])
            } else if children.len() >= 2 {
                let cond_type = resolve_expr(&children[0], source, ctx, table);
                if !cond_type.is_any() && cond_type != Kind::Bool {
                    let span = Span::from_node(&children[0]);
                    ctx.emit(Diagnostic::warning(
                        span,
                        Code::TypeMismatch,
                        format!("condition should be bool, got `{}`", cond_type),
                    ));
                }
                let then_type = resolve_expr(&children[1], source, ctx, table);
                then_type
            } else {
                Kind::Any
            }
        }

        // ── Method call (.method()) ───────────────────────────
        "method_call" => {
            let mut cursor = node.walk();
            let children: Vec<Node> = node.named_children(&mut cursor).collect();
            if children.len() >= 2 {
                let receiver_type = resolve_expr(&children[0], source, ctx, table);
                let method_name = node_text(&children[1], source);
                resolve_method_call(&receiver_type, method_name, node, source, ctx, table)
            } else {
                Kind::Any
            }
        }

        // Wrapper nodes — recurse into children
        "subquery_statement" | "primary_statement" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                let typ = resolve_expr(&child, source, ctx, table);
                if typ != Kind::Any {
                    return typ;
                }
            }
            Kind::Any
        }

        // ── IF/ELSE clause wrappers ──────────────────────────
        // These nodes contain keyword children plus a block or result.
        // Resolve the last named non-keyword child to get the branch type.
        "else_clause" | "else_if_clause" | "else_then_clause"
        | "else_if_then_clause" | "if_then_result" => {
            let mut cursor = node.walk();
            let mut last_type = Kind::Null;
            for child in node.named_children(&mut cursor) {
                if !child.kind().starts_with("keyword_") {
                    last_type = resolve_expr(&child, source, ctx, table);
                }
            }
            last_type
        }

        // Fallback — unknown node kinds
        _ => Kind::Any,
    }
}

// ── Terminator Detection ──────────────────────────────────────

/// Returns `true` if `node` is a control-flow terminator
/// (RETURN, THROW, BREAK, or CONTINUE) that makes subsequent
/// statements in the same block unreachable.
///
/// Tree-sitter may wrap the actual statement in wrapper nodes like
/// `expression`, `value`, or `base_value`, so we recurse through
/// those transparent wrappers.
fn is_terminator(node: &Node) -> bool {
    match node.kind() {
        "return_statement" | "return_clause" | "throw_statement" | "break_statement"
        | "continue_statement" => true,
        // Transparent wrapper nodes — check the inner child
        "value" | "base_value" | "expression" | "inclusive_predicate"
        | "subquery_statement" | "primary_statement" => {
            let mut cursor = node.walk();
            let result = node
                .named_children(&mut cursor)
                .any(|child| is_terminator(&child));
            result
        }
        _ => false,
    }
}

/// Check if a node is or contains an explicit RETURN statement.
fn is_return_node(node: &Node) -> bool {
    match node.kind() {
        "return_statement" | "return_clause" => true,
        "value" | "base_value" | "expression" | "subquery_statement" | "primary_statement"
        | "block" | "block_expression" | "expressions" => {
            let mut cursor = node.walk();
            let children: Vec<_> = node.named_children(&mut cursor).collect();
            children.iter().any(|child| is_return_node(child))
        }
        "if_expression" | "if_statement" => {
            let mut cursor = node.walk();
            let children: Vec<_> = node.named_children(&mut cursor).collect();
            children.iter().any(|child| {
                matches!(child.kind(), "if_then_result" | "block" | "block_expression"
                    | "else_then_clause" | "else_if_clause" | "else_clause")
                    && is_return_node(child)
            })
        }
        _ => false,
    }
}

// ── Control Flow Analysis ─────────────────────────────────────
//
// Collects all possible return types from all code paths through a
// block, function body, or expression. This enables:
// - Union types for blocks with multiple RETURN paths
// - "Not all paths return a value" warnings for functions
// - Accurate type inference for LET $x = { IF ... RETURN ... }

/// Result of control flow analysis on a block or expression.
#[derive(Debug, Clone)]
pub struct FlowResult {
    /// All possible return types from explicit RETURN statements.
    pub return_types: Vec<Kind>,
    /// The implicit return type (last expression, if no RETURN terminates first).
    pub implicit_type: Option<Kind>,
    /// Whether all code paths definitely return/terminate.
    pub always_returns: bool,
}

impl FlowResult {
    /// Compute the unified type across all possible code paths.
    pub fn unified_type(&self) -> Kind {
        let mut all_types = self.return_types.clone();
        if !self.always_returns {
            if let Some(ref implicit) = self.implicit_type {
                all_types.push(implicit.clone());
            } else {
                all_types.push(Kind::Null);
            }
        }
        if all_types.is_empty() {
            Kind::Null
        } else {
            unify_types(&all_types)
        }
    }
}

/// Analyze control flow through a node, collecting all possible return types.
///
/// This is the core of control flow analysis. It walks blocks, IF/ELSE branches,
/// and nested expressions to find every possible RETURN path.
pub fn analyze_flow(node: &Node, source: &str, ctx: &mut Context, table: Option<&str>) -> FlowResult {
    match node.kind() {
        "block" | "block_expression" => analyze_block_flow(node, source, ctx, table),
        "if_expression" | "if_statement" => analyze_if_flow(node, source, ctx, table),
        "return_statement" | "return_clause" => {
            let mut cursor = node.walk();
            let children: Vec<Node> = node.named_children(&mut cursor).collect();
            let ret_type = if let Some(val) = children.iter().find(|c| c.kind() != "keyword_return") {
                resolve_expr(val, source, ctx, table)
            } else {
                Kind::Null
            };
            FlowResult {
                return_types: vec![ret_type],
                implicit_type: None,
                always_returns: true,
            }
        }
        "throw_statement" => FlowResult {
            return_types: Vec::new(),
            implicit_type: None,
            always_returns: true, // THROW terminates but doesn't return a value
        },
        // Transparent wrappers
        "value" | "base_value" | "expression" | "subquery_statement"
        | "primary_statement" | "inclusive_predicate" | "expressions" => {
            let mut cursor = node.walk();
            let children: Vec<_> = node.named_children(&mut cursor).collect();
            if let Some(child) = children.iter().find(|c| c.kind() != "semi_colon") {
                analyze_flow(child, source, ctx, table)
            } else {
                FlowResult {
                    return_types: Vec::new(),
                    implicit_type: Some(Kind::Null),
                    always_returns: false,
                }
            }
        }
        // Any other expression — just resolve its type as implicit return
        _ => {
            let typ = resolve_expr(node, source, ctx, table);
            FlowResult {
                return_types: Vec::new(),
                implicit_type: Some(typ),
                always_returns: is_terminator(node),
            }
        }
    }
}

/// Analyze control flow through a block, collecting return types from all paths.
fn analyze_block_flow(node: &Node, source: &str, ctx: &mut Context, table: Option<&str>) -> FlowResult {
    let mut all_returns = Vec::new();
    let mut last_type = Kind::Null;
    let mut terminated = false;

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "semi_colon" {
            continue;
        }
        if terminated {
            // Still resolve for diagnostics but don't track
            resolve_expr(&child, source, ctx, table);
            continue;
        }

        if child.kind() == "expressions" {
            // Walk inner expressions
            let mut inner_cursor = child.walk();
            for expr in child.named_children(&mut inner_cursor) {
                if expr.kind() == "semi_colon" {
                    continue;
                }
                if terminated {
                    resolve_expr(&expr, source, ctx, table);
                    continue;
                }

                let flow = analyze_flow(&expr, source, ctx, table);
                all_returns.extend(flow.return_types);

                if flow.always_returns {
                    terminated = true;
                } else if let Some(t) = flow.implicit_type {
                    last_type = t;
                }
            }
        } else {
            let flow = analyze_flow(&child, source, ctx, table);
            all_returns.extend(flow.return_types);

            if flow.always_returns {
                terminated = true;
            } else if let Some(t) = flow.implicit_type {
                last_type = t;
            }
        }
    }

    FlowResult {
        return_types: all_returns,
        implicit_type: if terminated { None } else { Some(last_type) },
        always_returns: terminated,
    }
}

/// Analyze control flow through an IF/ELSE, merging branch return types.
fn analyze_if_flow(node: &Node, source: &str, ctx: &mut Context, table: Option<&str>) -> FlowResult {
    let mut all_returns = Vec::new();
    let mut branch_implicits = Vec::new();
    let mut has_else = false;
    let mut all_branches_return = true;

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "if_then_result" | "block" | "block_expression" => {
                let flow = analyze_flow(&child, source, ctx, table);
                all_returns.extend(flow.return_types);
                if !flow.always_returns {
                    all_branches_return = false;
                    if let Some(t) = flow.implicit_type {
                        branch_implicits.push(t);
                    }
                }
            }
            "else_then_clause" | "else_if_clause" | "else_clause" => {
                has_else = true;
                let flow = analyze_flow(&child, source, ctx, table);
                all_returns.extend(flow.return_types);
                if !flow.always_returns {
                    all_branches_return = false;
                    if let Some(t) = flow.implicit_type {
                        branch_implicits.push(t);
                    }
                }
            }
            k if k.starts_with("keyword_") => {}
            _ => {
                // Condition expression — just resolve for type checking
                resolve_expr(&child, source, ctx, table);
            }
        }
    }

    // If there's no else branch, the IF might not execute at all
    if !has_else {
        all_branches_return = false;
        branch_implicits.push(Kind::Null);
    }

    let implicit = if branch_implicits.is_empty() {
        None
    } else {
        Some(unify_types(&branch_implicits))
    };

    FlowResult {
        return_types: all_returns,
        implicit_type: implicit,
        always_returns: has_else && all_branches_return,
    }
}

// ── Object Property Resolution (with duplicate key detection) ─

fn resolve_object_property(
    prop: &Node,
    source: &str,
    ctx: &mut Context,
    table: Option<&str>,
    fields: &mut std::collections::BTreeMap<String, Kind>,
    seen_keys: &mut std::collections::HashMap<String, Span>,
) {
    let mut inner_cursor = prop.walk();
    let mut children: Vec<Node> = prop.named_children(&mut inner_cursor).collect();
    let (key, key_span, val_type) = if children.len() >= 2 {
        let key = node_text(&children[0], source).to_string();
        let key_span = Span::from_node(&children[0]);
        let val_type = resolve_expr(&children[1], source, ctx, table);
        (key, key_span, val_type)
    } else if children.len() == 1 {
        let key = node_text(&children[0], source).to_string();
        let key_span = Span::from_node(&children[0]);
        let val_type = resolve_expr(&children.remove(0), source, ctx, table);
        (key, key_span, val_type)
    } else {
        return;
    };

    if let Some(prev_span) = seen_keys.get(&key) {
        ctx.emit(
            Diagnostic::warning(
                key_span,
                Code::DuplicateObjectKey,
                format!("duplicate key `{}` in object literal", key),
            )
            .with_related(*prev_span, format!("key `{}` first defined here", key)),
        );
    } else {
        seen_keys.insert(key.clone(), key_span);
    }
    fields.insert(key, val_type);
}

// ── Binary Expression Resolution ──────────────────────────────

/// Try to extract the field name from an expression node (identifier or field access).
/// Returns the field name text if the node is a simple identifier or field reference.
fn extract_field_name<'a>(node: &Node, source: &'a str) -> Option<&'a str> {
    match node.kind() {
        "identifier" | "field" => Some(node_text(node, source)),
        "value" | "base_value" | "expression" => {
            let mut cursor = node.walk();
            let children: Vec<Node> = node.named_children(&mut cursor).collect();
            children.iter().find_map(|child| extract_field_name(child, source))
        }
        _ => None,
    }
}

/// Look up the field definition span for an operand node, if it references a known field.
fn field_def_span(node: &Node, source: &str, ctx: &Context, table: Option<&str>) -> Option<(Span, String)> {
    let field_name = extract_field_name(node, source)?;
    let tbl = table?;
    let field_def = ctx.get_field(tbl, field_name)?;
    Some((field_def.span, format!("`{}`.`{}` defined here", tbl, field_name)))
}

fn resolve_binary_expr(node: &Node, source: &str, ctx: &mut Context, table: Option<&str>) -> Kind {
    let mut cursor = node.walk();
    let children: Vec<Node> = node.named_children(&mut cursor).collect();

    if children.len() < 3 {
        // Might be structured differently, try with all children
        let mut cursor2 = node.walk();
        let all: Vec<Node> = node.children(&mut cursor2).collect();
        if all.len() >= 3 {
            let left_type = resolve_expr(&all[0], source, ctx, table);
            let op = node_text(&all[1], source).to_uppercase();
            let right_type = resolve_expr(&all[2], source, ctx, table);
            let left_field = field_def_span(&all[0], source, ctx, table);
            let right_field = field_def_span(&all[2], source, ctx, table);
            return resolve_binary_op(&left_type, &op, &right_type, node, ctx, left_field.as_ref(), right_field.as_ref());
        }
        return Kind::Any;
    }

    let left_type = resolve_expr(&children[0], source, ctx, table);
    let op_text = node_text(&children[1], source).to_uppercase();
    let right_type = resolve_expr(&children[2], source, ctx, table);
    let left_field = field_def_span(&children[0], source, ctx, table);
    let right_field = field_def_span(&children[2], source, ctx, table);

    resolve_binary_op(&left_type, &op_text, &right_type, node, ctx, left_field.as_ref(), right_field.as_ref())
}

fn resolve_binary_op(
    left: &Kind,
    op: &str,
    right: &Kind,
    node: &Node,
    ctx: &mut Context,
    left_field: Option<&(Span, String)>,
    right_field: Option<&(Span, String)>,
) -> Kind {
    match op {
        // Comparison operators -> bool
        "==" | "!=" | "<" | ">" | "<=" | ">=" | "IS" | "IS NOT" | "?" | "??" => {
            // Warn on clearly incompatible comparison types
            if !left.is_any() && !right.is_any() && are_clearly_incompatible(left, right) {
                let span = Span::from_node(node);
                let mut diag = Diagnostic::warning(
                    span,
                    Code::TypeMismatch,
                    format!(
                        "comparing incompatible types: expected compatible types, found `{}` and `{}`",
                        left, right
                    ),
                );
                if let Some((field_span, field_msg)) = left_field {
                    diag = diag.with_related(*field_span, field_msg.clone());
                }
                if let Some((field_span, field_msg)) = right_field {
                    diag = diag.with_related(*field_span, field_msg.clone());
                }
                diag = diag.with_suggestion(format!(
                    "if intentional, cast explicitly: `<{}>`",
                    left
                ));
                ctx.emit(diag);
            }
            Kind::Bool
        }

        // CONTAINS family -- left should be array, set, or string
        "CONTAINS" | "CONTAINSNOT" | "CONTAINSALL" | "CONTAINSANY" | "CONTAINSNONE" => {
            if !left.is_any() && !right.is_any() {
                let left_is_container = matches!(
                    left,
                    Kind::Array(_, _) | Kind::Set(_, _) | Kind::String
                );
                if !left_is_container {
                    let span = Span::from_node(node);
                    let mut diag = Diagnostic::warning(
                        span,
                        Code::TypeMismatch,
                        format!(
                            "`{}` expects left operand to be `array`, `set`, or `string`, found `{}`",
                            op, left
                        ),
                    );
                    if let Some((field_span, field_msg)) = left_field {
                        diag = diag.with_related(*field_span, field_msg.clone());
                    }
                    diag = diag.with_suggestion(format!(
                        "consider wrapping the value in an array: `[{}]`",
                        left
                    ));
                    ctx.emit(diag);
                }
            }
            Kind::Bool
        }

        // INSIDE family -- right should be array, set, or string
        "INSIDE" | "NOTINSIDE" | "ALLINSIDE" | "ANYINSIDE" | "NONEINSIDE" => {
            if !left.is_any() && !right.is_any() {
                let right_is_container = matches!(
                    right,
                    Kind::Array(_, _) | Kind::Set(_, _) | Kind::String
                );
                if !right_is_container {
                    let span = Span::from_node(node);
                    let mut diag = Diagnostic::warning(
                        span,
                        Code::TypeMismatch,
                        format!(
                            "`{}` expects right operand to be `array`, `set`, or `string`, found `{}`",
                            op, right
                        ),
                    );
                    if let Some((field_span, field_msg)) = right_field {
                        diag = diag.with_related(*field_span, field_msg.clone());
                    }
                    diag = diag.with_suggestion(format!(
                        "consider wrapping the value in an array: `[{}]`",
                        right
                    ));
                    ctx.emit(diag);
                }
            }
            Kind::Bool
        }

        // MATCHES -- regex match, both operands should be strings
        "MATCHES" => {
            if !left.is_any() && *left != Kind::String {
                let span = Span::from_node(node);
                let mut diag = Diagnostic::warning(
                    span,
                    Code::TypeMismatch,
                    format!(
                        "`MATCHES` expects `string` operands, found `{}` on the left",
                        left
                    ),
                );
                if let Some((field_span, field_msg)) = left_field {
                    diag = diag.with_related(*field_span, field_msg.clone());
                }
                diag = diag.with_suggestion(format!(
                    "cast to string first: `<string>{}`",
                    left
                ));
                ctx.emit(diag);
            }
            if !right.is_any() && *right != Kind::String {
                let span = Span::from_node(node);
                let mut diag = Diagnostic::warning(
                    span,
                    Code::TypeMismatch,
                    format!(
                        "`MATCHES` expects `string` operands, found `{}` on the right",
                        right
                    ),
                );
                if let Some((field_span, field_msg)) = right_field {
                    diag = diag.with_related(*field_span, field_msg.clone());
                }
                diag = diag.with_suggestion("the right operand of `MATCHES` should be a regex pattern string".to_string());
                ctx.emit(diag);
            }
            Kind::Bool
        }

        // Logical operators -> bool
        "AND" | "OR" | "&&" | "||" => {
            let canonical_op = match op {
                "&&" => "AND",
                "||" => "OR",
                other => other,
            };
            let left_bad = !left.is_any() && *left != Kind::Bool;
            let right_bad = !right.is_any() && *right != Kind::Bool;
            if left_bad || right_bad {
                let span = Span::from_node(node);
                let side_detail = if left_bad && right_bad {
                    format!("found `{}` on the left and `{}` on the right", left, right)
                } else if left_bad {
                    format!("found `{}` on the left", left)
                } else {
                    format!("found `{}` on the right", right)
                };
                let mut diag = Diagnostic::warning(
                    span,
                    Code::TypeMismatch,
                    format!(
                        "`{}` expects `bool` operands, {}",
                        canonical_op, side_detail
                    ),
                );
                if left_bad {
                    if let Some((field_span, field_msg)) = left_field {
                        diag = diag.with_related(*field_span, field_msg.clone());
                    }
                }
                if right_bad {
                    if let Some((field_span, field_msg)) = right_field {
                        diag = diag.with_related(*field_span, field_msg.clone());
                    }
                }
                ctx.emit(diag);
            }
            Kind::Bool
        }

        // Arithmetic operators → numeric type
        "+" | "-" | "*" | "/" | "%" | "**" => {
            // String concatenation
            if op == "+" && (*left == Kind::String || *right == Kind::String) {
                return Kind::String;
            }
            // Duration arithmetic
            if *left == Kind::Duration || *right == Kind::Duration {
                return Kind::Duration;
            }
            // Datetime + duration
            if *left == Kind::Datetime && *right == Kind::Duration {
                return Kind::Datetime;
            }
            // Numeric coercion
            if left.is_numeric() && right.is_numeric() {
                return coerce_numeric(left, right);
            }
            // If either side is Any, result is Any
            if left.is_any() || right.is_any() {
                return Kind::Any;
            }
            // Type mismatch
            if ctx.strict {
                let span = Span::from_node(node);
                ctx.emit(Diagnostic::warning(
                    span,
                    Code::InvalidOperator,
                    format!(
                        "operator `{}` may not be valid for types `{}` and `{}`",
                        op, left, right
                    ),
                ));
            }
            Kind::Any
        }

        // Null coalescing: `a ?? b` returns `a` if non-null, else `b`
        // Result type is the non-optional inner type of left, unified with right
        "?:" => {
            let unwrapped_left = match left {
                Kind::Option(inner) => inner.as_ref().clone(),
                Kind::Null => right.clone(),
                other => other.clone(),
            };
            crate::resolve::unify_types(&[unwrapped_left, right.clone()])
        }

        _ => Kind::Any,
    }
}

/// Returns a type category for incompatibility checks.
/// Types in the same category are considered potentially compatible.
/// Returns None for types that are compatible with most things (Any, Null, Option).
fn type_category(kind: &Kind) -> Option<u8> {
    match kind {
        // Null/Option are compatible with anything (null checks)
        Kind::Null | Kind::Option(_) => None,
        // Any is unknown
        Kind::Any => None,
        // Either/Literal are too complex to categorize simply
        Kind::Either(_) | Kind::Literal(_) => None,
        // Numerics are all compatible with each other
        Kind::Int | Kind::Float | Kind::Decimal | Kind::Number => Some(0),
        Kind::Bool => Some(1),
        Kind::Duration => Some(2),
        Kind::Datetime => Some(3),
        Kind::Uuid => Some(4),
        Kind::Record(_) => Some(5),
        Kind::Geometry(_) => Some(6),
        Kind::Bytes => Some(7),
        Kind::Array(_, _) => Some(8),
        Kind::Set(_, _) => Some(9),
        Kind::Object => Some(10),
        Kind::Range => Some(11),
        Kind::Function(_, _) => Some(12),
        // String has its own category — comparing string to int/bool/etc is suspicious
        Kind::String => Some(13),
        // Regex (Kind::Point sentinel) gets its own category
        Kind::Point => Some(14),
        _ => None,
    }
}

/// Check if two types are clearly incompatible for comparison.
/// Only returns true for truly nonsensical comparisons where no coercion exists.
fn are_clearly_incompatible(left: &Kind, right: &Kind) -> bool {
    match (type_category(left), type_category(right)) {
        (Some(a), Some(b)) => a != b,
        // If either type has no category, they might be compatible
        _ => false,
    }
}

/// Coerce two numeric types to a common numeric type.
fn coerce_numeric(a: &Kind, b: &Kind) -> Kind {
    match (a, b) {
        (Kind::Decimal, _) | (_, Kind::Decimal) => Kind::Decimal,
        (Kind::Float, _) | (_, Kind::Float) => Kind::Float,
        (Kind::Number, _) | (_, Kind::Number) => Kind::Number,
        (Kind::Int, Kind::Int) => Kind::Int,
        _ => Kind::Number,
    }
}

// ── Function Call Resolution ──────────────────────────────────

fn resolve_function_call(
    node: &Node,
    source: &str,
    ctx: &mut Context,
    table: Option<&str>,
) -> Kind {
    let full_text = node_text(node, source);

    // Extract function name
    let fn_name = if let Some(name_node) = child_by_kind(node, "function_name") {
        node_text(&name_node, source).to_string()
    } else if let Some(name_node) = child_by_kind(node, "builtin_function_name") {
        node_text(&name_node, source).to_string()
    } else if let Some(name_node) = child_by_kind(node, "identifier") {
        node_text(&name_node, source).to_string()
    } else {
        // Try to extract from the full text (before the '(')
        full_text
            .split('(')
            .next()
            .unwrap_or("")
            .trim()
            .to_string()
    };

    // For array functions that accept closures, use special resolution
    // that infers closure parameter types from the array element type.
    if fn_name.starts_with("array::") {
        let func = &fn_name[7..]; // strip "array::"
        match func {
            "map" | "filter" | "find" | "find_index" | "any" | "all" | "every" | "some"
            | "includes" | "filter_index" | "fold" | "reduce" => {
                let arg_nodes = collect_arg_nodes(node, source);
                if arg_nodes.len() >= 2 {
                    return resolve_array_closure_call(
                        func, &arg_nodes, node, source, ctx, table,
                    );
                }
                // Fall through to normal resolution if arg count is wrong
                // (will be caught by validation in array::resolve)
            }
            _ => {}
        }
    }

    // Collect argument types
    let arg_types = resolve_function_args(node, source, ctx, table);

    // Check custom functions first
    if fn_name.starts_with("fn::") {
        // Try both with and without fn:: prefix since storage varies
        let func_info = ctx
            .get_function(&fn_name)
            .or_else(|| ctx.get_function(&fn_name[4..]))
            .map(|f| (f.params.len(), f.return_type.clone(), f.params.clone()));

        if let Some((param_count, return_type, params)) = func_info {
            if param_count != arg_types.len() {
                let span = Span::from_node(node);
                ctx.emit(Diagnostic::error(
                    span,
                    Code::WrongArgCount,
                    format!(
                        "`{}` takes {} argument{} but {} {} supplied",
                        fn_name,
                        param_count,
                        if param_count == 1 { "" } else { "s" },
                        arg_types.len(),
                        if arg_types.len() == 1 { "was" } else { "were" }
                    ),
                ));
            }
            // Check argument types against declared parameter types
            for (i, ((param_name, param_type), arg_type)) in params.iter().zip(arg_types.iter()).enumerate() {
                if !param_type.is_any() && !arg_type.is_any()
                    && !crate::types::is_assignable(param_type, arg_type)
                {
                    let span = crate::functions::find_nth_arg_node(node, i)
                        .map(|n| Span::from_node(&n))
                        .unwrap_or_else(|| Span::from_node(node));
                    ctx.emit(Diagnostic::warning(
                        span,
                        Code::WrongArgType,
                        format!(
                            "`{}` expects `{}` for parameter `{}`, found `{}`",
                            fn_name, param_type, param_name, arg_type
                        ),
                    ));
                }
            }
            return return_type.unwrap_or(Kind::Any);
        } else {
            let span = Span::from_node(node);
            ctx.emit(Diagnostic::error(
                span,
                Code::FunctionNotFound,
                format!("custom function `{}` not defined", fn_name),
            ));
            return Kind::Any;
        }
    }

    // Built-in function dispatch
    functions::resolve_builtin(&fn_name, &arg_types, node, ctx)
}

fn resolve_function_args(
    node: &Node,
    source: &str,
    ctx: &mut Context,
    table: Option<&str>,
) -> Vec<Kind> {
    let mut args = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "function_name" | "builtin_function_name" | "custom_function_name" | "identifier"
                if child.start_byte() == node.start_byte() =>
            {
                // Skip the function name
                continue;
            }
            "arguments" | "argument_list" => {
                let mut arg_cursor = child.walk();
                for arg in child.named_children(&mut arg_cursor) {
                    args.push(resolve_expr(&arg, source, ctx, table));
                }
            }
            _ => {
                // Direct arguments (not in an argument_list wrapper)
                if child.kind() != "(" && child.kind() != ")" && child.kind() != "," {
                    args.push(resolve_expr(&child, source, ctx, table));
                }
            }
        }
    }
    args
}

/// Collect raw argument nodes from a function call without resolving them.
fn collect_arg_nodes<'a>(node: &Node<'a>, source: &str) -> Vec<Node<'a>> {
    let mut args = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "function_name" | "builtin_function_name" | "custom_function_name" | "identifier"
                if child.start_byte() == node.start_byte() =>
            {
                continue;
            }
            "arguments" | "argument_list" => {
                let mut arg_cursor = child.walk();
                for arg in child.named_children(&mut arg_cursor) {
                    args.push(arg);
                }
            }
            _ => {
                if child.kind() != "(" && child.kind() != ")" && child.kind() != "," {
                    args.push(child);
                }
            }
        }
    }
    let _ = source; // used only for symmetry with resolve_function_args
    args
}

/// Resolve a closure node with a known parameter type instead of Kind::Any.
fn resolve_closure_with_param_type(
    closure_node: &Node,
    param_type: &Kind,
    source: &str,
    ctx: &mut Context,
    table: Option<&str>,
) -> Kind {
    ctx.scope.push(crate::scope::ScopeKind::Closure);
    // Parse closure params — each is a `closure_param` node containing a `variable_name`
    let mut cursor = closure_node.walk();
    for child in closure_node.named_children(&mut cursor) {
        if child.kind() == "closure_param" {
            // closure_param contains a variable_name child
            let mut param_cursor = child.walk();
            for param_child in child.named_children(&mut param_cursor) {
                if param_child.kind() == "variable_name"
                    || param_child.kind() == "variable"
                    || param_child.kind() == "parameter"
                {
                    let name = node_text(&param_child, source).to_string();
                    let var_name = if name.starts_with('$') {
                        name
                    } else {
                        format!("${}", name)
                    };
                    ctx.scope.bind(crate::scope::Binding {
                        name: var_name,
                        typ: param_type.clone(),
                        span: Span::from_node(&param_child),
                        mutable: false,
                        kind: crate::scope::BindingKind::ClosureParam,
                    });
                }
            }
        }
    }
    // Resolve body
    let mut body_type = Kind::Any;
    let mut cursor2 = closure_node.walk();
    for child in closure_node.named_children(&mut cursor2) {
        if child.kind() != "closure_param" {
            body_type = resolve_expr(&child, source, ctx, table);
        }
    }
    ctx.scope.pop();
    body_type
}

/// Resolve array functions that accept closures (map, filter, find, etc.)
/// with proper closure parameter type inference.
fn resolve_array_closure_call(
    func: &str,
    arg_nodes: &[Node],
    call_node: &Node,
    source: &str,
    ctx: &mut Context,
    table: Option<&str>,
) -> Kind {
    use crate::functions::{expect_arg_type, expect_args, is_array};

    // Resolve the first argument (the array) to get its type
    let array_type = resolve_expr(&arg_nodes[0], source, ctx, table);

    // Extract the element type from the array
    let element_type = match &array_type {
        Kind::Array(inner, _) => inner.as_ref().clone(),
        Kind::Set(inner, _) => inner.as_ref().clone(),
        _ => Kind::Any,
    };

    // Check if the second argument is a closure
    let closure_node = if arg_nodes.len() >= 2 {
        let second = &arg_nodes[1];
        if second.kind() == "closure" {
            Some(second)
        } else {
            None
        }
    } else {
        None
    };

    // For fold/reduce, handle the 3-arg form: array, initial, closure
    let (closure_return_type, all_arg_types) = match func {
        "fold" | "reduce" => {
            let mut arg_types = vec![array_type.clone()];
            if arg_nodes.len() >= 2 {
                arg_types.push(resolve_expr(&arg_nodes[1], source, ctx, table));
            }
            let closure_ret = if arg_nodes.len() >= 3 && arg_nodes[2].kind() == "closure" {
                let ret = resolve_closure_with_param_type(
                    &arg_nodes[2],
                    &element_type,
                    source,
                    ctx,
                    table,
                );
                arg_types.push(Kind::Function(None, Some(Box::new(ret.clone()))));
                ret
            } else if arg_nodes.len() >= 3 {
                arg_types.push(resolve_expr(&arg_nodes[2], source, ctx, table));
                Kind::Any
            } else {
                Kind::Any
            };
            (closure_ret, arg_types)
        }
        _ => {
            let mut arg_types = vec![array_type.clone()];
            let closure_ret = if let Some(closure) = closure_node {
                let ret = resolve_closure_with_param_type(
                    closure,
                    &element_type,
                    source,
                    ctx,
                    table,
                );
                arg_types.push(Kind::Function(None, Some(Box::new(ret.clone()))));
                ret
            } else if arg_nodes.len() >= 2 {
                // Not a closure — resolve normally
                arg_types.push(resolve_expr(&arg_nodes[1], source, ctx, table));
                Kind::Any
            } else {
                Kind::Any
            };
            // Resolve any remaining args
            for arg in arg_nodes.iter().skip(2) {
                arg_types.push(resolve_expr(arg, source, ctx, table));
            }
            (closure_ret, arg_types)
        }
    };

    // Validate arg count and array type
    let full_name = format!("array::{}", func);

    match func {
        "map" => {
            expect_args(&full_name, &all_arg_types, 2, call_node, ctx);
            expect_arg_type(&full_name, &all_arg_types, 0, "array", is_array, call_node, ctx);
            if closure_node.is_some() {
                Kind::Array(Box::new(closure_return_type), None)
            } else {
                Kind::Array(Box::new(Kind::Any), None)
            }
        }
        "filter" => {
            expect_args(&full_name, &all_arg_types, 2, call_node, ctx);
            expect_arg_type(&full_name, &all_arg_types, 0, "array", is_array, call_node, ctx);
            // Warn if closure doesn't return bool
            if closure_node.is_some()
                && !closure_return_type.is_any()
                && !matches!(closure_return_type, Kind::Bool)
            {
                let span = closure_node
                    .map(|n| Span::from_node(&n))
                    .unwrap_or_else(|| Span::from_node(call_node));
                ctx.emit(Diagnostic::warning(
                    span,
                    Code::WrongArgType,
                    format!(
                        "`array::filter` closure should return `bool`, found `{}`",
                        closure_return_type
                    ),
                ));
            }
            Kind::Array(Box::new(element_type), None)
        }
        "filter_index" => {
            expect_args(&full_name, &all_arg_types, 2, call_node, ctx);
            expect_arg_type(&full_name, &all_arg_types, 0, "array", is_array, call_node, ctx);
            Kind::Array(Box::new(Kind::Int), None)
        }
        "find" => {
            expect_args(&full_name, &all_arg_types, 2, call_node, ctx);
            expect_arg_type(&full_name, &all_arg_types, 0, "array", is_array, call_node, ctx);
            element_type.optional()
        }
        "find_index" | "index_of" => {
            expect_args(&full_name, &all_arg_types, 2, call_node, ctx);
            expect_arg_type(&full_name, &all_arg_types, 0, "array", is_array, call_node, ctx);
            Kind::Int.optional()
        }
        "any" | "some" | "includes" | "all" | "every" => {
            expect_args(&full_name, &all_arg_types, 1.max(all_arg_types.len()), call_node, ctx);
            expect_arg_type(&full_name, &all_arg_types, 0, "array", is_array, call_node, ctx);
            Kind::Bool
        }
        "fold" | "reduce" => {
            let expected = 3;
            if all_arg_types.len() != expected {
                let span = crate::functions::find_arg_list_span(call_node)
                    .unwrap_or_else(|| Span::from_node(call_node));
                ctx.emit(Diagnostic::error(
                    span,
                    Code::WrongArgCount,
                    format!(
                        "`{}` takes {} arguments but {} {} supplied",
                        full_name,
                        expected,
                        all_arg_types.len(),
                        if all_arg_types.len() == 1 { "was" } else { "were" }
                    ),
                ));
            }
            expect_arg_type(&full_name, &all_arg_types, 0, "array", is_array, call_node, ctx);
            Kind::Any
        }
        _ => {
            // Shouldn't reach here, but fall back to generic resolution
            crate::functions::resolve_builtin(
                &format!("array::{}", func),
                &all_arg_types,
                call_node,
                ctx,
            )
        }
    }
}

// ── Path Expression Resolution ────────────────────────────────

fn resolve_path_expr(
    node: &Node,
    source: &str,
    ctx: &mut Context,
    table: Option<&str>,
) -> Kind {
    let mut cursor = node.walk();
    let children: Vec<Node> = node.named_children(&mut cursor).collect();

    if children.is_empty() {
        return Kind::Any;
    }

    // Detect multi-hop graph paths: path { graph_path, path_element { graph_path }, ... }
    // These need to be collected and validated together
    let graph_segments = collect_graph_segments(node, source);
    if graph_segments.len() >= 2 {
        return resolve_multi_hop_graph(node, source, ctx, table, &graph_segments);
    }

    let mut current_type = resolve_expr(&children[0], source, ctx, table);

    for child in &children[1..] {
        match child.kind() {
            "path_element" => {
                // path_element can contain graph_path, subscript, filter, or destructure
                let mut inner_cursor = child.walk();
                for inner in child.named_children(&mut inner_cursor) {
                    match inner.kind() {
                        "filter" => {
                            // filter: [WHERE condition] or [index]
                            // For graph paths, resolve WHERE against the relation table
                            let filter_table = child.prev_named_sibling()
                                .and_then(|prev| {
                                    if prev.kind() == "graph_path" {
                                        crate::parser::find_all(&prev, "identifier")
                                            .first()
                                            .map(|id| node_text(id, source).to_string())
                                    } else {
                                        None
                                    }
                                });
                            let effective_table = filter_table.as_deref().or(table);
                            current_type = resolve_filter_node(&inner, &current_type, source, ctx, effective_table);
                        }
                        "subscript" => {
                            // subscript: ('.' | '?.') (identifier | '*' | method_call)
                            let subscript_text = node_text(&inner, source);
                            let is_optional_chain = subscript_text.starts_with("?.");
                            let base_was_optional = current_type.is_nullable();

                            // Check for unnecessary optional chaining
                            if is_optional_chain && !current_type.is_any() && !base_was_optional {
                                let span = Span::from_node(&inner);
                                ctx.emit(Diagnostic::warning(
                                    span,
                                    Code::UnnecessaryOptionalChaining,
                                    format!(
                                        "unnecessary optional chaining (`?.`) on non-optional type `{}`",
                                        current_type
                                    ),
                                ));
                            }

                            // Unwrap Option for field resolution
                            let resolve_base = match &current_type {
                                Kind::Option(inner_type) => *inner_type.clone(),
                                other => other.clone(),
                            };

                            // Find the identifier being accessed
                            let mut sub_cursor = inner.walk();
                            let mut resolved = false;
                            for sub_child in inner.named_children(&mut sub_cursor) {
                                if sub_child.kind() == "identifier" {
                                    current_type = resolve_accessor(&resolve_base, &sub_child, source, ctx);
                                    resolved = true;
                                    break;
                                }
                            }
                            if !resolved {
                                current_type = resolve_expr(&inner, source, ctx, table);
                            }

                            // If base was optional, propagate optionality to result
                            if base_was_optional && !current_type.is_any() && !current_type.is_nullable() {
                                current_type = current_type.optional();
                            }
                        }
                        _ => {
                            current_type = resolve_expr(&inner, source, ctx, table);
                        }
                    }
                }
            }
            _ => {
                current_type = resolve_accessor(&current_type, child, source, ctx);
            }
        }
    }

    current_type
}

/// Collect all graph segments from a path node.
/// Returns (direction, table_name, span) for each segment.
fn collect_graph_segments<'a>(
    node: &Node<'a>,
    source: &'a str,
) -> Vec<(String, String, Span)> {
    let mut segments = Vec::new();

    fn walk_graph<'b>(node: &Node<'b>, source: &'b str, segments: &mut Vec<(String, String, Span)>) {
        if node.kind() == "graph_path" {
            let mut dir = "->".to_string();
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                match child.kind() {
                    "->" | "arrow_right" => dir = "->".to_string(),
                    "<-" | "arrow_left" => dir = "<-".to_string(),
                    "<->" | "arrow_both" => dir = "<->".to_string(),
                    "identifier" => {
                        segments.push((
                            dir.clone(),
                            node_text(&child, source).to_string(),
                            Span::from_node(&child),
                        ));
                    }
                    "graph_predicate" => {
                        // Extract the table name identifier from the predicate,
                        // but skip where_clause (field references, not table names)
                        let idents = crate::parser::find_all(&child, "identifier");
                        // The first identifier before any where_clause is the table name
                        if let Some(where_node) = crate::parser::child_by_kind(&child, "where_clause") {
                            // Only take identifiers that appear before the where_clause
                            let where_start = where_node.start_byte();
                            for ident in &idents {
                                if ident.start_byte() < where_start {
                                    segments.push((
                                        dir.clone(),
                                        node_text(ident, source).to_string(),
                                        Span::from_node(ident),
                                    ));
                                }
                            }
                        } else {
                            // No where clause, just take the first identifier
                            if let Some(ident) = idents.first() {
                                segments.push((
                                    dir.clone(),
                                    node_text(ident, source).to_string(),
                                    Span::from_node(ident),
                                ));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            // Skip where_clause nodes — identifiers inside WHERE conditions
            // are field references, not graph segment table names
            if child.kind() == "where_clause" {
                continue;
            }
            walk_graph(&child, source, segments);
        }
    }

    walk_graph(node, source, &mut segments);
    segments
}

/// Resolve a multi-hop graph traversal with schema validation.
fn resolve_multi_hop_graph(
    node: &Node,
    source: &str,
    ctx: &mut Context,
    table: Option<&str>,
    segments: &[(String, String, Span)],
) -> Kind {
    // Resolve WHERE clauses in graph_predicate nodes.
    // The WHERE should resolve fields against the RELATION table, not the FROM table.
    // e.g., ->(wrote WHERE created_at > ...) — `created_at` is on `wrote`, not `user`
    let predicates = crate::parser::find_all(node, "graph_predicate");
    for pred in &predicates {
        // Find which relation this predicate belongs to
        let relation_table = crate::parser::find_all(pred, "identifier")
            .first()
            .map(|id| node_text(id, source).to_string());
        let relation_ref = relation_table.as_deref().or(table);

        if let Some(where_node) = crate::parser::child_by_kind(pred, "where_clause") {
            resolve_where_clause(&where_node, source, ctx, relation_ref);
        }
    }

    // Resolve filter nodes (e.g., [WHERE ...] or [0]) in path_elements.
    // Filters like ->wrote[WHERE ...] should resolve against the preceding relation.
    let filters = crate::parser::find_all(node, "filter");
    for filter_node in &filters {
        // Walk up to find the preceding graph_path to get the relation name
        let relation_table = filter_node.parent()
            .and_then(|pe| pe.prev_named_sibling())
            .and_then(|prev| crate::parser::find_all(&prev, "identifier").into_iter().last())
            .map(|id| node_text(&id, source).to_string());
        let relation_ref = relation_table.as_deref().or(table);

        if let Some(where_node) = crate::parser::child_by_kind(filter_node, "where_clause") {
            resolve_where_clause(&where_node, source, ctx, relation_ref);
        } else {
            let mut cursor = filter_node.walk();
            let children: Vec<_> = filter_node.named_children(&mut cursor).collect();
            for child in &children {
                resolve_expr(child, source, ctx, relation_ref);
            }
        }
    }

    // Validate edges: segments come in pairs (edge, target)
    // source_table ->edge-> target_table ->edge2-> target2
    for (chunk_idx, pair) in segments.chunks(2).enumerate() {
        if pair.len() < 2 {
            break;
        }
        let (ref edge_dir, ref edge_name, edge_span) = pair[0];
        let (_, ref target_name, target_span) = pair[1];
        let _is_reverse = edge_dir == "<-";

        // Edge must be a relation
        if ctx.has_table(edge_name) && !ctx.is_relation(edge_name) {
            let mut diag = Diagnostic::error(
                edge_span,
                Code::NotARelation,
                format!("table `{}` is not a relation; only relations can be used in graph traversals", edge_name),
            );
            if let Some(table_def) = ctx.get_table(edge_name) {
                diag = diag.with_related(table_def.span, format!("`{}` defined as a normal table here", edge_name));
            }
            diag = diag.with_suggestion(format!(
                "define `{}` as a relation: `DEFINE TABLE {} TYPE RELATION`",
                edge_name, edge_name
            ));
            ctx.emit(diag);
        }

        // Validate source
        let source_table = if chunk_idx == 0 {
            table.map(|s| s.to_string())
        } else {
            segments.get(chunk_idx * 2 - 1).map(|s| s.1.clone())
        };
        if let Some(ref src_tbl) = source_table {
            let from_tables = ctx.get_relation_target(edge_name, true);
            if let Some(allowed) = from_tables {
                if !allowed.iter().any(|t| t == src_tbl) {
                    let from_str = allowed.join(" | ");
                    let to_str = ctx.get_relation_target(edge_name, false)
                        .map(|t| t.join(" | "))
                        .unwrap_or_else(|| "?".into());
                    let mut diag = Diagnostic::error(
                        edge_span,
                        Code::InvalidGraphEdge,
                        format!(
                            "relation `{}` connects `{}` -> `{}`, not from `{}`",
                            edge_name, from_str, to_str, src_tbl
                        ),
                    );
                    if let Some(table_def) = ctx.get_table(edge_name) {
                        diag = diag.with_related(table_def.span, format!("`{}` defined here", edge_name));
                    }
                    diag = diag.with_suggestion(format!(
                        "allowed source tables for `{}`: {}",
                        edge_name, from_str
                    ));
                    ctx.emit(diag);
                }
            }
        }

        // Validate target
        let to_tables = ctx.get_relation_target(edge_name, false);
        if let Some(allowed) = to_tables {
            if !allowed.iter().any(|t| t == target_name) {
                let from_str = ctx.get_relation_target(edge_name, true)
                    .map(|t| t.join(" | "))
                    .unwrap_or_else(|| "?".into());
                let to_str = allowed.join(" | ");
                let mut diag = Diagnostic::error(
                    target_span,
                    Code::InvalidGraphEdge,
                    format!(
                        "relation `{}` connects `{}` -> `{}`, not to `{}`",
                        edge_name, from_str, to_str, target_name
                    ),
                );
                if let Some(table_def) = ctx.get_table(edge_name) {
                    diag = diag.with_related(table_def.span, format!("`{}` defined here", edge_name));
                }
                diag = diag.with_suggestion(format!(
                    "allowed target tables for `{}`: {}",
                    edge_name, to_str
                ));
                ctx.emit(diag);
            }
        }
    }

    // Build result type from the last segment
    let (_, ref last_table, _) = segments[segments.len() - 1];
    let last_type = Kind::Array(
        Box::new(Kind::Record(vec![Table::from(last_table.clone())])),
        None,
    );

    // Build nested object type from inside out
    if segments.len() <= 1 {
        return last_type;
    }

    let mut result = last_type;
    for i in (0..segments.len() - 1).rev() {
        let key = format!("{}{}", segments[i + 1].0, segments[i + 1].1);
        let mut fields = std::collections::BTreeMap::new();
        fields.insert(key, result);
        if i == 0 {
            let outer_key = format!("{}{}", segments[i].0, segments[i].1);
            let mut outer_fields = std::collections::BTreeMap::new();
            outer_fields.insert(outer_key, Kind::Literal(Literal::Object(fields)));
            result = Kind::Literal(Literal::Object(outer_fields));
        } else {
            result = Kind::Literal(Literal::Object(fields));
        }
    }

    result
}

fn resolve_accessor(base_type: &Kind, node: &Node, source: &str, ctx: &mut Context) -> Kind {
    let accessor_name = node_text(node, source);

    match base_type {
        Kind::Literal(Literal::Object(fields)) => {
            if let Some(field_type) = fields.get(accessor_name) {
                field_type.clone()
            } else {
                Kind::Any
            }
        }
        Kind::Record(tables) => {
            // Access on record type → look up field in table schema
            if tables.len() == 1 {
                if let Some(field) = ctx.get_field(&tables[0], accessor_name) {
                    return field.typ.clone().unwrap_or(Kind::Any);
                }
            }
            Kind::Any
        }
        Kind::Array(inner, _) => {
            // Array indexing or method
            if accessor_name.parse::<usize>().is_ok() {
                *inner.clone()
            } else {
                Kind::Any
            }
        }
        Kind::Option(inner) => {
            // Optional chaining — resolve on inner type, make result optional
            let inner_result = resolve_accessor(inner, node, source, ctx);
            inner_result.optional()
        }
        _ => Kind::Any,
    }
}

// ── WHERE Clause Resolution ───────────────────────────────────

/// Resolve a where_clause node: evaluate the condition expression and warn if not boolean.
fn resolve_where_clause(
    node: &Node,
    source: &str,
    ctx: &mut Context,
    table: Option<&str>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "keyword_where" {
            let typ = resolve_expr(&child, source, ctx, table);
            if typ != Kind::Bool && typ != Kind::Any {
                let span = Span::from_node(&child);
                ctx.emit(Diagnostic::warning(
                    span,
                    Code::TypeMismatch,
                    format!("WHERE condition should be bool, got `{}`", typ),
                ));
            }
        }
    }
}

// ── Filter Node Resolution ────────────────────────────────────

/// Resolve a filter node (e.g., `[WHERE condition]` or `[0]`).
/// Returns the resulting type after filtering.
fn resolve_filter_node(
    node: &Node,
    base_type: &Kind,
    source: &str,
    ctx: &mut Context,
    table: Option<&str>,
) -> Kind {
    // filter: '[' (where_clause | value | where_clause '?' value) ']'
    if let Some(where_node) = crate::parser::child_by_kind(node, "where_clause") {
        // [WHERE condition] — filters an array, result is still the same array type
        resolve_where_clause(&where_node, source, ctx, table);
        // Filtering preserves the base type (still an array of the same element type)
        base_type.clone()
    } else {
        // [value] — could be an integer index
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            let val_type = resolve_expr(&child, source, ctx, table);
            // If it's an integer literal index, unwrap array element type
            let child_text = node_text(&child, source);
            if child_text.parse::<i64>().is_ok() {
                if let Kind::Array(inner, _) = base_type {
                    return *inner.clone();
                }
            }
            // Check for non-integer index on an array type
            if matches!(base_type, Kind::Array(_, _)) {
                if !val_type.is_any() && !val_type.is_numeric() {
                    let span = Span::from_node(&child);
                    ctx.emit(Diagnostic::warning(
                        span,
                        Code::NonIntegerArrayIndex,
                        format!(
                            "array index should be an integer, got `{}`",
                            val_type
                        ),
                    ));
                }
            }
        }
        // For non-index filters, return base type
        base_type.clone()
    }
}

// ── Graph Expression Resolution ───────────────────────────────

fn resolve_graph_expr(
    node: &Node,
    source: &str,
    ctx: &mut Context,
    table: Option<&str>,
) -> Kind {
    // Graph traversals like ->knows->person produce nested object types:
    // ->knows->person  →  { "->knows": { "->person": array<record<person>> } }
    //
    // Also validates edges against schema:
    // - The edge table must be a RELATION
    // - The source table must be in the relation's FROM list
    // - The target table must be in the relation's TO list
    let mut cursor = node.walk();
    let all_children: Vec<Node> = node.children(&mut cursor).collect();

    // Collect graph segments: each is a direction (->/<-/<->) + table name + span
    struct GraphSegment {
        dir: String,
        name: String,
        span: Span,
    }
    let mut segments: Vec<GraphSegment> = Vec::new();
    let mut current_dir = "->".to_string();

    for child in &all_children {
        match child.kind() {
            "->" | "arrow_right" => current_dir = "->".to_string(),
            "<-" | "arrow_left" => current_dir = "<-".to_string(),
            "<->" | "arrow_both" => current_dir = "<->".to_string(),
            "identifier" => {
                segments.push(GraphSegment {
                    dir: current_dir.clone(),
                    name: node_text(child, source).to_string(),
                    span: Span::from_node(child),
                });
            }
            "graph_predicate" => {
                // graph_predicate: value (edge table), optional where_clause, optional alias
                let idents = crate::parser::find_all(child, "identifier");
                for ident in &idents {
                    segments.push(GraphSegment {
                        dir: current_dir.clone(),
                        name: node_text(ident, source).to_string(),
                        span: Span::from_node(ident),
                    });
                }
                // Resolve WHERE clause if present, checking result is boolean
                if let Some(where_node) = crate::parser::child_by_kind(child, "where_clause") {
                    resolve_where_clause(&where_node, source, ctx, table);
                }
            }
            _ => {
                let idents = crate::parser::find_all(child, "identifier");
                for ident in idents {
                    segments.push(GraphSegment {
                        dir: current_dir.clone(),
                        name: node_text(&ident, source).to_string(),
                        span: Span::from_node(&ident),
                    });
                }
                // Also check for where_clause in any nested graph_predicates
                let predicates = crate::parser::find_all(child, "graph_predicate");
                for pred in &predicates {
                    if let Some(where_node) = crate::parser::child_by_kind(pred, "where_clause") {
                        resolve_where_clause(&where_node, source, ctx, table);
                    }
                }
            }
        }
    }

    if segments.is_empty() {
        return Kind::Array(Box::new(Kind::Any), None);
    }

    // Validate graph edges against schema
    // Pattern: source_table ->edge_table-> target_table
    // With 2 segments: segments[0] = edge, segments[1] = target
    // The source is `table` (the FROM clause context)
    if segments.len() >= 2 {
        // Pairs: (edge, target)
        for (chunk_idx, pair) in segments.chunks(2).enumerate() {
            if pair.len() < 2 {
                break;
            }
            let edge = &pair[0];
            let target = &pair[1];
            let is_reverse = edge.dir == "<-";

            // Edge must be a relation
            if ctx.has_table(&edge.name) && !ctx.is_relation(&edge.name) {
                let mut diag = Diagnostic::error(
                    edge.span,
                    Code::NotARelation,
                    format!("table `{}` is not a relation; only relations can be used in graph traversals", edge.name),
                );
                if let Some(table_def) = ctx.get_table(&edge.name) {
                    diag = diag.with_related(table_def.span, format!("`{}` defined as a normal table here", edge.name));
                }
                diag = diag.with_suggestion(format!(
                    "define `{}` as a relation: `DEFINE TABLE {} TYPE RELATION`",
                    edge.name, edge.name
                ));
                ctx.emit(diag);
            }

            // Validate source table is allowed by the relation's FROM
            // For the first pair, source is the FROM clause table context
            // For chained traversals, source is the previous target
            let source_table = if chunk_idx == 0 {
                table.map(|s| s.to_string())
            } else {
                segments.get(chunk_idx * 2 - 1).map(|s| s.name.clone())
            };
            if let Some(source_table) = source_table {
                let from_tables = if is_reverse {
                    ctx.get_relation_target(&edge.name, true) // FROM tables for reverse
                } else {
                    ctx.get_relation_target(&edge.name, true) // FROM = source direction
                };

                if let Some(allowed) = from_tables {
                    if !allowed.iter().any(|t| t == &source_table) {
                        let from_str = allowed.join(" | ");
                        let to_str = ctx.get_relation_target(&edge.name, false)
                            .map(|t| t.join(" | "))
                            .unwrap_or_else(|| "?".into());
                        let mut diag = Diagnostic::error(
                            edge.span,
                            Code::InvalidGraphEdge,
                            format!(
                                "relation `{}` connects `{}` -> `{}`, not from `{}`",
                                edge.name, from_str, to_str, source_table
                            ),
                        );
                        if let Some(table_def) = ctx.get_table(&edge.name) {
                            diag = diag.with_related(table_def.span, format!("`{}` defined here", edge.name));
                        }
                        diag = diag.with_suggestion(format!(
                            "allowed source tables for `{}`: {}",
                            edge.name, from_str
                        ));
                        ctx.emit(diag);
                    }
                }
            }

            // Validate target table is allowed by the relation's TO
            let to_tables = if is_reverse {
                ctx.get_relation_target(&edge.name, false) // reverse: TO becomes "from" perspective
            } else {
                ctx.get_relation_target(&edge.name, false) // TO tables
            };

            if let Some(allowed) = to_tables {
                if !allowed.iter().any(|t| t == &target.name) {
                    let from_str = ctx.get_relation_target(&edge.name, true)
                        .map(|t| t.join(" | "))
                        .unwrap_or_else(|| "?".into());
                    let to_str = allowed.join(" | ");
                    let mut diag = Diagnostic::error(
                        target.span,
                        Code::InvalidGraphEdge,
                        format!(
                            "relation `{}` connects `{}` -> `{}`, not to `{}`",
                            edge.name, from_str, to_str, target.name
                        ),
                    );
                    if let Some(table_def) = ctx.get_table(&edge.name) {
                        diag = diag.with_related(table_def.span, format!("`{}` defined here", edge.name));
                    }
                    diag = diag.with_suggestion(format!(
                        "allowed target tables for `{}`: {}",
                        edge.name, to_str
                    ));
                    ctx.emit(diag);
                }
            }
        }
    }

    // Build result type
    if segments.len() == 1 {
        let ref table_name = segments[0].name;
        return Kind::Array(
            Box::new(Kind::Record(vec![Table::from(table_name.clone())])),
            None,
        );
    }

    // For multi-segment, build nested object type from inside out
    let ref last_table = segments[segments.len() - 1].name;
    let last_type = Kind::Array(
        Box::new(Kind::Record(vec![Table::from(last_table.clone())])),
        None,
    );

    let mut result = last_type;
    for i in (0..segments.len() - 1).rev() {
        let key = format!("{}{}", segments[i + 1].dir, segments[i + 1].name);
        let mut fields = std::collections::BTreeMap::new();
        fields.insert(key, result);
        if i == 0 {
            let outer_key = format!("{}{}", segments[i].dir, segments[i].name);
            let mut outer_fields = std::collections::BTreeMap::new();
            outer_fields.insert(outer_key, Kind::Literal(Literal::Object(fields)));
            result = Kind::Literal(Literal::Object(outer_fields));
        } else {
            result = Kind::Literal(Literal::Object(fields));
        }
    }

    result
}

// ── Method Call Resolution ────────────────────────────────────

fn resolve_method_call(
    receiver_type: &Kind,
    method_name: &str,
    _node: &Node,
    _source: &str,
    _ctx: &mut Context,
    _table: Option<&str>,
) -> Kind {
    // Method calls are syntactic sugar for namespace functions
    // e.g., "hello".uppercase() → string::uppercase("hello")
    match receiver_type {
        Kind::String => match method_name {
            "len" | "length" => Kind::Int,
            "uppercase" | "lowercase" | "trim" | "slug" | "reverse" => Kind::String,
            "split" => Kind::Array(Box::new(Kind::String), None),
            "contains" | "starts_with" | "ends_with" => Kind::Bool,
            "to_int" | "to_float" | "to_decimal" => Kind::Number,
            _ => Kind::Any,
        },
        Kind::Array(inner, _) => match method_name {
            "len" | "length" => Kind::Int,
            "first" | "last" => inner.clone().optional(),
            "flatten" => {
                if let Kind::Array(nested, _) = inner.as_ref() {
                    Kind::Array(nested.clone(), None)
                } else {
                    Kind::Array(inner.clone(), None)
                }
            }
            "reverse" | "sort" | "distinct" | "unique" => {
                Kind::Array(inner.clone(), None)
            }
            "map" | "filter" | "find" => Kind::Any, // depends on closure
            "any" | "all" | "some" | "none" => Kind::Bool,
            "push" | "append" => Kind::Null,
            "join" | "concat" => Kind::String,
            _ => Kind::Any,
        },
        Kind::Literal(Literal::Object(_)) | Kind::Object => match method_name {
            "keys" => Kind::Array(Box::new(Kind::String), None),
            "values" => Kind::Array(Box::new(Kind::Any), None),
            "entries" => Kind::Array(Box::new(Kind::Array(Box::new(Kind::Any), None)), None),
            "len" | "length" => Kind::Int,
            _ => Kind::Any,
        },
        _ => Kind::Any,
    }
}

// ── LET Resolution ────────────────────────────────────────────

fn resolve_let(node: &Node, source: &str, ctx: &mut Context, table: Option<&str>) {
    let mut cursor = node.walk();
    let children: Vec<Node> = node.named_children(&mut cursor).collect();

    // Check for object destructure binding
    let mut destructure_node = None;
    let mut var_name = None;
    let mut var_span = Span::new(0, 0);
    let mut val_type = Kind::Any;

    for child in &children {
        if child.kind() == "object_destructure" {
            destructure_node = Some(*child);
        } else if child.kind() == "variable" || child.kind() == "parameter" || child.kind() == "variable_name" {
            let name = node_text(child, source);
            var_name = Some(if name.starts_with('$') {
                name.to_string()
            } else {
                format!("${}", name)
            });
            var_span = Span::from_node(child);
        } else if child.kind() != "keyword_let" && child.kind() != "operator" && child.kind() != "=" {
            val_type = resolve_expr(child, source, ctx, table);
        }
    }

    if let Some(destr) = destructure_node {
        bind_destructure(&destr, source, ctx, &val_type, BindingKind::Let);
    } else if let Some(name) = var_name {
        // Check for shadowing
        if let Some(existing) = ctx.scope.would_shadow(&name) {
            let existing_span = existing.span;
            ctx.emit(
                Diagnostic::warning(var_span, Code::VariableShadow, format!("variable `{}` shadows a previous binding", name))
                    .with_related(existing_span, "previously defined here"),
            );
        }

        ctx.scope.bind(Binding {
            name,
            typ: val_type,
            span: var_span,
            mutable: true,
            kind: BindingKind::Let,
        });
    }
}

/// Bind variables from an object_destructure node.
/// Extracts field types from `source_type` if it's a known object type.
fn bind_destructure(
    node: &Node,
    source: &str,
    ctx: &mut Context,
    source_type: &Kind,
    binding_kind: BindingKind,
) {
    // Extract field map from source type
    let fields: Option<&std::collections::BTreeMap<String, Kind>> = match source_type {
        Kind::Literal(Literal::Object(fields)) => Some(fields),
        _ => None,
    };

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "destructure_binding" {
            let mut inner = child.walk();
            let mut var_node = None;
            for c in child.named_children(&mut inner) {
                if c.kind() == "variable_name" {
                    var_node = Some(c);
                }
            }
            if let Some(vn) = var_node {
                let raw_name = node_text(&vn, source);
                let var_name = if raw_name.starts_with('$') {
                    raw_name.to_string()
                } else {
                    format!("${}", raw_name)
                };
                let field_key = raw_name.trim_start_matches('$');
                let var_span = Span::from_node(&vn);

                // Look up field type from source object
                let field_type = if let Some(field_map) = fields {
                    if let Some(ft) = field_map.get(field_key) {
                        ft.clone()
                    } else {
                        // Field not found in source type — emit warning
                        ctx.emit(Diagnostic::warning(
                            var_span,
                            Code::FieldNotFound,
                            format!(
                                "destructured field `{}` not found in source type `{}`",
                                field_key, source_type
                            ),
                        ));
                        Kind::Any
                    }
                } else {
                    Kind::Any
                };

                // Check for shadowing
                if let Some(existing) = ctx.scope.would_shadow(&var_name) {
                    let existing_span = existing.span;
                    ctx.emit(
                        Diagnostic::warning(
                            var_span,
                            Code::VariableShadow,
                            format!("variable `{}` shadows a previous binding", var_name),
                        )
                        .with_related(existing_span, "previously defined here"),
                    );
                }

                ctx.scope.bind(Binding {
                    name: var_name,
                    typ: field_type,
                    span: var_span,
                    mutable: binding_kind == BindingKind::Let,
                    kind: binding_kind,
                });
            }
        }
    }
}

// ── FOR Resolution ────────────────────────────────────────────

fn resolve_for(node: &Node, source: &str, ctx: &mut Context, table: Option<&str>) {
    ctx.scope.push(crate::scope::ScopeKind::ForLoop);

    let mut cursor = node.walk();
    let children: Vec<Node> = node.named_children(&mut cursor).collect();

    let mut iter_var = None;
    let mut iter_span = Span::new(0, 0);
    let mut destructure_node = None;
    let mut found_in = false;

    for child in &children {
        if child.kind() == "object_destructure" && !found_in {
            destructure_node = Some(*child);
        } else if child.kind() == "variable" || child.kind() == "parameter" || child.kind() == "variable_name" {
            if !found_in {
                let name = node_text(child, source);
                iter_var = Some(if name.starts_with('$') {
                    name.to_string()
                } else {
                    format!("${}", name)
                });
                iter_span = Span::from_node(child);
            }
        } else if child.kind() == "keyword_in" {
            found_in = true;
        } else if found_in && (iter_var.is_some() || destructure_node.is_some()) {
            if child.kind() == "block" || child.kind() == "block_expression" {
                // Resolve the loop body (after bindings are set up)
                resolve_expr(child, source, ctx, table);
                continue;
            }
            // This is the iterable expression
            let iterable_type = resolve_expr(child, source, ctx, table);
            let element_type = match &iterable_type {
                Kind::Array(inner, _) | Kind::Set(inner, _) => *inner.clone(),
                Kind::Range | Kind::Any => Kind::Any,
                other => {
                    let span = Span::from_node(child);
                    ctx.emit(Diagnostic::warning(
                        span,
                        Code::TypeMismatch,
                        format!("cannot iterate over type `{}`, expected array or set", other),
                    ));
                    Kind::Any
                }
            };

            if let Some(destr) = destructure_node.take() {
                bind_destructure(&destr, source, ctx, &element_type, BindingKind::ForLoop);
            } else if let Some(name) = iter_var.take() {
                ctx.scope.bind(Binding {
                    name,
                    typ: element_type,
                    span: iter_span,
                    mutable: false,
                    kind: BindingKind::ForLoop,
                });
            }
            found_in = false;
        } else if child.kind() == "block" || child.kind() == "block_expression" {
            // Resolve the loop body
            resolve_expr(child, source, ctx, table);
        }
    }

    ctx.scope.pop();
}

// ── Type Utilities ────────────────────────────────────────────

/// Unify a list of types into a single type.
/// If all types are the same, returns that type.
/// Otherwise returns an Either union.
pub fn unify_types(types: &[Kind]) -> Kind {
    if types.is_empty() {
        return Kind::Any;
    }

    let mut unique: Vec<Kind> = Vec::new();
    for t in types {
        if !unique.contains(t) && *t != Kind::Any {
            unique.push(t.clone());
        }
    }

    match unique.len() {
        0 => Kind::Any,
        1 => unique.into_iter().next().unwrap(),
        _ => Kind::Either(unique),
    }
}

fn type_name_to_type(name: &str) -> Option<Kind> {
    match name {
        "any" => Some(Kind::Any),
        "null" | "none" => Some(Kind::Null),
        "bool" => Some(Kind::Bool),
        "int" => Some(Kind::Int),
        "float" => Some(Kind::Float),
        "decimal" => Some(Kind::Decimal),
        "number" => Some(Kind::Number),
        "string" => Some(Kind::String),
        "bytes" => Some(Kind::Bytes),
        "duration" => Some(Kind::Duration),
        "datetime" => Some(Kind::Datetime),
        "uuid" => Some(Kind::Uuid),
        "object" => Some(Kind::Object),
        "range" => Some(Kind::Range),
        "regex" => Some(crate::types::regex_kind()),
        "array" => Some(Kind::Array(Box::new(Kind::Any), None)),
        "set" => Some(Kind::Set(Box::new(Kind::Any), None)),
        "option" => Some(Kind::Option(Box::new(Kind::Any))),
        "record" => Some(Kind::Record(vec![])),
        "geometry" => Some(Kind::Geometry(vec![])),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::diagnostic::Code;

    fn has_diagnostic(source: &str, code: Code, substring: &str) -> bool {
        let result = crate::analyze(source).unwrap();
        result
            .diagnostics
            .iter()
            .any(|d| d.code == code && d.message.contains(substring))
    }

    #[allow(dead_code)]
    fn no_diagnostic(source: &str, code: Code) -> bool {
        let result = crate::analyze(source).unwrap();
        !result.diagnostics.iter().any(|d| d.code == code)
    }

    // ── Comparison operator type validation ──────────────────

    #[test]
    fn comparison_incompatible_types_warns() {
        let src = "LET $r = IF true == 5s THEN 1 ELSE 0 END;";
        assert!(
            has_diagnostic(src, Code::TypeMismatch, "comparing incompatible types"),
            "expected warning for bool vs duration comparison"
        );
    }

    #[test]
    fn comparison_compatible_numeric_types_no_warn() {
        // int vs float — both numeric, should not warn
        let src = "LET $r = IF 1 == 1.5f THEN 1 ELSE 0 END;";
        let result = crate::analyze(src).unwrap();
        let has_cmp_warn = result.diagnostics.iter().any(|d| {
            d.code == Code::TypeMismatch && d.message.contains("comparing incompatible")
        });
        assert!(!has_cmp_warn, "should not warn for int vs float comparison");
    }

    #[test]
    fn comparison_string_vs_int_warns() {
        // string vs int — these are incompatible types, should warn
        let src = r#"LET $r = IF "hello" == 42 THEN 1 ELSE 0 END;"#;
        let result = crate::analyze(src).unwrap();
        let has_cmp_warn = result.diagnostics.iter().any(|d| {
            d.code == Code::TypeMismatch && d.message.contains("comparing incompatible")
        });
        assert!(has_cmp_warn, "should warn for string vs int comparison");
    }

    // ── Logical operator type validation ─────────────────────

    #[test]
    fn logical_operator_non_bool_warns() {
        // Use LET to exercise logical operator with non-bool operands
        let src = "LET $r = IF 1 AND 2 THEN 1 ELSE 0 END;";
        assert!(
            has_diagnostic(src, Code::TypeMismatch, "`AND` expects `bool` operands"),
            "expected warning for non-bool logical operands"
        );
    }

    #[test]
    fn logical_operator_bool_no_warn() {
        let src = "LET $r = IF true AND false THEN 1 ELSE 0 END;";
        let result = crate::analyze(src).unwrap();
        let has_logical_warn = result.diagnostics.iter().any(|d| {
            d.code == Code::TypeMismatch && d.message.contains("expects `bool` operands")
        });
        assert!(!has_logical_warn, "should not warn for bool AND bool");
    }

    // ── NOT operator type check ──────────────────────────────

    #[test]
    fn not_operator_non_bool_warns() {
        // Use negation on a cast-to-int variable
        let src = "LET $x = <int> 42; LET $y = !$x;";
        let result = crate::analyze(src).unwrap();
        assert!(
            result.diagnostics.iter().any(|d| d.code == Code::TypeMismatch && d.message.contains("`NOT` expects `bool` operand")),
            "expected warning for NOT on non-bool, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn not_operator_bool_no_warn() {
        let src = "LET $x = true; LET $y = !$x;";
        let result = crate::analyze(src).unwrap();
        let has_not_warn = result.diagnostics.iter().any(|d| {
            d.code == Code::TypeMismatch && d.message.contains("`NOT` expects `bool`")
        });
        assert!(!has_not_warn, "should not warn for NOT on bool");
    }

    // ── FOR loop iterable type check ─────────────────────────

    #[test]
    fn for_loop_non_iterable_errors() {
        let src = r#"
            DEFINE TABLE t SCHEMAFULL;
            DEFINE FIELD age ON t TYPE int;
            SELECT * FROM t;
            FOR $item IN 42 {
                RETURN $item;
            };
        "#;
        assert!(
            has_diagnostic(src, Code::TypeMismatch, "cannot iterate over type"),
            "expected error for iterating over int"
        );
    }

    #[test]
    fn for_loop_array_no_error() {
        let src = r#"
            FOR $item IN [1, 2, 3] {
                RETURN $item;
            };
        "#;
        let result = crate::analyze(src).unwrap();
        let has_iter_err = result.diagnostics.iter().any(|d| {
            d.code == Code::TypeMismatch && d.message.contains("cannot iterate")
        });
        assert!(!has_iter_err, "should not error for iterating over array");
    }

    // ── Closure parameter type inference for array methods ────

    #[test]
    fn array_map_infers_closure_param() {
        // array::map on array<string> with closure using string::uppercase
        // should not produce type errors since $v is inferred as string
        let src = r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD tags ON user TYPE array<string>;
            SELECT array::map(tags, |$v| string::uppercase($v)) FROM user;
        "#;
        let result = crate::analyze(src).unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error())
            .collect();
        assert!(
            errors.is_empty(),
            "expected no errors for array::map with typed closure param, got: {:?}",
            errors
        );
    }

    #[test]
    fn array_filter_returns_same_element_type() {
        // array::filter on array<int> should return array<int>, not array<any>
        let src = r#"
            DEFINE TABLE data SCHEMAFULL;
            DEFINE FIELD nums ON data TYPE array<int>;
            SELECT array::filter(nums, |$v| $v > 0) FROM data;
        "#;
        let result = crate::analyze(src).unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error())
            .collect();
        assert!(
            errors.is_empty(),
            "expected no errors for array::filter, got: {:?}",
            errors
        );
    }

    #[test]
    fn array_filter_warns_non_bool_closure() {
        // array::filter closure should warn if it returns non-bool
        let src = r#"
            DEFINE TABLE data SCHEMAFULL;
            DEFINE FIELD nums ON data TYPE array<int>;
            SELECT array::filter(nums, |$v| $v + 1) FROM data;
        "#;
        let result = crate::analyze(src).unwrap();
        let has_warn = result.diagnostics.iter().any(|d| {
            d.code == Code::WrongArgType
                && d.message.contains("array::filter")
                && d.message.contains("should return")
        });
        assert!(
            has_warn,
            "expected warning for array::filter closure returning non-bool, diagnostics: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn array_find_returns_option_element() {
        // array::find on array<string> should return option<string>
        let src = r#"
            DEFINE TABLE data SCHEMAFULL;
            DEFINE FIELD names ON data TYPE array<string>;
            SELECT array::find(names, |$v| $v == "test") FROM data;
        "#;
        let result = crate::analyze(src).unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error())
            .collect();
        assert!(
            errors.is_empty(),
            "expected no errors for array::find, got: {:?}",
            errors
        );
    }

    #[test]
    fn array_map_closure_return_type_propagates() {
        // array::map should return array<return_type_of_closure>
        // If closure returns string (via string::uppercase), result should be array<string>
        let src = r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD tags ON user TYPE array<string>;
            LET $result = array::map(tags, |$v| string::uppercase($v));
        "#;
        let result = crate::analyze(src).unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error())
            .collect();
        assert!(
            errors.is_empty(),
            "expected no errors, got: {:?}",
            errors
        );
    }

    #[test]
    fn unreachable_after_return_warns() {
        let result = crate::analyze(
            r#"
            DEFINE FUNCTION fn::test() -> int {
                RETURN 42;
                LET $x = 1;
            };
        "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("unreachable"))
            .collect();
        assert!(!warnings.is_empty(), "Expected unreachable code warning after RETURN, got: {:?}", result.diagnostics);
    }

    #[test]
    fn no_unreachable_without_return() {
        let result = crate::analyze(
            r#"
            DEFINE FUNCTION fn::test() -> int {
                LET $x = 1;
                RETURN $x;
            };
        "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("unreachable"))
            .collect();
        assert!(warnings.is_empty(), "Expected no unreachable warnings, got: {:?}", warnings);
    }

    #[test]
    fn unreachable_after_throw_warns() {
        let result = crate::analyze(
            r#"
            DEFINE FUNCTION fn::test() -> int {
                THROW "error";
                RETURN 42;
            };
        "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("unreachable"))
            .collect();
        assert!(!warnings.is_empty(), "Expected unreachable code warning after THROW, got: {:?}", result.diagnostics);
    }

    // ── geo:: argument type validation ─────────────────────────

    #[test]
    fn geo_distance_requires_geometry() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE t SCHEMAFULL;
            DEFINE FIELD x ON t TYPE string;
            SELECT * FROM t WHERE geo::distance(x, x) > 0;
        "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::WrongArgType)
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected type warning for non-geometry args to geo::distance, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn geo_bearing_requires_geometry() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE t SCHEMAFULL;
            DEFINE FIELD x ON t TYPE int;
            SELECT * FROM t WHERE geo::bearing(x, x) > 0;
        "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::WrongArgType)
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected type warning for non-geometry args to geo::bearing, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn geo_hash_decode_requires_string() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE t SCHEMAFULL;
            DEFINE FIELD x ON t TYPE int;
            SELECT * FROM t WHERE geo::hash::decode(x) != NONE;
        "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::WrongArgType)
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected type warning for non-string arg to geo::hash::decode, got: {:?}",
            result.diagnostics
        );
    }

    // ── encoding:: argument type validation ────────────────────

    #[test]
    fn encoding_base64_decode_requires_string() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE t SCHEMAFULL;
            DEFINE FIELD x ON t TYPE int;
            SELECT * FROM t WHERE encoding::base64::decode(x) != "";
        "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::WrongArgType)
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected type warning for non-string arg to encoding::base64::decode, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn encoding_base64_encode_requires_string() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE t SCHEMAFULL;
            DEFINE FIELD x ON t TYPE int;
            SELECT * FROM t WHERE encoding::base64::encode(x) != "";
        "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::WrongArgType)
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected type warning for non-string arg to encoding::base64::encode, got: {:?}",
            result.diagnostics
        );
    }

    // ── parse:: argument type validation ───────────────────────

    #[test]
    fn parse_email_requires_string() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE t SCHEMAFULL;
            DEFINE FIELD x ON t TYPE int;
            SELECT * FROM t WHERE parse::email::host(x) != "";
        "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::WrongArgType)
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected type warning for non-string arg to parse::email::host, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn parse_url_domain_requires_string() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE t SCHEMAFULL;
            DEFINE FIELD x ON t TYPE int;
            SELECT * FROM t WHERE parse::url::domain(x) != "";
        "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::WrongArgType)
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected type warning for non-string arg to parse::url::domain, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn parse_url_port_requires_string() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE t SCHEMAFULL;
            DEFINE FIELD x ON t TYPE int;
            SELECT * FROM t WHERE parse::url::port(x) > 0;
        "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::WrongArgType)
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected type warning for non-string arg to parse::url::port, got: {:?}",
            result.diagnostics
        );
    }

    // ── Array indexing with non-integer ──────────────────────

    #[test]
    fn array_index_with_string_warns() {
        // Indexing an array with a string should warn
        let result = crate::analyze(
            r#"
            DEFINE TABLE t SCHEMAFULL;
            DEFINE FIELD items ON t TYPE array<string>;
            SELECT items["hello"] FROM t;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::NonIntegerArrayIndex)
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected non-integer array index warning, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn array_index_with_bool_warns() {
        // Indexing an array with a bool should warn
        let result = crate::analyze(
            r#"
            DEFINE TABLE t SCHEMAFULL;
            DEFINE FIELD items ON t TYPE array<int>;
            SELECT items[true] FROM t;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::NonIntegerArrayIndex)
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected non-integer array index warning for bool, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn array_index_with_integer_no_warn() {
        // Indexing an array with an integer should not warn
        let result = crate::analyze(
            r#"
            DEFINE TABLE t SCHEMAFULL;
            DEFINE FIELD items ON t TYPE array<string>;
            SELECT items[0] FROM t;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::NonIntegerArrayIndex)
            .collect();
        assert!(
            warnings.is_empty(),
            "Integer array index should not warn: {:?}",
            warnings
        );
    }

    #[test]
    fn array_where_filter_no_warn() {
        // WHERE filter on an array should not produce non-integer index warning
        let result = crate::analyze(
            r#"
            DEFINE TABLE t SCHEMAFULL;
            DEFINE FIELD items ON t TYPE array<string>;
            SELECT items[WHERE true] FROM t;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::NonIntegerArrayIndex)
            .collect();
        assert!(
            warnings.is_empty(),
            "WHERE filter should not produce non-integer index warning: {:?}",
            warnings
        );
    }

    // ── Optional chaining on non-optional ────────────────────

    #[test]
    fn optional_chaining_on_non_optional_warns() {
        // Using ?. on a non-optional string field should warn
        let result = crate::analyze(
            r#"
            DEFINE TABLE t SCHEMAFULL;
            DEFINE FIELD meta ON t TYPE object;
            SELECT meta?.name FROM t;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::UnnecessaryOptionalChaining)
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected unnecessary optional chaining warning, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn optional_chaining_on_optional_no_warn() {
        // Using ?. on an option<object> field should not warn
        let result = crate::analyze(
            r#"
            DEFINE TABLE t SCHEMAFULL;
            DEFINE FIELD meta ON t TYPE option<object>;
            SELECT meta?.name FROM t;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::UnnecessaryOptionalChaining)
            .collect();
        assert!(
            warnings.is_empty(),
            "Optional chaining on option type should not warn: {:?}",
            warnings
        );
    }

    #[test]
    fn regular_dot_access_no_optional_warn() {
        // Using regular . access should not produce optional chaining warning
        let result = crate::analyze(
            r#"
            DEFINE TABLE t SCHEMAFULL;
            DEFINE FIELD meta ON t TYPE object;
            SELECT meta.name FROM t;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::UnnecessaryOptionalChaining)
            .collect();
        assert!(
            warnings.is_empty(),
            "Regular dot access should not warn about optional chaining: {:?}",
            warnings
        );
    }

    #[test]
    fn cast_unknown_type_warns() {
        // <foobar>42 should warn about unknown type
        assert!(has_diagnostic(
            "LET $x = <foobar>42;",
            Code::UnknownCastType,
            "unknown cast target type"
        ));
    }

    #[test]
    fn cast_known_type_no_warning() {
        // <int>"42" should not warn about unknown type
        assert!(!has_diagnostic(
            r#"LET $x = <int>"42";"#,
            Code::UnknownCastType,
            "unknown"
        ));
    }

    // ── Regex cast tests ──────────────────────────────────────

    #[test]
    fn cast_regex_no_unknown_type_warning() {
        // <regex>"test" should not warn about unknown cast type
        assert!(!has_diagnostic(
            r#"LET $x = <regex>"test";"#,
            Code::UnknownCastType,
            "unknown"
        ));
    }

    #[test]
    fn cast_regex_from_string_no_error() {
        let result = crate::analyze(r#"LET $x = <regex>"test.*";"#).unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "Unexpected errors for <regex> cast: {:?}", errors);
    }

    // ── Destructuring tests ──────────────────────────────────

    #[test]
    fn let_destructure_binds_variables() {
        let result = crate::analyze(r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            LET { $name, $age } = (SELECT * FROM user ONLY);
            RETURN $name;
        "#).unwrap();
        // Should not have undefined variable errors for $name
        let undef: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == Code::UndefinedVariable && d.message.contains("name"))
            .collect();
        assert!(undef.is_empty(), "Expected $name to be bound by destructuring, got: {:?}", result.diagnostics);
    }

    #[test]
    fn let_destructure_field_not_found_warns() {
        let result = crate::analyze(r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            LET $obj = { name: "test", age: 42 };
            LET { $name, $nonexistent } = $obj;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == Code::FieldNotFound && d.message.contains("nonexistent"))
            .collect();
        assert!(!warnings.is_empty(),
            "Expected FieldNotFound for destructured field 'nonexistent', got: {:?}",
            result.diagnostics);
    }

    #[test]
    fn let_destructure_infers_field_types() {
        // Verify that destructured variables get correct types:
        // $name should be string, $age should be int
        // Comparing $name > 5 should warn (string vs int comparison)
        let result = crate::analyze(r#"
            LET $obj = { name: "test", age: 42 };
            LET { $name, $age } = $obj;
            LET $cmp = $name > 5;
        "#).unwrap();
        let type_errors: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == Code::TypeMismatch && d.message.contains("comparing"))
            .collect();
        assert!(!type_errors.is_empty(),
            "Expected comparison type mismatch for string > int, got: {:?}",
            result.diagnostics);
    }

    #[test]
    fn for_destructure_binds_variables() {
        let result = crate::analyze(r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            FOR { $name, $age } IN (SELECT * FROM user) {
                RETURN $name;
            };
        "#).unwrap();
        let undef: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == Code::UndefinedVariable && d.message.contains("name"))
            .collect();
        assert!(undef.is_empty(), "Expected $name to be bound by FOR destructuring, got: {:?}", result.diagnostics);
    }

    #[test]
    fn let_destructure_non_object_source() {
        // Destructuring from a non-object source should still work (binds as Any)
        let result = crate::analyze(r#"
            LET { $a, $b } = 42;
            RETURN $a;
        "#).unwrap();
        let undef: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == Code::UndefinedVariable && d.message.contains("$a"))
            .collect();
        assert!(undef.is_empty(), "Expected $a to be bound even from non-object source");
    }
}
