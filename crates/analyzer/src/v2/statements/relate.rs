//! RELATE statement analysis.
//!
//! Handles: RELATE source->edge->target SET ...,
//! validates edge is a relation table, validates source/target tables
//! match FROM/TO constraints, SET clause validates against the edge
//! table's fields (not source/target).
//!
//! Result type:
//! - RELATE source->edge->target SET ... → array<edge_type>
//! - RELATE ONLY source->edge->target SET ... → edge_type

use std::collections::HashSet;
use tree_sitter::Node;

use crate::context::{Context, SchemaMode, TableKind};
use crate::diagnostic::{Code, Diagnostic};
use crate::parser::{find_all, node_text};
use crate::span::Span;
use crate::types::{Kind, KindExt};
use crate::v2::expr;

/// Analyze a RELATE statement. Returns the result type.
pub fn analyze(node: &Node, source: &str, ctx: &mut Context) -> Kind {
    let is_only = find_all(node, "keyword_only").len() > 0;

    // Extract relate subjects: source -> edge -> target
    let subjects = collect_relate_subjects(node, source);

    let (source_table, edge_table, target_table) = match subjects.len() {
        3 => (
            Some(subjects[0].clone()),
            Some(subjects[1].clone()),
            Some(subjects[2].clone()),
        ),
        _ => (None, None, None),
    };

    // Validate edge is a relation table
    if let Some(ref edge) = edge_table {
        let edge_name = &edge.0;
        if let Some(table_def) = ctx.get_table(edge_name) {
            let kind = table_def.kind.clone();
            match kind {
                TableKind::Relation { ref from, ref to } => {
                    // Validate source matches FROM constraint
                    if let Some(ref from_tables) = from {
                        if let Some(ref src) = source_table {
                            let src_name = &src.0;
                            if !from_tables.iter().any(|t| t == src_name) {
                                ctx.emit(Diagnostic::error(
                                    src.1,
                                    Code::InvalidRelationTraversal,
                                    format!(
                                        "table `{}` is not a valid source for relation `{}`; expected one of: {}",
                                        src_name, edge_name,
                                        from_tables.join(", ")
                                    ),
                                ));
                            }
                        }
                    }
                    // Validate target matches TO constraint
                    if let Some(ref to_tables) = to {
                        if let Some(ref tgt) = target_table {
                            let tgt_name = &tgt.0;
                            if !to_tables.iter().any(|t| t == tgt_name) {
                                ctx.emit(Diagnostic::error(
                                    tgt.1,
                                    Code::InvalidRelationTraversal,
                                    format!(
                                        "table `{}` is not a valid target for relation `{}`; expected one of: {}",
                                        tgt_name, edge_name,
                                        to_tables.join(", ")
                                    ),
                                ));
                            }
                        }
                    }
                }
                _ => {
                    ctx.emit(Diagnostic::error(
                        edge.1,
                        Code::NotARelation,
                        format!("table `{}` is not a relation table", edge_name),
                    ));
                }
            }
        }
    }

    // Process SET clause (validates against the edge table, not source/target)
    process_set_clause(node, source, ctx, edge_table.as_ref().map(|e| e.0.as_str()));

    // Build result type from the edge table
    let table_type = edge_table
        .as_ref()
        .and_then(|e| ctx.build_table_type(&e.0))
        .unwrap_or(Kind::Any);

    let result = process_return_clause(node, &table_type);

    if is_only { result } else { Kind::Array(Box::new(result), None) }
}

// ── Relate subjects extraction ───────────────────────────────

/// Collect (table_name, span) for each relate_subject.
/// Order is: source, edge, target.
fn collect_relate_subjects(node: &Node, source: &str) -> Vec<(String, Span)> {
    let mut subjects = Vec::new();
    for subj in find_all(node, "relate_subject") {
        let span = Span::from_node(&subj);
        // Try identifier first
        let idents = find_all(&subj, "identifier");
        if let Some(ident) = idents.first() {
            subjects.push((node_text(ident, source).to_string(), span));
            continue;
        }
        // Try record_id (e.g. user:alice)
        let rids = find_all(&subj, "record_id");
        if let Some(rid) = rids.first() {
            let text = node_text(rid, source);
            if let Some(table) = text.split(':').next() {
                // For record_id, try object_key first (that's the table part)
                let keys = find_all(rid, "object_key");
                if let Some(key) = keys.first() {
                    subjects.push((node_text(key, source).to_string(), span));
                } else {
                    subjects.push((table.to_string(), span));
                }
                continue;
            }
        }
        // Fallback: use the raw text
        let text = node_text(&subj, source);
        let table = text.split(':').next().unwrap_or(&text);
        subjects.push((table.to_string(), span));
    }
    subjects
}

// ── SET clause ───────────────────────────────────────────────

fn process_set_clause(node: &Node, source: &str, ctx: &mut Context, table: Option<&str>) {
    let mut seen = HashSet::new();

    for assignment in find_all(node, "field_assignment") {
        let idents = find_all(&assignment, "identifier");
        let Some(field_ident) = idents.first() else { continue };
        let field_name = node_text(field_ident, source);

        // Duplicate check
        if !seen.insert(field_name.to_string()) {
            ctx.emit(Diagnostic::warning(
                Span::from_node(field_ident),
                Code::DuplicateFieldAssignment,
                format!("field `{}` is assigned multiple times", field_name),
            ));
        }

        let Some(tbl) = table else { continue };

        let is_schemafull = ctx.get_table(tbl)
            .map(|t| t.schema_mode == SchemaMode::Schemafull)
            .unwrap_or(false);

        let field_info = ctx.get_field(tbl, field_name).map(|f| f.typ.clone());

        if field_info.is_none() && is_schemafull {
            ctx.emit(Diagnostic::warning(
                Span::from_node(field_ident),
                Code::UndefinedFieldOnSchemafull,
                format!("field `{}` is not defined on table `{}`", field_name, tbl),
            ));
            continue;
        }

        if let Some(Some(ref expected)) = field_info.as_ref().map(|t| t.as_ref()) {
            if let Some(val_node) = find_assignment_value(&assignment) {
                let actual = expr::resolve_simple(&val_node, source, ctx, Some(tbl));
                if !actual.is_any() && !crate::types::is_assignable(expected, &actual) {
                    ctx.emit(Diagnostic::error(
                        Span::from_node(&val_node),
                        Code::IncompatibleAssignment,
                        format!("expected `{}`, found `{}`", expected, actual),
                    ));
                }
            }
        }
    }
}

fn find_assignment_value<'a>(node: &Node<'a>) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    let children: Vec<Node<'a>> = node.named_children(&mut cursor).collect();
    children.into_iter().rev().find(|c| {
        c.kind() != "identifier" && c.kind() != "operator"
    })
}

// ── RETURN clause ────────────────────────────────────────────

fn process_return_clause(node: &Node, table_type: &Kind) -> Kind {
    for rc in find_all(node, "return_clause") {
        let mut cursor = rc.walk();
        let children: Vec<_> = rc.named_children(&mut cursor).collect();
        for child in &children {
            match child.kind() {
                "keyword_none" => return Kind::Null,
                "keyword_before" => return Kind::Null,
                "keyword_after" => return table_type.clone(),
                "keyword_diff" => return Kind::Any,
                _ => {}
            }
        }
    }
    table_type.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{analyze_with_context, Context};

    fn test_schema() -> Context {
        let mut ctx = Context::new();
        let _ = analyze_with_context(
            r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD name ON user TYPE string;
                DEFINE FIELD age ON user TYPE int;

            DEFINE TABLE post SCHEMAFULL;
                DEFINE FIELD title ON post TYPE string;
                DEFINE FIELD body ON post TYPE string;

            DEFINE TABLE wrote TYPE RELATION FROM user TO post;
                DEFINE FIELD created_at ON wrote TYPE datetime;

            DEFINE TABLE follows TYPE RELATION FROM user TO user;

            DEFINE TABLE normal SCHEMAFULL;
                DEFINE FIELD value ON normal TYPE string;
            "#,
            &mut ctx,
        );
        ctx.take_diagnostics();
        ctx
    }

    fn query_type(query: &str) -> Kind {
        let mut ctx = test_schema();
        let full = format!("LET $result = {};", query);
        let _ = analyze_with_context(&full, &mut ctx);
        ctx.take_diagnostics();
        ctx.scope.lookup("$result")
            .map(|b| b.typ.clone())
            .unwrap_or(Kind::Any)
    }

    fn query_diagnostics(query: &str) -> Vec<crate::Diagnostic> {
        let mut ctx = test_schema();
        analyze_with_context(query, &mut ctx).unwrap_or_default()
    }

    // ── Valid relation ───────────────────────────────────────

    #[test]
    fn valid_relation() {
        let diags = query_diagnostics(
            "RELATE user:alice->wrote->post:first SET created_at = d'2024-01-01T00:00:00Z'"
        );
        let errors: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::NotARelation || d.code == Code::InvalidRelationTraversal)
            .collect();
        assert!(errors.is_empty(), "Valid relation should not error: {:?}", errors);
    }

    #[test]
    fn returns_array() {
        let typ = query_type(
            "RELATE user:alice->wrote->post:first SET created_at = d'2024-01-01T00:00:00Z'"
        );
        assert!(matches!(&typ, Kind::Array(_, _)), "Expected array, got: {:?}", typ);
    }

    // ── Not a relation ───────────────────────────────────────

    #[test]
    fn not_a_relation_error() {
        let diags = query_diagnostics(
            "RELATE user:alice->normal->post:first"
        );
        let errors: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::NotARelation)
            .collect();
        assert!(!errors.is_empty(), "Should error on non-relation table: {:?}", diags);
    }

    // ── Invalid source/target ────────────────────────────────

    #[test]
    fn invalid_source() {
        let diags = query_diagnostics(
            "RELATE post:first->wrote->post:second"
        );
        let errors: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::InvalidRelationTraversal)
            .collect();
        assert!(!errors.is_empty(), "Should error on invalid source: {:?}", diags);
    }

    #[test]
    fn invalid_target() {
        let diags = query_diagnostics(
            "RELATE user:alice->wrote->user:bob"
        );
        let errors: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::InvalidRelationTraversal)
            .collect();
        assert!(!errors.is_empty(), "Should error on invalid target: {:?}", diags);
    }

    // ── SET type check ───────────────────────────────────────

    #[test]
    fn set_type_mismatch_on_edge() {
        let diags = query_diagnostics(
            "RELATE user:alice->wrote->post:first SET created_at = 'not_a_datetime'"
        );
        let errors: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::IncompatibleAssignment)
            .collect();
        assert!(!errors.is_empty(), "Should error on SET type mismatch: {:?}", diags);
    }

    #[test]
    fn self_relation_valid() {
        let diags = query_diagnostics(
            "RELATE user:alice->follows->user:bob"
        );
        let errors: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::InvalidRelationTraversal || d.code == Code::NotARelation)
            .collect();
        assert!(errors.is_empty(), "Self-relation should be valid: {:?}", errors);
    }
}
