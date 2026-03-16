/// Inlay hint collection for the SurrealGuard analyzer.
///
/// Walks a parsed CST and collects type annotations for:
/// - LET bindings (inferred type of the assigned expression)
/// - SELECT/CREATE/UPDATE/DELETE/UPSERT/INSERT/RELATE statements (result type)
/// - FOR loop variables (element type)
use tree_sitter::Node;

use crate::context::Context;
use crate::resolve::resolve_expr;
use crate::types::{display_kind, Kind, KindExt};

/// A type hint to display inline in the editor.
#[derive(Debug, Clone)]
pub struct TypeHint {
    /// Byte offset where the hint should be displayed (after this position).
    pub position: u32,
    /// The type label to display.
    pub label: String,
    /// What kind of hint this is.
    pub kind: TypeHintKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeHintKind {
    /// Type annotation for a variable binding (LET $x = ...)
    Variable,
    /// Result type for a DML statement (SELECT, CREATE, etc.)
    Statement,
}

/// Collect all type hints for a document.
///
/// Must be called AFTER `analyze_with_context()` so that the context
/// has full schema and scope information.
pub fn collect_type_hints(root: &Node, source: &str, ctx: &mut Context) -> Vec<TypeHint> {
    let mut hints = Vec::new();
    collect_hints_recursive(root, source, ctx, &mut hints);
    // Discard any diagnostics emitted during re-resolution
    ctx.take_diagnostics();
    hints
}

fn collect_hints_recursive(
    node: &Node,
    source: &str,
    ctx: &mut Context,
    hints: &mut Vec<TypeHint>,
) {
    match node.kind() {
        "let_statement" => collect_let_hint(node, source, ctx, hints),
        "for_statement" => collect_for_hint(node, source, ctx, hints),
        // Only hint standalone DML statements (not inside LET, RETURN, subqueries).
        // If the statement is a direct child of the source_file / expressions,
        // it's standalone. Otherwise it's part of a larger expression — skip it.
        "select_statement" | "create_statement" | "update_statement"
        | "delete_statement" | "upsert_statement" | "insert_statement"
        | "relate_statement" => {
            let is_standalone = node.parent()
                .map(|p| matches!(p.kind(), "source_file" | "expressions" | "expression" | "block"))
                .unwrap_or(true);
            if is_standalone {
                collect_statement_hint(node, source, ctx, hints, "");
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_hints_recursive(&child, source, ctx, hints);
    }
}

fn collect_let_hint(node: &Node, source: &str, ctx: &mut Context, hints: &mut Vec<TypeHint>) {
    let mut cursor = node.walk();
    let children: Vec<Node> = node.named_children(&mut cursor).collect();

    let mut var_end: Option<u32> = None;
    let mut val_type = Kind::Any;

    for child in &children {
        if child.kind() == "variable" || child.kind() == "parameter" || child.kind() == "variable_name" {
            var_end = Some(child.end_byte() as u32);
        } else if child.kind() == "object_destructure" {
            // Don't show hints for destructured bindings — the individual fields are typed
            return;
        } else if child.kind() != "keyword_let" && child.kind() != "operator" && child.kind() != "=" {
            val_type = resolve_expr(child, source, ctx, None);
        }
    }

    if let Some(end) = var_end {
        if !val_type.is_any() {
            let label = format_type_label(&val_type);
            if !label.is_empty() {
                hints.push(TypeHint {
                    position: end,
                    label: format!(" <{label}>"),
                    kind: TypeHintKind::Variable,
                });
            }
        }
    }
}

fn collect_for_hint(node: &Node, source: &str, ctx: &mut Context, hints: &mut Vec<TypeHint>) {
    let mut cursor = node.walk();
    let children: Vec<Node> = node.named_children(&mut cursor).collect();

    let mut var_end: Option<u32> = None;
    let mut iterable_type = Kind::Any;

    for child in &children {
        if child.kind() == "variable" || child.kind() == "parameter" || child.kind() == "variable_name" {
            var_end = Some(child.end_byte() as u32);
        } else if child.kind() == "object_destructure" {
            return;
        } else if child.kind() != "keyword_for" && child.kind() != "keyword_in" && child.kind() != "block" {
            iterable_type = resolve_expr(child, source, ctx, None);
        }
    }

    // Extract element type from iterable
    let element_type = match &iterable_type {
        Kind::Array(inner, _) => inner.as_ref().clone(),
        Kind::Set(inner, _) => inner.as_ref().clone(),
        _ if !iterable_type.is_any() => iterable_type,
        _ => return,
    };

    if let Some(end) = var_end {
        if !element_type.is_any() {
            let label = format_type_label(&element_type);
            if !label.is_empty() {
                hints.push(TypeHint {
                    position: end,
                    label: format!(" <{label}>"),
                    kind: TypeHintKind::Variable,
                });
            }
        }
    }
}

fn collect_statement_hint(
    node: &Node,
    source: &str,
    ctx: &mut Context,
    hints: &mut Vec<TypeHint>,
    _stmt_kind: &str,
) {
    // Re-resolve the statement to get its result type
    let result_type = resolve_expr(node, source, ctx, None);

    if !result_type.is_any() {
        let label = format_type_label(&result_type);
        if !label.is_empty() {
            // Place hint at the end of the first keyword (e.g., after "SELECT", "CREATE")
            let mut cursor = node.walk();
            let first_keyword_end = node.children(&mut cursor)
                .find(|c| c.kind().starts_with("keyword_"))
                .map(|c| c.end_byte() as u32)
                .unwrap_or(node.start_byte() as u32 + 6);

            hints.push(TypeHint {
                position: first_keyword_end,
                label: format!(" <{label}>"),
                kind: TypeHintKind::Statement,
            });
        }
    }
}

/// Format a Kind into a concise type label for inlay hints.
///
/// Keeps simple types readable, abbreviates complex objects to avoid
/// massive inline annotations that obscure the code.
fn format_type_label(kind: &Kind) -> String {
    match kind {
        // Object literals with fields → show field names (without types for brevity)
        Kind::Literal(crate::types::Literal::Object(fields)) => {
            if fields.len() <= 3 {
                let field_strs: Vec<String> = fields
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k, display_kind(v)))
                    .collect();
                format!("{{ {} }}", field_strs.join(", "))
            } else {
                // Show field names only, truncated
                let names: Vec<&String> = fields.keys().collect();
                let shown: Vec<String> = names.iter().take(4).map(|n| n.to_string()).collect();
                if names.len() > 4 {
                    format!("{{ {}, ... }}", shown.join(", "))
                } else {
                    format!("{{ {} }}", shown.join(", "))
                }
            }
        }
        // Arrays of objects → abbreviate the element type
        Kind::Array(inner, _) => {
            let inner_label = format_type_label(inner);
            format!("array<{}>", inner_label)
        }
        // Sets of objects → same treatment
        Kind::Set(inner, _) => {
            let inner_label = format_type_label(inner);
            format!("set<{}>", inner_label)
        }
        // Option wrapping complex type
        Kind::Option(inner) => {
            let inner_label = format_type_label(inner);
            format!("option<{}>", inner_label)
        }
        // Everything else: use the standard display
        _ => display_kind(kind),
    }
}
